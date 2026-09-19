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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsKokoroModelConfig,
    OfflineTtsModelConfig, OfflineTtsVitsModelConfig,
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

pub enum Command {
    Speak { generation: u64, voice: Voice, chunks: Vec<String>, speed: f32 },
    Stop,
    Shutdown,
}

pub struct Handle {
    tx: Sender<Command>,
    generation: Arc<AtomicU64>,
    speaking: Arc<AtomicBool>,
    pub state: Arc<PlaybackState>,
}

impl Handle {
    /// Interrupts whatever is playing and speaks these chunks.
    pub fn speak(&self, voice: Voice, chunks: Vec<String>, speed: f32) {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.state.request_flush();
        let _ = self.tx.send(Command::Speak { generation, voice, chunks, speed });
    }

    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.state.request_flush();
        let _ = self.tx.send(Command::Stop);
    }

    pub fn is_speaking(&self) -> bool {
        self.speaking.load(Ordering::Relaxed)
    }

    pub fn level(&self) -> f32 {
        self.state.level()
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}

/// Starts the actor. Returns once the audio device is open and the model is
/// warm, so the first hotkey press does not pay the ~820 ms load.
pub fn spawn(voice: Voice) -> Result<Handle, String> {
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
    Ok(Handle { tx, generation, speaking, state })
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
    fn new(voice: Voice, generation: Arc<AtomicU64>, speaking: Arc<AtomicBool>) -> Result<Self, String> {
        let player = Player::new()?;
        let tts = load(&voice)?;
        let resampler = Resampler2::new(tts.sample_rate() as u32, player.device_rate);
        let state = player.state.clone();
        let device_rate = player.device_rate;
        let out = Rc::new(RefCell::new(Output { player, resampler }));

        let mut actor = Self { tts, voice, out, device_rate, state, generation, speaking };
        actor.warm();
        Ok(actor)
    }

    /// One tiny synthesis at startup so the first real request is not the one
    /// that pays for lazy initialisation inside the model.
    fn warm(&mut self) {
        let cfg = GenerationConfig { sid: self.voice.sid, speed: 1.0, ..Default::default() };
        let _ = self.tts.generate_with_config("Ready.", &cfg, None::<fn(&[f32], f32) -> bool>);
    }

    fn run(&mut self, rx: Receiver<Command>) {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Command::Shutdown => return,
                Command::Stop => {
                    self.out.borrow_mut().resampler.reset();
                    self.speaking.store(false, Ordering::Relaxed);
                }
                Command::Speak { generation, voice, chunks, speed } => {
                    if generation < self.generation.load(Ordering::Acquire) {
                        continue; // superseded before we even started
                    }
                    if voice != self.voice {
                        match load(&voice) {
                            Ok(tts) => {
                                self.out.borrow_mut().resampler =
                                    Resampler2::new(tts.sample_rate() as u32, self.device_rate);
                                self.tts = tts;
                                self.voice = voice;
                            }
                            Err(e) => {
                                eprintln!("lector: failed to load voice: {e}");
                                continue;
                            }
                        }
                    }
                    self.speaking.store(true, Ordering::Relaxed);
                    self.speak(generation, &chunks, speed);
                    self.speaking.store(false, Ordering::Relaxed);
                }
            }
        }
    }

    fn speak(&mut self, generation: u64, chunks: &[String], speed: f32) {
        self.out.borrow_mut().resampler.reset();
        for (i, chunk) in chunks.iter().enumerate() {
            if self.generation.load(Ordering::Acquire) != generation {
                return;
            }
            if i > 0 {
                self.push_silence(SENTENCE_GAP);
            }
            self.synthesize(generation, chunk, speed);
        }
    }

    fn synthesize(&mut self, generation: u64, text: &str, speed: f32) {
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

        let cfg = GenerationConfig { sid: self.voice.sid, speed, ..Default::default() };
        let _ = self.tts.generate_with_config(text, &cfg, Some(cb));

        let tail: Vec<f32> = self.out.borrow_mut().resampler.flush().to_vec();
        self.out.borrow_mut().push_all(&tail, generation, &self.generation);
    }

    fn push_silence(&mut self, d: Duration) {
        let n = (self.device_rate as f64 * d.as_secs_f64()) as usize;
        let silence = vec![0.0f32; n];
        let gen = self.generation.load(Ordering::Acquire);
        self.out.borrow_mut().push_all(&silence, gen, &self.generation);
    }
}

/// Trims the model's baked-in silence and fades the cut edges.
///
/// Left as-is, each chunk carries leading and trailing quiet that compounds into
/// long dead air between sentences. We cut it and insert our own gap instead.
/// The fade matters: cutting mid-waveform without one is an audible click.
fn trim_and_fade(samples: &[f32], rate: u32) -> Vec<f32> {
    let start = samples.iter().position(|s| s.abs() > SILENCE_FLOOR).unwrap_or(0);
    let end = samples.iter().rposition(|s| s.abs() > SILENCE_FLOOR).map(|i| i + 1).unwrap_or(samples.len());
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

fn load(voice: &Voice) -> Result<OfflineTts, String> {
    let t0 = Instant::now();
    let model = match voice.engine {
        VoiceEngine::Piper => OfflineTtsModelConfig {
            vits: OfflineTtsVitsModelConfig {
                model: Some(voice.model_file.clone()),
                tokens: Some(voice.tokens.clone()),
                data_dir: Some(voice.data_dir.clone()),
                ..Default::default()
            },
            num_threads: THREADS,
            ..Default::default()
        },
        VoiceEngine::Kokoro => OfflineTtsModelConfig {
            kokoro: OfflineTtsKokoroModelConfig {
                model: Some(voice.model_file.clone()),
                tokens: Some(voice.tokens.clone()),
                data_dir: Some(voice.data_dir.clone()),
                voices: Some(voice.voices_bin.clone().unwrap_or_default()),
                ..Default::default()
            },
            num_threads: THREADS,
            ..Default::default()
        },
    };
    let cfg = OfflineTtsConfig { model, ..Default::default() };
    let tts = OfflineTts::create(&cfg).ok_or_else(|| format!("could not load {}", voice.id))?;
    eprintln!("lector: loaded {} in {} ms", voice.id, t0.elapsed().as_millis());
    Ok(tts)
}
