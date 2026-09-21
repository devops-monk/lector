//! Audio output: one long-lived stream, fed from a lock-free ring.
//!
//! The stream is opened once and **never stops**, feeding silence when idle.
//! Restarting a stream per utterance is what lets Bluetooth fade-in and noise
//! gates swallow the first word, and on AirPods it costs the opening ~200 ms of
//! every sentence.
//!
//! Nothing here allocates, locks, or emits on the audio thread.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapProd, HeapRb};

/// How much audio the ring can hold. Generous: synthesis runs ~6x faster than
/// playback, so the producer is normally far ahead.
const RING_SECONDS: usize = 30;

/// How long `flush_and_settle` waits for the audio thread to confirm the cut.
const SETTLE_TIMEOUT_MS: u64 = 50;

/// "Chunk `index` starts at this many frames into the current generation."
///
/// Recorded by the synthesis thread as it pushes; consumed by the cursor thread
/// once the sound card has actually rendered that far.
#[derive(Clone, Copy, Debug)]
pub struct Mark {
    pub generation: u64,
    pub index: usize,
    pub at_frame: usize,
}

/// Shared state between the audio callback, the synthesis actor and the cursor.
pub struct PlaybackState {
    /// Frames the sound card has actually *rendered from the ring*.
    ///
    /// This is the position clock: a mark becomes audible when this crosses its
    /// offset, which is why marks must never be emitted at synthesis time.
    ///
    /// It deliberately does NOT count buffers served from an empty ring. Those
    /// are underruns, not content -- counting them would let idle time advance
    /// the clock and drift every highlight afterwards.
    frames_rendered: AtomicUsize,

    /// Frames handed to the ring. Same coordinate space as `frames_rendered`,
    /// because the source is mono at the device rate: one sample is one frame.
    /// A sample pushed at offset X is audible when rendered crosses X.
    frames_pushed: AtomicUsize,

    /// Bumped to discard everything queued. The audio thread notices and drains.
    flush_epoch: AtomicU64,

    /// The epoch the audio thread has actually acted on. Without this, a flush
    /// is racy: the producer starts pushing before the callback has cleared,
    /// and the new audio is thrown away with the old.
    acked_epoch: AtomicU64,

    /// While true the callback emits silence and pops nothing, so the ring and
    /// the clock both freeze. Pause is a consumer-side gate on purpose -- see
    /// `set_paused`.
    paused: AtomicBool,

    /// Most recent output level, as an f32 bitcast, for the meter.
    level: AtomicU64,

    /// Pending marks, oldest first. Never touched by the audio thread.
    marks: Mutex<VecDeque<Mark>>,
}

impl PlaybackState {
    fn new() -> Self {
        Self {
            frames_rendered: AtomicUsize::new(0),
            frames_pushed: AtomicUsize::new(0),
            flush_epoch: AtomicU64::new(0),
            acked_epoch: AtomicU64::new(0),
            paused: AtomicBool::new(false),
            level: AtomicU64::new(0),
            marks: Mutex::new(VecDeque::new()),
        }
    }

