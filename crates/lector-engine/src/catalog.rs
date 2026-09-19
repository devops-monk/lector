//! What voices exist, where to get them, and what they should hash to.
//!
//! Static and compiled in, on purpose. A model list that can change under you is
//! a model list that can serve you a different binary tomorrow -- and for an app
//! whose entire pitch is that nothing leaves the machine, a remote catalog would
//! be the one phone-home.
//!
//! The catalog knows labels, sizes and URLs. It deliberately does **not** know
//! file layouts: which engine a directory holds is inferred from its contents by
//! `Voice::from_dir`, so an archive packed slightly differently still loads.

use crate::voice::Engine;

const TTS_RELEASE: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models";

/// One selectable speaker within a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Speaker {
    /// The voice's own name, as the model knows it.
    pub name: &'static str,
    /// Index into the model's speaker table -- the only thing it actually takes.
    pub sid: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Model {
    /// Also the directory name under `models/`.
    pub id: &'static str,
    pub engine: Engine,
    pub label: &'static str,
    pub mb: u32,
    pub url: &'static str,
    pub sha256: &'static str,
    pub speakers: &'static [Speaker],
    /// What choosing this one costs and buys, for the picker.
    pub tradeoff: &'static str,
}

/// Kokoro's speaker ids are positions in `voices.bin`, fixed by the script that
/// built it. The model carries 53, most of them Chinese; these are the English
/// ones, because a wall of names helps nobody choose.
const KOKORO_EN: &[Speaker] = &[
    Speaker { name: "af_heart", sid: 3 },
    Speaker { name: "af_bella", sid: 2 },
    Speaker { name: "am_michael", sid: 16 },
    Speaker { name: "bf_emma", sid: 21 },
    Speaker { name: "bm_george", sid: 26 },
];

const PIPER_AMY: &[Speaker] = &[Speaker { name: "amy", sid: 0 }];

/// English-first. The other seven Piper languages verba ships are deliberately
/// absent: each is a checksum someone has to keep correct, for no value to a
/// user reading English.
pub const CATALOG: &[Model] = &[
    Model {
        id: "vits-piper-en_US-amy-medium-int8",
        engine: Engine::Piper,
        label: "Piper",
        mb: 21,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_US-amy-medium-int8.tar.bz2",
        sha256: "bd23c0aa629eb3719448582f45ede49e8fa6a679061fed5eab16a6a6fd8e7e82",
        speakers: PIPER_AMY,
        tradeoff: "Fast and small. Ready in seconds.",
    },
    Model {
        id: "kokoro-multi-lang-v1_0",
        engine: Engine::Kokoro,
        label: "Kokoro",
        mb: 349,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-multi-lang-v1_0.tar.bz2",
        sha256: "c133d26353d776da730870dac7da07dbfc9a5e3bc80cc5e8e83ab6e823be7046",
        speakers: KOKORO_EN,
        tradeoff: "Noticeably better. A larger download.",
    },
];

/// The voice Lector speaks with until told otherwise: the small one, so the app
/// works within seconds of first launch rather than after a 349 MB download.
pub const DEFAULT_MODEL: &str = "vits-piper-en_US-amy-medium-int8";

pub fn model(id: &str) -> Option<&'static Model> {
    CATALOG.iter().find(|m| m.id == id)
}

/// A short line each voice reads when auditioned.
///
/// No voice should be chosen without being heard, and a sentence in the register
/// the app actually speaks in is a fairer sample than a pangram.
pub const AUDITION: &str = "This is how I sound. Shall I read that for you?";

#[allow(dead_code)]
fn _catalog_is_consistent() {
    // Compile-time-ish guard: every entry's URL must live on the release we pin.
    debug_assert!(CATALOG.iter().all(|m| m.url.starts_with(TTS_RELEASE)));
}
