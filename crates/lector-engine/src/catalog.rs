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
    /// A few words on how it sounds, for the picker.
    pub note: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Model {
    /// Also the directory name under `models/`.
    pub id: &'static str,
    pub engine: Engine,
    pub label: &'static str,
    /// Shown beside the name, so a picker does not need a second column.
    pub accent: &'static str,
    pub mb: u32,
    pub url: &'static str,
    pub sha256: &'static str,
    pub speakers: &'static [Speaker],
    /// What choosing this one costs and buys, for the picker.
    pub tradeoff: &'static str,
}

/// Kokoro's English speakers.
///
/// The ids are positions in `voices.bin` and are not guessable, so they were
/// read out of the model's own `id2speaker` metadata rather than transcribed
/// from anywhere. The model carries 54 voices; these are the 28 English ones.
/// The rest are Spanish, French, Hindi, Italian, Japanese, Portuguese and
/// Chinese, and are left out only because Lector ships English lexicons.
///
/// Prefix convention: `af`/`am` American female/male, `bf`/`bm` British.
const KOKORO_EN: &[Speaker] = &[
    Speaker {
        name: "Heart",
        sid: 3,
        note: "American · warm",
    },
    Speaker {
        name: "Bella",
        sid: 2,
        note: "American · bright",
    },
    Speaker {
        name: "Nicole",
        sid: 6,
        note: "American · soft",
    },
    Speaker {
        name: "Sarah",
        sid: 9,
        note: "American",
    },
    Speaker {
        name: "Sky",
        sid: 10,
        note: "American · light",
    },
    Speaker {
        name: "Aoede",
        sid: 1,
        note: "American",
    },
    Speaker {
        name: "Alloy",
        sid: 0,
        note: "American · even",
    },
    Speaker {
        name: "Jessica",
        sid: 4,
        note: "American",
    },
    Speaker {
        name: "Kore",
        sid: 5,
        note: "American",
    },
    Speaker {
        name: "Nova",
        sid: 7,
        note: "American",
    },
    Speaker {
        name: "River",
        sid: 8,
        note: "American · calm",
    },
    Speaker {
        name: "Michael",
        sid: 16,
        note: "American · steady",
    },
    Speaker {
        name: "Adam",
        sid: 11,
        note: "American",
    },
    Speaker {
        name: "Echo",
        sid: 12,
        note: "American",
    },
    Speaker {
        name: "Eric",
        sid: 13,
        note: "American",
    },
    Speaker {
        name: "Fenrir",
        sid: 14,
        note: "American · deep",
    },
    Speaker {
        name: "Liam",
        sid: 15,
        note: "American",
    },
    Speaker {
        name: "Onyx",
        sid: 17,
        note: "American · deep",
    },
    Speaker {
        name: "Puck",
        sid: 18,
        note: "American · lively",
    },
    Speaker {
        name: "Santa",
        sid: 19,
        note: "American · character",
    },
    Speaker {
        name: "Emma",
        sid: 21,
        note: "British · warm",
    },
    Speaker {
        name: "Alice",
        sid: 20,
        note: "British",
    },
    Speaker {
        name: "Isabella",
        sid: 22,
        note: "British",
    },
    Speaker {
        name: "Lily",
        sid: 23,
        note: "British · light",
    },
    Speaker {
        name: "George",
        sid: 26,
        note: "British · steady",
    },
    Speaker {
        name: "Daniel",
        sid: 24,
        note: "British",
    },
    Speaker {
        name: "Fable",
        sid: 25,
        note: "British · storytelling",
    },
    Speaker {
        name: "Lewis",
        sid: 27,
        note: "British · deep",
    },
];

/// A single-speaker voice file: its speaker id is always zero.
const ONE: &[Speaker] = &[Speaker {
    name: "",
    sid: 0,
    note: "",
}];

