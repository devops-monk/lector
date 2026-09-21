//! The commands and events the window talks to.
//!
//! Audio never crosses this boundary -- the engine plays it natively. What the
//! window gets is the catalog, what is installed, what is downloading, and
//! whether Lector is currently speaking.

use std::sync::Arc;

use lector_engine::catalog;
use lector_text::SanitizeOptions;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::App;

#[derive(Serialize, Clone)]
pub struct SpeakerInfo {
    pub sid: i32,
    pub name: String,
    pub note: String,
}

#[derive(Serialize, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub accent: String,
    pub engine: String,
    pub mb: u32,
    pub tradeoff: String,
    pub installed: bool,
    pub installing: bool,
    pub speakers: Vec<SpeakerInfo>,
}

#[derive(Serialize, Clone)]
pub struct Snapshot {
    pub models: Vec<ModelInfo>,
    pub model_id: String,
    pub speaker: i32,
    pub speed: f32,
    pub ready: bool,
    pub speaking: bool,
    pub paused: bool,
    pub accessibility: bool,
    pub hotkey: String,
    pub voice_count: usize,
}

/// Everything the window needs to draw itself, in one call.
///
/// One round trip rather than six: the window is rebuilt from scratch on every
/// change, so a partial snapshot would be a source of inconsistency for no gain.
#[tauri::command]
pub fn snapshot(state: State<Arc<App>>) -> Snapshot {
    let s = state.settings.lock().unwrap().clone();
    let installing = state.installing.lock().unwrap().clone();
    let (ready, speaking, paused) = {
        let l = state.lector.lock().unwrap();
        (
            l.is_some(),
            l.as_ref().is_some_and(|l| l.is_speaking()),
            l.as_ref().is_some_and(|l| l.is_paused()),
        )
    };

    let models: Vec<ModelInfo> = catalog::CATALOG
        .iter()
        .map(|m| ModelInfo {
            id: m.id.to_string(),
            label: m.label.to_string(),
            accent: m.accent.to_string(),
            engine: format!("{:?}", m.engine),
            mb: m.mb,
            tradeoff: m.tradeoff.to_string(),
            installed: state.installed_dir(m).is_some(),
            installing: installing.contains(m.id),
            speakers: m
                .speakers
                .iter()
                .map(|sp| SpeakerInfo {
                    sid: sp.sid,
                    name: sp.name.to_string(),
                    note: sp.note.to_string(),
                })
                .collect(),
        })
        .collect();

    Snapshot {
        voice_count: models.iter().map(|m| m.speakers.len()).sum(),
        models,
        model_id: s.model_id,
        speaker: s.speaker,
        speed: s.speed,
        ready,
        speaking,
        paused,
        accessibility: crate::selection::has_accessibility(false),
        hotkey: "⌥⇧Space".into(),
    }
}

/// Current output level, 0.0..=1.0.
///
/// Separate from `snapshot` because it is polled many times a second while
/// speaking, and the snapshot walks the whole catalog.
#[tauri::command]
pub fn level(state: State<Arc<App>>) -> f32 {
    state
        .lector
        .lock()
        .unwrap()
        .as_ref()
        .map_or(0.0, |l| l.level())
}

#[tauri::command]
pub fn speak(text: String, state: State<Arc<App>>) {
    state.speak(&text);
}

#[tauri::command]
pub fn stop(state: State<Arc<App>>) {
    state.stop();
}

/// The chunks the engine will produce for this text.
///
/// The window must never split text itself. Two splitters would be two sources
/// of truth, and the moment they disagreed the highlight would land on the
/// wrong words -- a bug that looks like a timing problem and is not one. This
/// calls exactly what the speak path calls, with the same options.
#[tauri::command]
pub fn chunk_preview(text: String) -> Vec<String> {
    lector_text::prepare(&text, SanitizeOptions::READING, true)
}

/// Enumerates a document and starts reading it from `from`.
///
/// The chunking happens once, here, and both the engine and the returned list
/// come from that single enumeration.
#[tauri::command]
pub fn read_document(text: String, from: usize, state: State<Arc<App>>) -> Vec<String> {
    // false, deliberately: the fast-first-chunk rule shifts boundaries, so a
    // position saved under it would not survive a reopen.
    let chunks = Arc::new(lector_text::prepare(&text, SanitizeOptions::READING, false));
    *state.doc.lock().unwrap() = Some(chunks.clone());
    state.bookmark.begin(&text, from, chunks.len());
    if let Some(l) = state.lector.lock().unwrap().as_ref() {
        l.read(chunks.clone(), from);
    }
    (*chunks).clone()
}

/// Where the last session stopped, if it stopped in the middle of something.
///
/// Returned rather than acted on: restoring a position must never start the
/// voice. An app that begins talking because it was launched is a fright, and
/// the one thing a reader cannot undo is having been heard.
#[tauri::command]
pub fn reading_state(state: State<Arc<App>>) -> Option<crate::reading::Reading> {
    state.bookmark.get().filter(|r| r.index > 0)
}

/// Forgets the saved position. The window offers this next to Resume, because
/// a bookmark that cannot be dismissed is a nag.
#[tauri::command]
pub fn forget_reading(state: State<Arc<App>>) {
    state.bookmark.clear();
}

/// Jumps to a unit in the document already loaded.
#[tauri::command]
pub fn seek(to: usize, state: State<Arc<App>>) {
    let doc = state.doc.lock().unwrap().clone();
    if let (Some(doc), Some(l)) = (doc, state.lector.lock().unwrap().as_ref()) {
        if to < doc.len() {
            state.bookmark.mark(to);
            l.read(doc, to);
        }
    }
}

#[tauri::command]
pub fn pause(state: State<Arc<App>>) {
    if let Some(l) = state.lector.lock().unwrap().as_ref() {
        l.pause();
    }
}

#[tauri::command]
pub fn resume(state: State<Arc<App>>) {
    if let Some(l) = state.lector.lock().unwrap().as_ref() {
        l.resume();
    }
}

#[tauri::command]
pub fn choose_voice(app: AppHandle, model_id: String, sid: i32) {
    std::thread::spawn(move || {
        app.state::<Arc<App>>().select_voice(&app, &model_id, sid);
        let _ = app.emit("changed", ());
    });
}

#[tauri::command]
pub fn set_speed(app: AppHandle, speed: f32) {
    std::thread::spawn(move || {
        app.state::<Arc<App>>().set_speed(&app, speed);
        let _ = app.emit("changed", ());
    });
}

/// Speaks a sample in a voice without selecting it.
///
/// No voice should have to be adopted to be heard, and with 181 of them that
/// matters more than it would with two.
#[tauri::command]
pub fn audition(app: AppHandle, model_id: String, sid: i32) {
    std::thread::spawn(move || {
        let state = app.state::<Arc<App>>().inner().clone();
        state.audition(&model_id, sid);
    });
}

#[tauri::command]
pub fn install(app: AppHandle, model_id: String) {
    crate::install_model(app, model_id);
}

#[tauri::command]
pub fn grant_accessibility() {
    std::thread::spawn(|| {
        crate::selection::has_accessibility(true);
    });
}
