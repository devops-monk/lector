//! Model rate to device rate.
//!
//! Piper medium emits 22.05 kHz and Kokoro 24 kHz, while the output device is
//! almost always 48 kHz. `rubato`'s sinc resampler wants a fixed input block, so
//! this buffers the ragged chunks the synthesizer produces and drains them in
//! whole blocks, keeping the remainder for next time.

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};

pub struct Resampler2 {
    inner: Option<SincFixedIn<f32>>,
    block: usize,
    pending: Vec<f32>,
    out: Vec<f32>,
}

impl Resampler2 {
    pub fn new(from: u32, to: u32) -> Self {
        if from == to {
            return Self {
                inner: None,
                block: 0,
                pending: Vec::new(),
                out: Vec::new(),
            };
        }
        let block = 1024;
        let params = SincInterpolationParameters {
            sinc_len: 128,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 128,
            window: WindowFunction::BlackmanHarris2,
        };
        let inner = SincFixedIn::<f32>::new(to as f64 / from as f64, 2.0, params, block, 1)
            .expect("resampler construction");
        Self {
            inner: Some(inner),
            block,
            pending: Vec::new(),
            out: Vec::new(),
        }
    }

    /// Feeds samples in, returns whatever whole blocks came out.
    pub fn process(&mut self, input: &[f32]) -> &[f32] {
        self.out.clear();
        let Some(inner) = self.inner.as_mut() else {
            self.out.extend_from_slice(input);
            return &self.out;
        };
        self.pending.extend_from_slice(input);
        while self.pending.len() >= self.block {
            let chunk: Vec<f32> = self.pending.drain(..self.block).collect();
            if let Ok(done) = inner.process(&[chunk], None) {
                self.out.extend_from_slice(&done[0]);
            }
        }
        &self.out
    }

    /// Pads the remainder out to one final block so the tail of an utterance is
    /// not silently dropped.
    pub fn flush(&mut self) -> &[f32] {
        self.out.clear();
        let Some(inner) = self.inner.as_mut() else {
            return &self.out;
        };
        if !self.pending.is_empty() {
            let mut chunk: Vec<f32> = std::mem::take(&mut self.pending);
            chunk.resize(self.block, 0.0);
            if let Ok(done) = inner.process(&[chunk], None) {
                self.out.extend_from_slice(&done[0]);
            }
        }
        &self.out
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.out.clear();
    }
}
