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
}

impl Voice {
    /// Reads a model directory and works out what it contains.
    pub fn from_dir(dir: &Path, sid: i32) -> Result<Self, String> {
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
        })
    }
}