    /// Peak level of the last callback, in 0.0..=1.0.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed) as u32)
    }

    pub fn frames_rendered(&self) -> usize {
        self.frames_rendered.load(Ordering::Acquire)
    }

    pub fn frames_pushed(&self) -> usize {
        self.frames_pushed.load(Ordering::Acquire)
    }

    /// True while audio already generated has not been heard yet.
    ///
    /// Synthesis runs several times faster than speech, so "the actor has
    /// finished" and "the voice has finished" are seconds apart -- long enough
    /// that a UI keyed on the former closes the reading view mid-sentence.
    /// Both counters share an origin, reset together by a flush, so their
    /// difference is exactly the audio still queued.
    pub fn draining(&self) -> bool {
        self.frames_rendered() < self.frames_pushed()
    }

    /// Pauses or resumes playback.
    ///
    /// Gated at the consumer rather than the producer, which buys three things
    /// for about six lines: the ring keeps its samples so resume is instant,
    /// the clock freezes automatically because nothing is popped, and the
    /// synthesis thread parks itself on the backpressure it already has.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// Silences the ring immediately, from any thread.
    ///
    /// This is the *audible* cut and it is all a caller wants: pressing stop
    /// should go quiet now, not when the synthesis thread next looks up. It
    /// does not reset the frame counters, because the actor may still be
    /// pushing the tail of the job being cancelled -- see `flush_and_settle`.
    pub fn request_flush(&self) {
        self.flush_epoch.fetch_add(1, Ordering::Release);
    }

    /// Discards queued audio and waits for the audio thread to confirm it.
    ///
    /// The wait is what makes marks trustworthy. `request_flush` alone returns
    /// before the callback has cleared the ring, so audio pushed immediately
    /// afterwards gets discarded along with the old -- and the mark offsets
    /// that went with it are then wrong for the rest of the utterance.
    /// Called by the synthesis thread, and only there: it must run after that
    /// thread has stopped pushing the old generation and before it pushes the
    /// first sample of the new one, or `frames_pushed` carries a tail the marks
    /// do not account for.
    pub fn flush_and_settle(&self) {
        let epoch = self.flush_epoch.fetch_add(1, Ordering::Release) + 1;

        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(SETTLE_TIMEOUT_MS);
        while self.acked_epoch.load(Ordering::Acquire) < epoch {
            if std::time::Instant::now() > deadline {
                // A silent device never calls back. Carry on rather than hang:
                // the clock restarts either way, and nothing is audible anyway.
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        self.frames_pushed.store(0, Ordering::Release);
        self.marks.lock().unwrap().clear();
    }

    /// Records that a chunk's audio begins at the current push offset.
    pub fn mark(&self, generation: u64, index: usize) {
        let at_frame = self.frames_pushed.load(Ordering::Acquire);
        self.marks.lock().unwrap().push_back(Mark {
            generation,
            index,
            at_frame,
        });
    }

    /// The newest mark the sound card has reached, discarding those it passed
    /// and any left over from a superseded generation.
    pub fn take_reached(&self, generation: u64) -> Option<Mark> {
        let rendered = self.frames_rendered();
        let mut marks = self.marks.lock().unwrap();
        let mut latest = None;
        while let Some(m) = marks.front().copied() {
            if m.generation != generation {
                marks.pop_front();
                continue;
            }
            if m.at_frame > rendered {
                break;
            }
            marks.pop_front();
            latest = Some(m);
        }
        latest
    }
}

pub struct Player {
    _stream: cpal::Stream,
    prod: HeapProd<f32>,
    pub state: Arc<PlaybackState>,
    pub device_rate: u32,
}

impl Player {
    pub fn new() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or("no output device")?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let device_rate = supported.sample_rate().0;
        let channels = supported.channels() as usize;

        let rb = HeapRb::<f32>::new(device_rate as usize * RING_SECONDS);
        let (prod, mut cons) = rb.split();

        let state = Arc::new(PlaybackState::new());
        let cb_state = state.clone();
        let mut seen_epoch = 0u64;

        let stream = device
            .build_output_stream(
                &supported.config(),
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    // A stop bumps the epoch; drop whatever is still queued so the
                    // cut is immediate rather than after the buffered audio. The
                    // ack is what lets a flush be waited on from another thread.
                    let epoch = cb_state.flush_epoch.load(Ordering::Acquire);
                    if epoch != seen_epoch {
                        seen_epoch = epoch;
                        cons.clear();
                        cb_state.frames_rendered.store(0, Ordering::Release);
                        cb_state.acked_epoch.store(epoch, Ordering::Release);
                    }

                    // Paused: emit silence, pop nothing. The ring keeps its
                    // contents and the clock stops, both of which resume needs.
                    if cb_state.paused.load(Ordering::Acquire) {
                        out.fill(0.0);
                        cb_state
                            .level
                            .store(0f32.to_bits() as u64, Ordering::Relaxed);
                        return;
                    }

                    let frames = out.len() / channels;
                    let mut peak = 0.0f32;
                    // Frames actually taken from the ring, as opposed to
                    // silence invented because it was empty.
                    let mut rendered = 0usize;

                    for f in 0..frames {
                        let s = match cons.try_pop() {
                            Some(v) => {
                                rendered += 1;
                                v
                            }
                            None => 0.0,
                        };
                        let a = s.abs();
                        if a > peak {
                            peak = a;
                        }
                        // Mono source duplicated across channels.
                        for c in 0..channels {
                            out[f * channels + c] = s;
                        }
                    }

                    cb_state
                        .frames_rendered
                        .fetch_add(rendered, Ordering::Release);
                    cb_state
                        .level
                        .store(peak.to_bits() as u64, Ordering::Relaxed);
                },
                |e| eprintln!("lector: audio stream error: {e}"),
                None,
            )
            .map_err(|e| e.to_string())?;

        stream.play().map_err(|e| e.to_string())?;

        Ok(Self {
            _stream: stream,
            prod,
            state,
            device_rate,
        })
    }

    /// Queues mono samples already at the device rate.
    ///
    /// Returns how many were accepted. A short write means the ring is full,
    /// which is backpressure, not an error -- the caller should retry.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let n = self.prod.push_slice(samples);
        self.state.frames_pushed.fetch_add(n, Ordering::Release);
        n
    }

    /// Whether the ring still holds audio the sound card has not taken.
    ///
    /// This is how the actor knows the last chunk has finished being *heard*,
    /// which is when a document is genuinely over.
    pub fn is_playing(&self) -> bool {
        self.prod.occupied_len() > 0
    }
}
