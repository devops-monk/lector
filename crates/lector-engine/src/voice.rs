//! What a voice is, and where its files live.
//!
//! The catalog declares which engine a model uses; this module finds the files
//! that engine needs inside the unpacked directory. Sniffing the directory was
//! tried first and is wrong: Kitten ships a `voices.bin` exactly like Kokoro
//! does, so contents alone cannot tell the two apart.
//!
//! Layouts vary more than the archives suggest. Piper carries `espeak-ng-data`
//! and phonemizes; VCTK carries a `lexicon.txt` and does not. Kokoro carries
//! several lexicons and a jieba dictionary. So each file is looked for and
//! quietly omitted when absent, rather than assumed.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// VITS: Piper's single-speaker voices and multi-speaker sets like VCTK.
    Piper,
    Kokoro,
    Kitten,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Voice {
    pub id: String,
    pub engine: Engine,
    /// Index into the model's speaker table. Always 0 for a single-speaker file.
    pub sid: i32,
    pub model_file: String,
    pub tokens: String,
    /// espeak-ng data, for models that phonemize rather than look words up.
    pub data_dir: Option<String>,
    pub voices_bin: Option<String>,
    /// Comma-separated lexicon files. Multi-lingual Kokoro refuses to load
    /// without them, and VCTK uses one instead of espeak.
    pub lexicon: Option<String>,
    /// Jieba dictionary, for segmenting Chinese. Kokoro only.
    pub dict_dir: Option<String>,
}

fn some_dir(p: PathBuf) -> Option<String> {
    p.is_dir().then(|| p.to_string_lossy().into_owned())
}

fn some_file(p: PathBuf) -> Option<String> {
    p.is_file().then(|| p.to_string_lossy().into_owned())
}

impl Voice {
    /// Builds a voice from an unpacked model directory.
    ///
    /// `speaker` picks the pronunciation table where a model ships more than
    /// one: Kokoro's British voices want the British lexicon, and handing them
    /// the American one gives a subtly wrong accent rather than an error.
    pub fn from_dir_for(
        dir: &Path,
        engine: Engine,
        sid: i32,
        speaker: Option<&str>,
    ) -> Result<Self, String> {
        let id = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("model")
            .to_string();

        let tokens = dir.join("tokens.txt");
        if !tokens.exists() {
            return Err(format!("{}: no tokens.txt", dir.display()));
        }

        let model_file = pick_model(dir, engine)?;

        // Kokoro ships one lexicon per locale plus a Chinese one; VCTK ships a
        // single lexicon.txt. Passing the Chinese table alongside English costs
        // nothing and stops CJK input from failing outright.
        let british = speaker.is_some_and(|s| s.starts_with('b'));
        let mut lexicons: Vec<String> = Vec::new();
        for name in [
            if british {
                "lexicon-gb-en.txt"
            } else {
                "lexicon-us-en.txt"
            },
            "lexicon-zh.txt",
            "lexicon.txt",
        ] {
            if let Some(p) = some_file(dir.join(name)) {
                lexicons.push(p);
            }
        }

        Ok(Self {
            id,
            engine,
            sid,
            model_file,
            tokens: tokens.to_string_lossy().into_owned(),
            data_dir: some_dir(dir.join("espeak-ng-data")),
            voices_bin: some_file(dir.join("voices.bin")),
            lexicon: (!lexicons.is_empty()).then(|| lexicons.join(",")),
            dict_dir: some_dir(dir.join("dict")),
        })
    }

    /// Convenience for a single-speaker model whose engine is already known.
    pub fn from_dir(dir: &Path, sid: i32) -> Result<Self, String> {
        // Sniffing is only sound when Kitten is not a possibility, which is the
        // case for the development binaries that use this.
        let engine = if dir.join("voices.bin").exists() {
            Engine::Kokoro
        } else {
            Engine::Piper
        };
        Self::from_dir_for(dir, engine, sid, None)
    }
}

/// Chooses the ONNX file to load.
///
/// Some archives ship both a float and an int8 build. For VITS the int8 build is
/// both smaller and faster, so it wins. Kokoro is the opposite -- on Apple
/// silicon its int8 build is *slower* than fp32, because ARM pays a
/// dequantisation tax the smaller weights never earn back -- but its archives
/// carry only one model, so there is nothing to choose between.
fn pick_model(dir: &Path, engine: Engine) -> Result<String, String> {
    let mut onnx: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "onnx"))
        .collect();
    if onnx.is_empty() {
        return Err(format!("{}: no .onnx model", dir.display()));
    }
    onnx.sort();

    let is_int8 = |p: &PathBuf| p.to_string_lossy().contains("int8");
    let chosen = match engine {
        Engine::Piper | Engine::Kitten => onnx
            .iter()
            .find(|p| is_int8(p))
            .or_else(|| onnx.first())
            .unwrap(),
        Engine::Kokoro => onnx
            .iter()
            .find(|p| !is_int8(p))
            .or_else(|| onnx.first())
            .unwrap(),
    };
    Ok(chosen.to_string_lossy().into_owned())
}
