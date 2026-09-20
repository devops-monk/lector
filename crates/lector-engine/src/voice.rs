//! What a voice is, and where its files live.
//!
//! Which engine a directory holds is inferred from its contents rather than
//! declared: a `voices.bin` means Kokoro, its absence means a Piper VITS model.
//! The catalog therefore knows labels, sizes and URLs, and deliberately does not
//! know file layouts -- so a model whose archive is packed slightly differently
//! still loads.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Piper,
    Kokoro,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Voice {
    pub id: String,
    pub engine: Engine,
    /// Speaker index. Always 0 for Piper, which ships one speaker per file.
    pub sid: i32,
    pub model_file: String,
    pub tokens: String,
    pub data_dir: String,
    pub voices_bin: Option<String>,
    /// Comma-separated lexicon files. Multi-lingual Kokoro refuses to load
    /// without them -- it cannot guess which pronunciation table you meant.
    pub lexicon: Option<String>,
    /// Jieba dictionary, for segmenting Chinese. Present only in Kokoro.
    pub dict_dir: Option<String>,
}

impl Voice {
    /// Reads a model directory and works out what it contains.
    ///
    /// `speaker` picks the pronunciation table where a model ships more than
    /// one: Kokoro's `b*` voices are British and the rest American, and handing
    /// a British voice the US lexicon gives a subtly wrong accent rather than an
    /// error.
    pub fn from_dir_for(dir: &Path, sid: i32, speaker: Option<&str>) -> Result<Self, String> {
        let id = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("model")
            .to_string();
        let tokens = dir.join("tokens.txt");
        if !tokens.exists() {
            return Err(format!("{}: no tokens.txt", dir.display()));
        }
        let data_dir = dir.join("espeak-ng-data");
        let voices_bin = dir.join("voices.bin");

        let onnx = std::fs::read_dir(dir)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x == "onnx"))
            .ok_or_else(|| format!("{}: no .onnx model", dir.display()))?;

        let engine = if voices_bin.exists() {
            Engine::Kokoro
        } else {
            Engine::Piper
        };

        // English first, then Chinese if the model carries it -- passing the zh
        // table costs nothing for English text and stops CJK input from failing
        // outright.
        let british = speaker.is_some_and(|s| s.starts_with('b'));
        let mut lexicons: Vec<String> = Vec::new();
        for name in [
            if british {
                "lexicon-gb-en.txt"
            } else {
                "lexicon-us-en.txt"
            },
            "lexicon-zh.txt",
        ] {
            let p = dir.join(name);
            if p.exists() {
                lexicons.push(p.to_string_lossy().into_owned());
            }
        }
        let dict = dir.join("dict");

        Ok(Self {
            id,
            engine,
            sid: if engine == Engine::Piper { 0 } else { sid },
            model_file: onnx.to_string_lossy().into_owned(),
            tokens: tokens.to_string_lossy().into_owned(),
            data_dir: data_dir.to_string_lossy().into_owned(),
            voices_bin: voices_bin
                .exists()
                .then(|| voices_bin.to_string_lossy().into_owned()),
            lexicon: (!lexicons.is_empty()).then(|| lexicons.join(",")),
            dict_dir: dict.is_dir().then(|| dict.to_string_lossy().into_owned()),
        })
    }

    /// Convenience for models with a single speaker, where there is nothing to
    /// choose between pronunciation tables.
    pub fn from_dir(dir: &Path, sid: i32) -> Result<Self, String> {
        Self::from_dir_for(dir, sid, None)
    }
}
