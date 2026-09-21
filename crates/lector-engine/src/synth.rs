//! The synthesis actor: one thread that owns the model and the audio output.
//!
//! Both live on the same thread because `cpal::Stream` is `!Send` on macOS, and
//! because it makes the hot path lock-free: the model's per-sentence callback
//! resamples straight into the ring the audio device is draining.
//!
//! Cancellation is a **generation counter**, not a flag. Every job carries the
//! generation it was enqueued under; the model's callback compares that against
//! the current value on each sentence and returns `false` the moment they differ,
//! which aborts synthesis mid-utterance. A bool cannot express "cancel what is in
//! flight but not what I just enqueued" -- and that race happens on every
//! hotkey press that interrupts speech already playing.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsKittenModelConfig,
    OfflineTtsKokoroModelConfig, OfflineTtsModelConfig, OfflineTtsVitsModelConfig,
};

use crate::player::{PlaybackState, Player};
use crate::resample::Resampler2;
use crate::voice::{Engine as VoiceEngine, Voice};

/// 4 threads is the measured knee on Apple silicon (RTF 0.54 / 0.41 / 0.49 at
/// 2 / 4 / 8) and leaves cores for the audio thread.
const THREADS: i32 = 4;

/// Silence inserted between chunks. The models bake in their own leading and
/// trailing silence; we trim that and insert a deliberate gap instead, so the
/// pacing is ours rather than whatever the checkpoint happened to learn.
const SENTENCE_GAP: Duration = Duration::from_millis(150);

/// Below this the signal is treated as silence when trimming chunk edges.
const SILENCE_FLOOR: f32 = 0.005;
/// Fade applied to a trimmed edge, to avoid a click where we cut.
const FADE_MS: f32 = 10.0;

/// What a `Speak` carries. Boxed in `Command` because it is far larger than the
/// other variants, and every message on the channel would otherwise be sized to
/// fit this one.
pub struct SpeakJob {
    pub generation: u64,
    pub voice: Voice,
    pub chunks: Vec<String>,
    pub speed: f32,
}

/// Reading a document differs from speaking a selection in one way that
/// matters: the chunks are already enumerated and addressed, so a job carries a
/// starting index rather than a fresh list. Seeking is then just a new index
/// over the same vector, which is why there is no queue here -- what would have
/// been a queue is a cursor.
pub struct ReadJob {
    pub generation: u64,
    pub voice: Voice,
    pub chunks: Arc<Vec<String>>,
    pub from: usize,
    pub speed: f32,
}

pub enum Command {
    Speak(Box<SpeakJob>),
    Read(Box<ReadJob>),
    Stop,
    Shutdown,
}

/// Where in the current utterance the voice has actually reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    pub generation: u64,
    /// Index of the chunk now audible.
    pub index: usize,
    pub total: usize,
}

pub struct Handle {
    tx: Sender<Command>,
    generation: Arc<AtomicU64>,
    speaking: Arc<AtomicBool>,
    pub state: Arc<PlaybackState>,
    /// How many chunks the current job has, for reporting progress.
    total: Arc<AtomicUsize>,
}