/// Kitten's eight expressive voices, read from the model's own metadata.
const KITTEN: &[Speaker] = &[
    Speaker {
        name: "Voice 2 male",
        sid: 0,
        note: "",
    },
    Speaker {
        name: "Voice 2 female",
        sid: 1,
        note: "",
    },
    Speaker {
        name: "Voice 3 male",
        sid: 2,
        note: "",
    },
    Speaker {
        name: "Voice 3 female",
        sid: 3,
        note: "",
    },
    Speaker {
        name: "Voice 4 male",
        sid: 4,
        note: "",
    },
    Speaker {
        name: "Voice 4 female",
        sid: 5,
        note: "",
    },
    Speaker {
        name: "Voice 5 male",
        sid: 6,
        note: "",
    },
    Speaker {
        name: "Voice 5 female",
        sid: 7,
        note: "",
    },
];

/// VCTK's 109 speakers. The corpus identifies them only by number, so there is
/// nothing more descriptive to call them -- which is what the audition button in
/// the voice picker is for.
const VCTK: &[Speaker] = &[
    Speaker {
        name: "VCTK 1",
        sid: 0,
        note: "",
    },
    Speaker {
        name: "VCTK 2",
        sid: 1,
        note: "",
    },
    Speaker {
        name: "VCTK 3",
        sid: 2,
        note: "",
    },
    Speaker {
        name: "VCTK 4",
        sid: 3,
        note: "",
    },
    Speaker {
        name: "VCTK 5",
        sid: 4,
        note: "",
    },
    Speaker {
        name: "VCTK 6",
        sid: 5,
        note: "",
    },
    Speaker {
        name: "VCTK 7",
        sid: 6,
        note: "",
    },
    Speaker {
        name: "VCTK 8",
        sid: 7,
        note: "",
    },
    Speaker {
        name: "VCTK 9",
        sid: 8,
        note: "",
    },
    Speaker {
        name: "VCTK 10",
        sid: 9,
        note: "",
    },
    Speaker {
        name: "VCTK 11",
        sid: 10,
        note: "",
    },
    Speaker {
        name: "VCTK 12",
        sid: 11,
        note: "",
    },
    Speaker {
        name: "VCTK 13",
        sid: 12,
        note: "",
    },
    Speaker {
        name: "VCTK 14",
        sid: 13,
        note: "",
    },
    Speaker {
        name: "VCTK 15",
        sid: 14,
        note: "",
    },
    Speaker {
        name: "VCTK 16",
        sid: 15,
        note: "",
    },
    Speaker {
        name: "VCTK 17",
        sid: 16,
        note: "",
    },
    Speaker {
        name: "VCTK 18",
        sid: 17,
        note: "",
    },
    Speaker {
        name: "VCTK 19",
        sid: 18,
        note: "",
    },
    Speaker {
        name: "VCTK 20",
        sid: 19,
        note: "",
    },
    Speaker {
        name: "VCTK 21",
        sid: 20,
        note: "",
    },
    Speaker {
        name: "VCTK 22",
        sid: 21,
        note: "",
    },
    Speaker {
        name: "VCTK 23",
        sid: 22,
        note: "",
    },
    Speaker {
        name: "VCTK 24",
        sid: 23,
        note: "",
    },
    Speaker {
        name: "VCTK 25",
        sid: 24,
        note: "",
    },
    Speaker {
        name: "VCTK 26",
        sid: 25,
        note: "",
    },
    Speaker {
        name: "VCTK 27",
        sid: 26,
        note: "",
    },
    Speaker {
        name: "VCTK 28",
        sid: 27,
        note: "",
    },
    Speaker {
        name: "VCTK 29",
        sid: 28,
        note: "",
    },
    Speaker {
        name: "VCTK 30",
        sid: 29,
        note: "",
    },
    Speaker {
        name: "VCTK 31",
        sid: 30,
        note: "",
    },
    Speaker {
        name: "VCTK 32",
        sid: 31,
        note: "",
    },
    Speaker {
        name: "VCTK 33",
        sid: 32,
        note: "",
    },
    Speaker {
        name: "VCTK 34",
        sid: 33,
        note: "",
    },
    Speaker {
        name: "VCTK 35",
        sid: 34,
        note: "",
    },
    Speaker {
        name: "VCTK 36",
        sid: 35,
        note: "",
    },
    Speaker {
        name: "VCTK 37",
        sid: 36,
        note: "",
    },
    Speaker {
        name: "VCTK 38",
        sid: 37,
        note: "",
    },
    Speaker {
        name: "VCTK 39",
        sid: 38,
        note: "",
    },
    Speaker {
        name: "VCTK 40",
        sid: 39,
        note: "",
    },
    Speaker {
        name: "VCTK 41",
        sid: 40,
        note: "",
    },
    Speaker {
        name: "VCTK 42",
        sid: 41,
        note: "",
    },
    Speaker {
        name: "VCTK 43",
        sid: 42,
        note: "",
    },
    Speaker {
        name: "VCTK 44",
        sid: 43,
        note: "",
    },
    Speaker {
        name: "VCTK 45",
        sid: 44,
        note: "",
    },
    Speaker {
        name: "VCTK 46",
        sid: 45,
        note: "",
    },
    Speaker {
        name: "VCTK 47",
        sid: 46,
        note: "",
    },
    Speaker {
        name: "VCTK 48",
        sid: 47,
        note: "",
    },
    Speaker {
        name: "VCTK 49",
        sid: 48,
        note: "",
    },
    Speaker {
        name: "VCTK 50",
        sid: 49,
        note: "",
    },
    Speaker {
        name: "VCTK 51",
        sid: 50,
        note: "",
    },
    Speaker {
        name: "VCTK 52",
        sid: 51,
        note: "",
    },
    Speaker {
        name: "VCTK 53",
        sid: 52,
        note: "",
    },
    Speaker {
        name: "VCTK 54",
        sid: 53,
        note: "",
    },
    Speaker {
        name: "VCTK 55",
        sid: 54,
        note: "",
    },
    Speaker {
        name: "VCTK 56",
        sid: 55,
        note: "",
    },
    Speaker {
        name: "VCTK 57",
        sid: 56,
        note: "",
    },
    Speaker {
        name: "VCTK 58",
        sid: 57,
        note: "",
    },
    Speaker {
        name: "VCTK 59",
        sid: 58,
        note: "",
    },
    Speaker {
        name: "VCTK 60",
        sid: 59,
        note: "",
    },
    Speaker {
        name: "VCTK 61",
        sid: 60,
        note: "",
    },
    Speaker {
        name: "VCTK 62",
        sid: 61,
        note: "",
    },
    Speaker {
        name: "VCTK 63",
        sid: 62,
        note: "",
    },
    Speaker {
        name: "VCTK 64",
        sid: 63,
        note: "",
    },
    Speaker {
        name: "VCTK 65",
        sid: 64,
        note: "",
    },
    Speaker {
        name: "VCTK 66",
        sid: 65,
        note: "",
    },
    Speaker {
        name: "VCTK 67",
        sid: 66,
        note: "",
    },
    Speaker {
        name: "VCTK 68",
        sid: 67,
        note: "",
    },
    Speaker {
        name: "VCTK 69",
        sid: 68,
        note: "",
    },
    Speaker {
        name: "VCTK 70",
        sid: 69,
        note: "",
    },
    Speaker {
        name: "VCTK 71",
        sid: 70,
        note: "",
    },
    Speaker {
        name: "VCTK 72",
        sid: 71,
        note: "",
    },
    Speaker {
        name: "VCTK 73",
        sid: 72,
        note: "",
    },
    Speaker {
        name: "VCTK 74",
        sid: 73,
        note: "",
    },
    Speaker {
        name: "VCTK 75",
        sid: 74,
        note: "",
    },
    Speaker {
        name: "VCTK 76",
        sid: 75,
        note: "",
    },
    Speaker {
        name: "VCTK 77",
        sid: 76,
        note: "",
    },
    Speaker {
        name: "VCTK 78",
        sid: 77,
        note: "",
    },
    Speaker {
        name: "VCTK 79",
        sid: 78,
        note: "",
    },
    Speaker {
        name: "VCTK 80",
        sid: 79,
        note: "",
    },
    Speaker {
        name: "VCTK 81",
        sid: 80,
        note: "",
    },
    Speaker {
        name: "VCTK 82",
        sid: 81,
        note: "",
    },
    Speaker {
        name: "VCTK 83",
        sid: 82,
        note: "",
    },
    Speaker {
        name: "VCTK 84",
        sid: 83,
        note: "",
    },
    Speaker {
        name: "VCTK 85",
        sid: 84,
        note: "",
    },
    Speaker {
        name: "VCTK 86",
        sid: 85,
        note: "",
    },
    Speaker {
        name: "VCTK 87",
        sid: 86,
        note: "",
    },
    Speaker {
        name: "VCTK 88",
        sid: 87,
        note: "",
    },
    Speaker {
        name: "VCTK 89",
        sid: 88,
        note: "",
    },
    Speaker {
        name: "VCTK 90",
        sid: 89,
        note: "",
    },
    Speaker {
        name: "VCTK 91",
        sid: 90,
        note: "",
    },
    Speaker {
        name: "VCTK 92",
        sid: 91,
        note: "",
    },
    Speaker {
        name: "VCTK 93",
        sid: 92,
        note: "",
    },
    Speaker {
        name: "VCTK 94",
        sid: 93,
        note: "",
    },
    Speaker {
        name: "VCTK 95",
        sid: 94,
        note: "",
    },
    Speaker {
        name: "VCTK 96",
        sid: 95,
        note: "",
    },
    Speaker {
        name: "VCTK 97",
        sid: 96,
        note: "",
    },
    Speaker {
        name: "VCTK 98",
        sid: 97,
        note: "",
    },
    Speaker {
        name: "VCTK 99",
        sid: 98,
        note: "",
    },
    Speaker {
        name: "VCTK 100",
        sid: 99,
        note: "",
    },
    Speaker {
        name: "VCTK 101",
        sid: 100,
        note: "",
    },
    Speaker {
        name: "VCTK 102",
        sid: 101,
        note: "",
    },
    Speaker {
        name: "VCTK 103",
        sid: 102,
        note: "",
    },
    Speaker {
        name: "VCTK 104",
        sid: 103,
        note: "",
    },
    Speaker {
        name: "VCTK 105",
        sid: 104,
        note: "",
    },
    Speaker {
        name: "VCTK 106",
        sid: 105,
        note: "",
    },
    Speaker {
        name: "VCTK 107",
        sid: 106,
        note: "",
    },
    Speaker {
        name: "VCTK 108",
        sid: 107,
        note: "",
    },
    Speaker {
        name: "VCTK 109",
        sid: 108,
        note: "",
    },
];

