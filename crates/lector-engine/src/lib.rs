//! Lector's speech engine: text in, sound out, entirely in this process.
//!
//! Inference is sherpa-onnx linked statically, so there is no Python, no sidecar
//! and no phonemizer subprocess. Playback is cpal, owned by the same thread as
//! the model, so audio never crosses an IPC boundary -- which matters because
//! two of the app's surfaces have no window to play it in.

pub mod catalog;
pub mod install;
pub mod player;
pub mod resample;
pub mod synth;
pub mod voice;

use std::path::Path;

pub use synth::Handle;
pub use voice::{Engine, Voice};

use lector_text::SanitizeOptions;

/// The speech engine. Cheap to clone-free share behind an `Arc`.
pub struct Lector {
    handle: Handle,
    voice: Voice,
    speed: f32,
}

impl Lector {
    /// Opens the audio device and warms the model. Blocks until ready, so the
    /// first hotkey press does not pay the ~820 ms load.
    pub fn new(model_dir: &Path) -> Result<Self, String> {
        Self::with_voice(Voice::from_dir(model_dir, 0)?)
    }

    /// Starts with a voice already chosen.
    ///
    /// Prefer this over `new` followed by `set_voice`: that pair loads the model
    /// twice, once for the default and again when the choice turns out to
    /// differ, and each load is most of a second.
    pub fn with_voice(voice: Voice) -> Result<Self, String> {
        let handle = synth::spawn(voice.clone())?;
        Ok(Self {
            handle,
            voice,
            speed: 1.0,
        })
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(0.5, 3.0);
    }

    /// Switches voice. The model is loaded lazily on the next utterance, by the
    /// actor thread, so this returns immediately rather than blocking for the
    /// ~800 ms a load takes.
    pub fn set_voice(&mut self, voice: Voice) {
        self.voice = voice;
    }

    pub fn voice(&self) -> &Voice {
        &self.voice
    }

    /// Speaks text, interrupting anything already playing.
    ///
    /// `first_chunk_fast` trades a little prosody for a lot of latency and should
    /// be true for anything the user is waiting on.
    pub fn speak(&self, text: &str, opts: SanitizeOptions, first_chunk_fast: bool) {
        let chunks = lector_text::prepare(text, opts, first_chunk_fast);
        if chunks.is_empty() {
            return;
        }
        self.handle.speak(self.voice.clone(), chunks, self.speed);
    }

    /// Stops immediately, mid-sentence.
    pub fn stop(&self) {
        self.handle.stop();
    }

    pub fn is_speaking(&self) -> bool {
        self.handle.is_speaking()
    }

    /// Output level in 0.0..=1.0, for a meter.
    pub fn level(&self) -> f32 {
        self.handle.level()
    }
}