impl Handle {
    /// Interrupts whatever is playing and speaks these chunks.
    pub fn speak(&self, voice: Voice, chunks: Vec<String>, speed: f32) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.total.store(chunks.len(), Ordering::Release);
        // Silence now. The counters are reset by the actor instead, once it has
        // stopped pushing the job this is interrupting.
        self.state.request_flush();
        self.state.set_paused(false);
        let _ = self.tx.send(Command::Speak(Box::new(SpeakJob {
            generation,
            voice,
            chunks,
            speed,
        })));
    }

    /// Reads an enumerated document from `from`, interrupting anything playing.
    ///
    /// The same call serves play, seek and restore-a-saved-position; there is
    /// no separate seek verb, because they differ only in the index.
    pub fn read(&self, voice: Voice, chunks: Arc<Vec<String>>, from: usize, speed: f32) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.total.store(chunks.len(), Ordering::Release);
        self.state.request_flush();
        self.state.set_paused(false);
        let _ = self.tx.send(Command::Read(Box::new(ReadJob {
            generation,
            voice,
            chunks,
            from,
            speed,
        })));
    }

    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.state.request_flush();
        self.state.set_paused(false);
        let _ = self.tx.send(Command::Stop);
    }

    /// Freezes playback without discarding anything.
    ///
    /// Costs nothing on the synthesis side: the ring stops draining, so the
    /// producer parks on the backpressure it already has.
    pub fn pause(&self) {
        self.state.set_paused(true);
    }

    pub fn resume(&self) {
        self.state.set_paused(false);
    }

    pub fn is_paused(&self) -> bool {
        self.state.is_paused()
    }

    /// True while anything is still to be heard.
    ///
    /// Deliberately not just "the actor is busy": at RTF 0.15 the actor
    /// finishes a paragraph long before the sound card does, and a caller that
    /// believed it would put the text box back while the voice was still
    /// reading from it.
    pub fn is_speaking(&self) -> bool {
        self.speaking.load(Ordering::Relaxed) || self.state.draining()
    }

    pub fn level(&self) -> f32 {
        self.state.level()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // Bump the generation first. An actor parked in `push_all` against a
        // full ring -- which a pause can cause indefinitely -- never returns to
        // `recv()`, so a bare Shutdown would leak the thread. The generation
        // change is what unwinds it.
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.state.request_flush();
        self.state.set_paused(false);
        let _ = self.tx.send(Command::Shutdown);
    }
}

/// How often the cursor thread checks whether the voice has reached a mark.
///
/// Well under the 150 ms sentence gap, so a highlight can never be late enough
/// to read as lag, and the thread costs nothing because it only wakes to
/// compare two integers.
const CURSOR_TICK: Duration = Duration::from_millis(25);

