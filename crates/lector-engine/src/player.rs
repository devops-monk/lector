//! Audio output: one long-lived stream, fed from a lock-free ring.
//!
//! The stream is opened once and **never stops**, feeding silence when idle.
//! Restarting a stream per utterance is what lets Bluetooth fade-in and noise
//! gates swallow the first word, and on AirPods it costs the opening ~200 ms of
//! every sentence.
//!
//! Nothing here allocates, locks, or emits on the audio thread.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapProd, HeapRb};

/// How much audio the ring can hold. Generous: synthesis runs ~6x faster than
/// playback, so the producer is normally far ahead.
const RING_SECONDS: usize = 30;

/// Shared counters the audio callback writes and everyone else reads.
pub struct PlaybackState {
    /// Frames the hardware has actually consumed. This is the position clock:
    /// a highlight mark becomes audible when this crosses its offset, which is
    /// why marks must never be emitted at synthesis time.
    pub frames_played: AtomicUsize,
    /// Bumped to discard everything queued. The audio thread notices and drains.
    flush_epoch: AtomicU64,
    /// Most recent output level, as an f32 bitcast, for the meter.
    level: AtomicU64,
}

impl PlaybackState {
    /// Discards everything queued, from any thread. The cut happens in the next
    /// audio callback -- within one buffer, so a few milliseconds.
    pub fn request_flush(&self) {
        self.flush_epoch.fetch_add(1, Ordering::Release);
    }
}

impl PlaybackState {
    fn new() -> Self {
        Self {
            frames_played: AtomicUsize::new(0),
            flush_epoch: AtomicU64::new(0),
            level: AtomicU64::new(0),
        }
    }

    /// Peak level of the last callback, in 0.0..=1.0.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed) as u32)
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
                    // cut is immediate rather than after the buffered audio.
                    let epoch = cb_state.flush_epoch.load(Ordering::Acquire);
                    if epoch != seen_epoch {
                        seen_epoch = epoch;
                        cons.clear();
                    }

                    let frames = out.len() / channels;
                    let mut peak = 0.0f32;
                    let mut got = 0usize;
                    for f in 0..frames {
                        let s = cons.try_pop().unwrap_or(0.0);
                        if s != 0.0 {
                            got += 1;
                        }
                        let a = s.abs();
                        if a > peak {
                            peak = a;
                        }
                        // Mono source duplicated across channels.
                        for c in 0..channels {
                            out[f * channels + c] = s;
                        }
                    }
                    let _ = got;
                    cb_state.frames_played.fetch_add(frames, Ordering::Relaxed);
                    cb_state.level.store(peak.to_bits() as u64, Ordering::Relaxed);
                },
                |e| eprintln!("lector: audio stream error: {e}"),
                None,
            )
            .map_err(|e| e.to_string())?;

        stream.play().map_err(|e| e.to_string())?;

        Ok(Self { _stream: stream, prod, state, device_rate })
    }

    /// Queues mono samples already at the device rate.
    ///
    /// Returns how many were accepted. A short write means the ring is full,
    /// which is backpressure, not an error -- the caller should retry.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        self.prod.push_slice(samples)
    }

    pub fn free_capacity(&self) -> usize {
        self.prod.vacant_len()
    }

    /// Whether the hardware still has queued audio to play.
    pub fn is_playing(&self) -> bool {
        self.prod.occupied_len() > 0
    }
}
