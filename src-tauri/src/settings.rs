//! What Lector remembers between launches.
//!
//! Deliberately tiny and deliberately not the model store: settings record
//! *which* voice is chosen, never whether it exists. Whether a model is on disk
//! is a question for the disk, so a hand-deleted directory falls through to the
//! default rather than leaving the app pointing at nothing.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub model_id: String,
    pub speaker: i32,
    pub speed: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model_id: lector_engine::catalog::DEFAULT_MODEL.to_string(),
            speaker: 0,
            speed: 1.0,
        }
    }
}

impl Settings {
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(dir))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Written via a temporary file and renamed, so an interrupted write cannot
    /// leave unparseable JSON that resets every preference on next launch.
    pub fn save(&self, dir: &Path) {
        let _ = std::fs::create_dir_all(dir);
        let tmp = dir.join("settings.json.tmp");
        let Ok(text) = serde_json::to_string_pretty(self) else {
            return;
        };
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, Self::path(dir));
        }
    }

    fn path(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }
}