/// Starts the actor. Returns once the audio device is open and the model is
/// warm, so the first hotkey press does not pay the ~820 ms load.
///
/// `on_position` is called from a dedicated thread whenever the chunk being
/// heard changes -- never from the audio callback, which must not allocate, and
/// never from the synthesis thread, which sits inside C++ for seconds at a time.
pub fn spawn(
    voice: Voice,
    on_position: Option<Box<dyn Fn(Position) + Send + 'static>>,
) -> Result<Handle, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let generation = Arc::new(AtomicU64::new(0));
    let speaking = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();

    let (gen_t, speak_t) = (generation.clone(), speaking.clone());
    std::thread::Builder::new()
        .name("lector-synth".into())
        .spawn(move || match Actor::new(voice, gen_t, speak_t) {
            Ok(mut actor) => {
                let _ = ready_tx.send(Ok(actor.state.clone()));
                actor.run(rx);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        })
        .map_err(|e| e.to_string())?;

    let state = ready_rx.recv().map_err(|e| e.to_string())??;
    let total = Arc::new(AtomicUsize::new(0));

    if let Some(on_position) = on_position {
        let (state_c, gen_c, total_c) = (state.clone(), generation.clone(), total.clone());
        std::thread::Builder::new()
            .name("lector-cursor".into())
            .spawn(move || {
                let mut last: Option<Position> = None;
                loop {
                    std::thread::sleep(CURSOR_TICK);
                    // The Handle owning this generation is gone; so is the app.
                    if Arc::strong_count(&gen_c) == 1 {
                        return;
                    }
                    if state_c.is_paused() {
                        continue;
                    }
                    let generation = gen_c.load(Ordering::Acquire);
                    if let Some(m) = state_c.take_reached(generation) {
                        let p = Position {
                            generation,
                            index: m.index,
                            total: total_c.load(Ordering::Acquire),
                        };
                        // Only on change: roughly once every few seconds of
                        // reading, rather than forty times a second.
                        if last != Some(p) {
                            last = Some(p);
                            on_position(p);
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
    }

    Ok(Handle {
        tx,
        generation,
        speaking,
        state,
        total,
    })
}

/// Player and resampler together, behind a handle the model's callback can also
/// hold. The callback must be `'static`, so it cannot borrow the actor -- but it
/// runs on this same thread, so `Rc<RefCell<..>>` is sound and never contends.
/// Sharing them is what lets audio reach the device *during* synthesis rather
/// than after it, which is the whole point of the streaming callback.
struct Output {
    player: Player,
    resampler: Resampler2,
}

impl Output {
    /// Writes into the ring, waiting when it is full. Backpressure, not an error.
    /// Returns false if the generation moved on, meaning: give up, you are stale.
    fn push_all(&mut self, mut samples: &[f32], generation: u64, current: &AtomicU64) -> bool {
        while !samples.is_empty() {
            if current.load(Ordering::Acquire) != generation {
                return false;
            }
            let n = self.player.push(samples);
            samples = &samples[n..];
            if !samples.is_empty() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        true
    }
}

struct Actor {
    tts: OfflineTts,
    voice: Voice,
    out: Rc<RefCell<Output>>,
    device_rate: u32,
    state: Arc<PlaybackState>,
    generation: Arc<AtomicU64>,
    speaking: Arc<AtomicBool>,
}

impl Actor {
    fn new(
        voice: Voice,
        generation: Arc<AtomicU64>,
        speaking: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let player = Player::new()?;
        let tts = load(&voice)?;
        let resampler = Resampler2::new(tts.sample_rate() as u32, player.device_rate);
        let state = player.state.clone();
        let device_rate = player.device_rate;
        let out = Rc::new(RefCell::new(Output { player, resampler }));

        let mut actor = Self {
            tts,
            voice,
            out,
            device_rate,
            state,
            generation,
            speaking,
        };
        actor.warm();
        Ok(actor)
    }

    /// One tiny synthesis at startup so the first real request is not the one
    /// that pays for lazy initialisation inside the model.
    fn warm(&mut self) {
        let cfg = GenerationConfig {
            sid: self.voice.sid,
            speed: 1.0,
            ..Default::default()
        };
        let _ = self
            .tts
            .generate_with_config("Ready.", &cfg, None::<fn(&[f32], f32) -> bool>);
    }

    fn run(&mut self, rx: Receiver<Command>) {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Command::Shutdown => return,
                Command::Stop => {
                    self.out.borrow_mut().resampler.reset();
                    // Settle here, on the only thread allowed to: `stop` cut
                    // the sound from another thread but left `frames_pushed`
                    // holding the discarded job's length, which would read as
                    // audio still waiting to be heard for the rest of the run.
                    self.state.flush_and_settle();
                    self.speaking.store(false, Ordering::Relaxed);
                }
                Command::Read(job) => {
                    let ReadJob {
                        generation,
                        voice,
                        chunks,
                        from,
                        speed,
                    } = *job;
                    if generation < self.generation.load(Ordering::Acquire) {
                        continue;
                    }
                    if let Err(e) = self.ensure_voice(voice) {
                        eprintln!("lector: {e}");
                        continue;
                    }
                    self.speaking.store(true, Ordering::Relaxed);
                    self.read(generation, &chunks, from, speed);
                    self.speaking.store(false, Ordering::Relaxed);
                }
                Command::Speak(job) => {
                    let SpeakJob {
                        generation,
                        voice,
                        chunks,
                        speed,
                    } = *job;
                    if generation < self.generation.load(Ordering::Acquire) {
                        continue; // superseded before we even started
                    }
                    if let Err(e) = self.ensure_voice(voice) {
                        eprintln!("lector: {e}");
                        continue;
                    }
                    self.speaking.store(true, Ordering::Relaxed);
                    self.speak(generation, &chunks, speed);
                    self.speaking.store(false, Ordering::Relaxed);
                }
            }
        }
    }

    /// Loads a voice if it differs from the one in hand.
    fn ensure_voice(&mut self, voice: Voice) -> Result<(), String> {
        if voice == self.voice {
            return Ok(());
        }
        let tts = load(&voice)?;
        self.out.borrow_mut().resampler =
            Resampler2::new(tts.sample_rate() as u32, self.device_rate);
        self.tts = tts;
        self.voice = voice;
        Ok(())
    }

    /// Reads from `from` to the end of the document.
    ///
    /// Two failure modes, both real and both from readest: a unit that produces
    /// no audio should be skipped rather than stall the book, and a model that
    /// has started failing should abort rather than grind through every
    /// remaining unit. Neither should end a book early on a single bad chunk.
    fn read(&mut self, generation: u64, chunks: &[String], from: usize, speed: f32) {
        self.out.borrow_mut().resampler.reset();
        self.state.flush_and_settle();

        let mut failures = 0u32;
        for (i, chunk) in chunks.iter().enumerate().skip(from) {
            if self.generation.load(Ordering::Acquire) != generation {
                return;
            }
            if i > from {
                self.push_silence(SENTENCE_GAP);
            }
            self.state.mark(generation, i);
            if self.synthesize(generation, chunk, speed) {
                failures = 0;
            } else {
                failures += 1;
                if failures >= 5 {
                    eprintln!("lector: five consecutive synthesis failures, stopping");
                    return;
                }
            }
        }
        self.mark_end(generation, chunks.len());
    }

    /// Marks the end of a document, one index past the last chunk.
    ///
    /// The end is a mark like any other, so it becomes true when the last
    /// sample is *heard* rather than when the last one is generated. Anything
    /// that happens at the end of a chapter -- moving to the next one, putting
    /// the text box back -- has to wait for the voice, and this is what lets it.
    fn mark_end(&mut self, generation: u64, total: usize) {
        if self.generation.load(Ordering::Acquire) == generation {
            self.state.mark(generation, total);
        }
    }

    fn speak(&mut self, generation: u64, chunks: &[String], speed: f32) {
        self.out.borrow_mut().resampler.reset();

        // Reset the frame counters here rather than in `Handle::speak`. Only
        // this thread knows it has finished pushing the job being interrupted;
        // resetting any earlier lets that tail land after the reset and offset
        // every mark in this utterance by its length.
        self.state.flush_and_settle();

        for (i, chunk) in chunks.iter().enumerate() {
            if self.generation.load(Ordering::Acquire) != generation {
                return;
            }
            if i > 0 {
                self.push_silence(SENTENCE_GAP);
            }
            // Recorded after the gap and before synthesis, so the highlight
            // moves when the voice starts rather than during the pause.
            self.state.mark(generation, i);
            self.synthesize(generation, chunk, speed);
        }
        self.mark_end(generation, chunks.len());
    }

    /// Returns whether the model produced audio.
    ///
    /// The result was previously discarded. It matters now: a document is long
    /// enough that a run of failures means the model is broken, not the text,
    /// and grinding silently through a hundred more units helps nobody.
    fn synthesize(&mut self, generation: u64, text: &str, speed: f32) -> bool {
        let current = self.generation.clone();
        let out = self.out.clone();
        let model_rate = self.tts.sample_rate() as u32;

        // Fires once per sentence, on this thread, inside the C++ call. It
        // resamples and writes straight into the ring the device is draining, so
        // the first sentence is audible while the rest is still being generated.
        let cb = move |samples: &[f32], _progress: f32| -> bool {
            // The C++ side frees this buffer on return, so copy before anything.
            let piece = trim_and_fade(samples, model_rate);
            let mut o = out.borrow_mut();
            let resampled: Vec<f32> = o.resampler.process(&piece).to_vec();
            if !o.push_all(&resampled, generation, &current) {
                return false;
            }
            // Returning false aborts mid-utterance. This is the whole
            // cancellation mechanism: one atomic read per sentence.
            current.load(Ordering::Acquire) == generation
        };

        let cfg = GenerationConfig {
            sid: self.voice.sid,
            speed,
            ..Default::default()
        };
        let produced = self
            .tts
            .generate_with_config(text, &cfg, Some(cb))
            .is_some();

        let tail: Vec<f32> = self.out.borrow_mut().resampler.flush().to_vec();
        self.out
            .borrow_mut()
            .push_all(&tail, generation, &self.generation);
        produced
    }

    fn push_silence(&mut self, d: Duration) {
        let n = (self.device_rate as f64 * d.as_secs_f64()) as usize;
        let silence = vec![0.0f32; n];
        let gen = self.generation.load(Ordering::Acquire);
        self.out
            .borrow_mut()
            .push_all(&silence, gen, &self.generation);
    }
}

/// Trims the model's baked-in silence and fades the cut edges.
///
/// Left as-is, each chunk carries leading and trailing quiet that compounds into
/// long dead air between sentences. We cut it and insert our own gap instead.
/// The fade matters: cutting mid-waveform without one is an audible click.
fn trim_and_fade(samples: &[f32], rate: u32) -> Vec<f32> {
    let start = samples
        .iter()
        .position(|s| s.abs() > SILENCE_FLOOR)
        .unwrap_or(0);
    let end = samples
        .iter()
        .rposition(|s| s.abs() > SILENCE_FLOOR)
        .map(|i| i + 1)
        .unwrap_or(samples.len());
    if start >= end {
        return Vec::new();
    }
    let mut out = samples[start..end].to_vec();

    let fade = ((rate as f32) * FADE_MS / 1000.0) as usize;
    let fade = fade.min(out.len() / 2);
    for i in 0..fade {
        let g = i as f32 / fade as f32;
        out[i] *= g;
        let j = out.len() - 1 - i;
        out[j] *= g;
    }

    // DC offset between chunks is a step, and a step is a click no fade removes.
    let mean = out.iter().sum::<f32>() / out.len() as f32;
    if mean.abs() > 1e-4 {
        for s in out.iter_mut() {
            *s -= mean;
        }
    }

    // Clip protection only. Per-chunk peak normalisation is deliberately absent:
    // it makes loudness pump between sentences, because a quiet sentence gets
    // boosted to match a loud one.
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 1.0 {
        for s in out.iter_mut() {
            *s /= peak;
        }
    }
    out
}

/// Builds the engine-specific model configuration for a voice.
///
/// Public so the measurement binaries use the same code path the app does.
/// They used to build their own copy, which silently drifted every time a new
/// engine family was added.
pub fn model_config(voice: &Voice) -> OfflineTtsModelConfig {
    match voice.engine {
        VoiceEngine::Piper => OfflineTtsModelConfig {
            vits: OfflineTtsVitsModelConfig {
                model: Some(voice.model_file.clone()),
                tokens: Some(voice.tokens.clone()),
                // Piper phonemizes with espeak; VCTK looks words up in a
                // lexicon instead. Passing whichever the archive carries lets
                // one arm serve both.
                data_dir: voice.data_dir.clone(),
                lexicon: voice.lexicon.clone(),
                ..Default::default()
            },
            num_threads: THREADS,
            ..Default::default()
        },
        VoiceEngine::Kokoro => OfflineTtsModelConfig {
            kokoro: OfflineTtsKokoroModelConfig {
                model: Some(voice.model_file.clone()),
                tokens: Some(voice.tokens.clone()),
                data_dir: voice.data_dir.clone(),
                voices: voice.voices_bin.clone(),
                lexicon: voice.lexicon.clone(),
                dict_dir: voice.dict_dir.clone(),
                ..Default::default()
            },
            num_threads: THREADS,
            ..Default::default()
        },
        VoiceEngine::Kitten => OfflineTtsModelConfig {
            kitten: OfflineTtsKittenModelConfig {
                model: Some(voice.model_file.clone()),
                tokens: Some(voice.tokens.clone()),
                data_dir: voice.data_dir.clone(),
                voices: voice.voices_bin.clone(),
                ..Default::default()
            },
            num_threads: THREADS,
            ..Default::default()
        },
    }
}

fn load(voice: &Voice) -> Result<OfflineTts, String> {
    let t0 = Instant::now();
    let cfg = OfflineTtsConfig {
        model: model_config(voice),
        ..Default::default()
    };
    let tts = OfflineTts::create(&cfg).ok_or_else(|| format!("could not load {}", voice.id))?;
    eprintln!(
        "lector: loaded {} in {} ms",
        voice.id,
        t0.elapsed().as_millis()
    );
    Ok(tts)
}