/// English-first. Kokoro and VCTK carry other languages, and sherpa publishes
/// hundreds more voices; what is listed here is what has been checked. Every
/// hash was verified against the live asset before being added -- a published
/// hash is not a verified hash, and the Kokoro entry was stale the first time it
/// was copied from another project's catalog. `scripts/verify-catalog.sh`
/// re-checks them all.
pub const CATALOG: &[Model] = &[
    Model {
        id: "kitten-nano-en-v0_8-int8",
        engine: Engine::Kitten,
        label: "Kitten Nano",
        accent: "8 expressive voices",
        mb: 29,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kitten-nano-en-v0_8-int8.tar.bz2",
        sha256: "6fa5be852612ce761094ba74ee6123b4fc4acfefa79bf64dc63acae4a83af2fd",
        speakers: KITTEN,
        tradeoff: "Tiny and quick, with eight voices.",
    },
    Model {
        id: "vits-piper-en_US-amy-medium-int8",
        engine: Engine::Piper,
        label: "Amy",
        accent: "American",
        mb: 21,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_US-amy-medium-int8.tar.bz2",
        sha256: "bd23c0aa629eb3719448582f45ede49e8fa6a679061fed5eab16a6a6fd8e7e82",
        speakers: ONE,
        tradeoff: "Fast and small. Ready in seconds.",
    },
    Model {
        id: "vits-piper-en_US-lessac-medium-int8",
        engine: Engine::Piper,
        label: "Lessac",
        accent: "American",
        mb: 20,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_US-lessac-medium-int8.tar.bz2",
        sha256: "f1c6d0295cf16087b05f80fdca5b44daca5cd78e2c425d419a42ba34929805f9",
        speakers: ONE,
        tradeoff: "Clear and neutral.",
    },
    Model {
        id: "vits-piper-en_US-ryan-medium-int8",
        engine: Engine::Piper,
        label: "Ryan",
        accent: "American",
        mb: 21,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_US-ryan-medium-int8.tar.bz2",
        sha256: "376eb489d42e98cb49f0e13a633e8d88580bd247ed8aacc298a733210404f771",
        speakers: ONE,
        tradeoff: "Male, conversational.",
    },
    Model {
        id: "vits-piper-en_US-kathleen-low-int8",
        engine: Engine::Piper,
        label: "Kathleen",
        accent: "American",
        mb: 21,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_US-kathleen-low-int8.tar.bz2",
        sha256: "08a1372ea3eed70b9477401c3aa378a73db1c609eece25fa10203c854cfe044a",
        speakers: ONE,
        tradeoff: "Lowest latency of the set.",
    },
    Model {
        id: "vits-piper-en_GB-alba-medium-int8",
        engine: Engine::Piper,
        label: "Alba",
        accent: "Scottish",
        mb: 21,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_GB-alba-medium-int8.tar.bz2",
        sha256: "f7581d123ae977f64f3032bb247d4deeac8440e881d918d36fdd36d8f1030fb7",
        speakers: ONE,
        tradeoff: "Scottish accent.",
    },
    Model {
        id: "vits-piper-en_GB-jenny_dioco-medium-int8",
        engine: Engine::Piper,
        label: "Jenny",
        accent: "British",
        mb: 19,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_GB-jenny_dioco-medium-int8.tar.bz2",
        sha256: "db7ed4edd3c1e28b1afdecd9a77f5cb44db326f687632ffd364be513ec6a11e6",
        speakers: ONE,
        tradeoff: "British, expressive.",
    },
    Model {
        id: "vits-piper-en_GB-northern_english_male-medium-int8",
        engine: Engine::Piper,
        label: "Northern English",
        accent: "Northern English",
        mb: 20,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_GB-northern_english_male-medium-int8.tar.bz2",
        sha256: "9b2afdc8f35426f6b2739f255e67841679a52b56d19ece874a1eae208cb88ad1",
        speakers: ONE,
        tradeoff: "Male, northern English.",
    },
    Model {
        id: "vits-piper-en_US-glados",
        engine: Engine::Piper,
        label: "GLaDOS",
        accent: "synthetic",
        mb: 64,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_US-glados.tar.bz2",
        sha256: "fa2b82a4984468081db8cebbaa7c0b1e008492ac2b4276d7f628d6141b6a2d1c",
        speakers: ONE,
        tradeoff: "For fun.",
    },
    Model {
        id: "vits-vctk",
        engine: Engine::Piper,
        label: "VCTK",
        accent: "109 English voices",
        mb: 144,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-vctk.tar.bz2",
        sha256: "4f0a02db66914b3760b144cebc004e65dd4d1aeef43379f2b058849e74002490",
        speakers: VCTK,
        tradeoff: "The widest choice of voices, one download.",
    },
    Model {
        id: "kokoro-multi-lang-v1_0",
        engine: Engine::Kokoro,
        label: "Kokoro",
        accent: "28 English voices",
        mb: 349,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-multi-lang-v1_0.tar.bz2",
        sha256: "c5f7e2d2caf082bc1d20fb70334a61d99d20b484500aad32e7cf84c128ea3298",
        speakers: KOKORO_EN,
        tradeoff: "Noticeably better. A larger download.",
    },
    Model {
        id: "kokoro-multi-lang-v1_1",
        engine: Engine::Kokoro,
        label: "Kokoro v1.1",
        accent: "28 English voices",
        mb: 347,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-multi-lang-v1_1.tar.bz2",
        sha256: "a3f4c73d043860e3fd2e5b06f36795eb81de0fc8e8de6df703245edddd87dbad",
        speakers: KOKORO_EN,
        tradeoff: "Newer Kokoro. Same size, same voices.",
    },
];

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
