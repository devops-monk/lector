// No console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Lector: select text anywhere, press a key, hear it.
//!
//! There is no window. The app lives in the menu bar, and both entry points --
//! the global hotkey and the macOS Services menu -- collapse to the same call.
//! That is deliberate: a second surface should be a second *caller*, never a
//! second implementation.

mod api;
mod books;
pub mod reading;
mod selection;
#[cfg(target_os = "macos")]
mod services;
pub mod settings;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use books::Open;
use lector_book::Library;
use lector_engine::catalog::{self, Model};
use lector_engine::install::{self, Cancel, Phase};
use lector_engine::{Lector, Voice};
use lector_text::SanitizeOptions;
use reading::Bookmark;
use settings::Settings;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// Mirrors Vox's default, one modifier over. Vox takes Alt+Space to start
/// listening; Lector takes Alt+Shift+Space to start speaking.
fn hotkey() -> Shortcut {
    Shortcut::new(Some(Modifiers::ALT | Modifiers::SHIFT), Code::Space)
}

pub struct App {
    lector: Mutex<Option<Lector>>,
    /// The document currently loaded for reading, enumerated once. Shared with
    /// the engine by Arc so a seek cannot re-chunk it into something else.
    doc: Mutex<Option<Arc<Vec<String>>>>,
    /// Kept so the engine's position callback can emit to the window. The
    /// callback outlives any single command, so it cannot borrow a handle.
    app: Mutex<Option<AppHandle>>,
    /// How far into the current document the voice had read. Written from the
    /// position callback, so it is the audible position, not the synthesized one.
    pub bookmark: Bookmark,
    /// The shelf. Cheap to hold: it is a path and reads the disk on demand.
    pub library: Library,
    /// Which book and chapter is being read, if any. Decides whether the
    /// position callback writes to the library or to the bookmark.
    pub open: Mutex<Option<Open>>,
    settings: Mutex<Settings>,
    data_dir: PathBuf,
    /// Set once, immediately after construction, so callbacks can reach back
    /// into the app without an ownership cycle.
    weak: Mutex<std::sync::Weak<App>>,
    /// Model ids with a download in flight. A set rather than a flag so two
    /// concurrent downloads cannot cancel or duplicate each other.
    installing: Mutex<HashSet<String>>,
}

impl App {
    /// A weak handle to self for the position callback.
    ///
    /// Weak rather than strong because the callback lives on the cursor thread
    /// and holding an `Arc<App>` there would keep the app alive past quit.
    fn clone_for_callback(&self) -> std::sync::Weak<App> {
        self.weak.lock().unwrap().clone()
    }

    fn models_dir(&self) -> PathBuf {
        self.data_dir.join("models")
    }

    /// Where a model actually is, allowing for a checkout that already has one.
    ///
    /// The repo copy is a development convenience: `cargo run` should work
    /// without first downloading 21 MB into an app-data directory nobody has
    /// looked in.
    fn installed_dir(&self, m: &Model) -> Option<PathBuf> {
        let candidates = [
            self.models_dir().join(m.id),
            PathBuf::from("models").join(m.id),
            PathBuf::from("../models").join(m.id),
        ];
        candidates
            .into_iter()
            .find(|p| p.join("tokens.txt").exists())
    }

    /// Speaks the current selection, or stops if already speaking.
    ///
    /// The toggle is the whole interaction: one key starts, the same key stops.
    fn toggle(&self) {
        let guard = self.lector.lock().unwrap();
        let Some(lector) = guard.as_ref() else {
            eprintln!("lector: no voice installed");
            return;
        };
        if lector.is_speaking() {
            lector.stop();
            return;
        }
        drop(guard);

        match selection::selected_text() {
            Ok(text) => self.speak(&text),
            Err(e) => eprintln!("lector: {e}"),
        }
    }

    /// Speaks text handed to us directly, as the Services menu does.
    ///
    /// The hotkey path differs only in where the text comes from -- it has to go
    /// and fetch the selection itself. Everything after that is this.
    fn speak(&self, text: &str) {
        if let Some(l) = self.lector.lock().unwrap().as_ref() {
            l.speak(text, SanitizeOptions::READING, true);
        }
    }

    fn stop(&self) {
        if let Some(l) = self.lector.lock().unwrap().as_ref() {
            l.stop();
        }
    }

    /// Loads the engine for the currently chosen voice, if its files are present.
    fn load_engine(&self) {
        let s = self.settings.lock().unwrap().clone();
        let Some(m) = catalog::model(&s.model_id) else {
            return;
        };
        let Some(dir) = self.installed_dir(m) else {
            eprintln!("lector: {} is not installed", m.label);
            return;
        };
        let speaker_name = m
            .speakers
            .iter()
            .find(|sp| sp.sid == s.speaker)
            .map(|sp| sp.name);
        let voice = match Voice::from_dir_for(&dir, m.engine, s.speaker, speaker_name) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("lector: {e}");
                return;
            }
        };

        let mut guard = self.lector.lock().unwrap();
        match guard.as_mut() {
            // Already running: switching voice is a field change, not a restart.
            // The actor loads the new model lazily on the next utterance.
            Some(l) => {
                l.set_voice(voice);
                l.set_speed(s.speed);
            }
            // Start with the chosen voice rather than loading a default first.
            // First start: install the position callback. It fires when the
            // chunk being *heard* changes, which is what a highlight follows.
            None => {
                let app = self.app.lock().unwrap().clone();
                let state = self.clone_for_callback();
                match Lector::with_voice_and_position(voice, move |p| {
                    if let Some(state) = state.upgrade() {
                        // One position, two places it can belong. A book keeps
                        // its own, beside the book; anything else is the
                        // bookmark for pasted text.
                        let open = state.open.lock().unwrap().clone();
                        match open {
                            Some(o) => {
                                state.library.set_progress(&o.book, o.chapter, p.index);
                            }
                            None => state.bookmark.mark(p.index),
                        }
                    }
                    if let Some(app) = app.as_ref() {
                        let _ = app.emit(
                            "reading",
                            serde_json::json!({
                                "index": p.index,
                                "total": p.total,
                                "generation": p.generation,
                            }),
                        );
                    }
                }) {
                    Ok(mut l) => {
                        l.set_speed(s.speed);
                        *guard = Some(l);
                    }
                    Err(e) => eprintln!("lector: engine failed to start: {e}"),
                }
            }
        }
    }

    fn set_speed(&self, app: &AppHandle, speed: f32) {
        {
            let mut s = self.settings.lock().unwrap();
            s.speed = speed;
            s.save(&self.data_dir);
        }
        if let Some(l) = self.lector.lock().unwrap().as_mut() {
            l.set_speed(speed);
        }
        rebuild_menu(app);
    }

    /// Speaks a sample in a voice without selecting it.
    ///
    /// The engine reloads for the audition and again when the real voice next
    /// speaks. That is a second of work in exchange for not having to adopt a
    /// voice to hear it, which with 181 of them is the right trade.
    fn audition(&self, model_id: &str, sid: i32) {
        let Some(m) = catalog::model(model_id) else {
            return;
        };
        let Some(dir) = self.installed_dir(m) else {
            return;
        };
        let name = m.speakers.iter().find(|sp| sp.sid == sid).map(|sp| sp.name);
        let Ok(voice) = Voice::from_dir_for(&dir, m.engine, sid, name) else {
            return;
        };

        let mut guard = self.lector.lock().unwrap();
        if let Some(l) = guard.as_mut() {
            let previous = l.voice().clone();
            l.set_voice(voice);
            l.speak(catalog::AUDITION, SanitizeOptions::VERBATIM, true);
            // Put the chosen voice back, so auditioning never silently changes
            // what the hotkey will use.
            l.set_voice(previous);
        }
    }

    fn select_voice(&self, app: &AppHandle, model_id: &str, sid: i32) {
        {
            let mut s = self.settings.lock().unwrap();
            s.model_id = model_id.to_string();
            s.speaker = sid;
            s.save(&self.data_dir);
        }
        self.load_engine();
        rebuild_menu(app);
    }
}

/// Downloads a model, reporting progress through the tray tooltip.
///
/// With no window there is nowhere else to put it, and a 349 MB download with no
/// feedback is indistinguishable from a hang.
pub fn install_model(app: AppHandle, model_id: String) {
    let state = app.state::<Arc<App>>().inner().clone();
    let Some(m) = catalog::model(&model_id) else {
        return;
    };

    if !state.installing.lock().unwrap().insert(model_id.clone()) {
        return; // already downloading
    }
    let root = state.models_dir();

    std::thread::spawn(move || {
        let tray = app.tray_by_id("lector");
        let set_tip = |t: &TrayIcon, s: &str| {
            let _ = t.set_tooltip(Some(s));
        };

        let result = install::install(&root, m, &Cancel::new(), |p| {
            let _ = app.emit(
                "progress",
                serde_json::json!({
                    "id": m.id,
                    "received": p.received,
                    "total": p.total,
                    "phase": format!("{:?}", p.phase),
                }),
            );
            let Some(t) = tray.as_ref() else { return };
            match p.phase {
                Phase::Downloading if p.total > 0 => {
                    let pct = p.received * 100 / p.total;
                    set_tip(t, &format!("Lector — downloading {} {pct}%", m.label));
                }
                Phase::Downloading => set_tip(t, &format!("Lector — downloading {}", m.label)),
                Phase::Verifying => set_tip(t, &format!("Lector — verifying {}", m.label)),
                Phase::Unpacking => set_tip(t, &format!("Lector — unpacking {}", m.label)),
                Phase::Done => set_tip(t, "Lector"),
            }
        });

        state.installing.lock().unwrap().remove(&model_id);
        if let Some(t) = tray.as_ref() {
            set_tip(t, "Lector");
        }

        match result {
            // Selecting it is the point of downloading it.
            Ok(_) => {
                state.select_voice(&app, &model_id, m.speakers[0].sid);
                let _ = app.emit("changed", ());
            }
            Err(e) => {
                eprintln!("lector: {e}");
                let _ = app.emit("failed", serde_json::json!({"id": m.id, "error": e}));
                if let Some(t) = tray.as_ref() {
                    set_tip(t, &format!("Lector — {} failed to download", m.label));
                }
            }
        }
    });
}

fn main() {
    let app_state: Arc<Mutex<Option<Arc<App>>>> = Arc::new(Mutex::new(None));
    let hotkey_state = app_state.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |_app, shortcut, event| {
                    // Fire on press only; the release would toggle straight back.
                    if shortcut != &hotkey() || event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let Some(s) = hotkey_state.lock().unwrap().clone() else {
                        return;
                    };
                    // Synthesizing a keystroke from the main thread would
                    // deadlock against the event tap that delivered this.
                    std::thread::spawn(move || s.toggle());
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            api::snapshot,
            api::level,
            api::speak,
            api::chunk_preview,
            api::read_document,
            api::seek,
            api::reading_state,
            api::forget_reading,
            books::library,
            books::import_book,
            books::accept_pdf,
            books::import_url,
            books::browse,
            books::get_book,
            books::open_book,
            books::read_chapter,
            books::chapter_text,
            books::cover,
            books::remove_book,
            books::close_book,
            api::pause,
            api::resume,
            api::stop,
            api::choose_voice,
            api::set_speed,
            api::audition,
            api::install,
            api::grant_accessibility,
        ])
        .setup(move |app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| PathBuf::from("."));
            let settings = Settings::load(&data_dir);

            let state = Arc::new(App {
                lector: Mutex::new(None),
                doc: Mutex::new(None),
                app: Mutex::new(Some(app.handle().clone())),
                bookmark: Bookmark::load(&data_dir),
                library: Library::new(&data_dir),
                open: Mutex::new(None),
                weak: Mutex::new(std::sync::Weak::new()),
                settings: Mutex::new(settings),
                data_dir,
                installing: Mutex::new(HashSet::new()),
            });
            *state.weak.lock().unwrap() = Arc::downgrade(&state);
            app.manage(state.clone());
            *app_state.lock().unwrap() = Some(state.clone());

            // Loading blocks for ~1s (audio device plus model warm). Doing it
            // here rather than on first hotkey is the difference between a key
            // that responds in 470ms and one that responds in 1.3s.
            state.load_engine();

            app.global_shortcut().register(hotkey())?;
            build_tray(app.handle())?;

            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_focus();
            }

            // No windows, so the app must not quit when none are open.
            #[cfg(target_os = "macos")]
            services::register(state.clone());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building lector")
        .run(|app, event| {
            // The debounce means the last few sentences are only in memory.
            // Quitting is the one moment that is guaranteed to matter.
            if let tauri::RunEvent::Exit = event {
                if let Some(s) = app.try_state::<Arc<App>>() {
                    s.bookmark.flush();
                }
            }
        });
}

/// The speeds worth offering. Kept short: a slider in a menu is a fiddle, and
/// nobody wants eleven options.
const SPEEDS: &[(f32, &str)] = &[
    (0.8, "Slower"),
    (1.0, "Normal"),
    (1.25, "Faster"),
    (1.5, "Fast"),
    (2.0, "Very fast"),
];

/// Builds the tray menu from the catalog and what is actually on disk.
fn menu_for(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let state = app.state::<Arc<App>>();
    let chosen = state.settings.lock().unwrap().clone();
    let installing = state.installing.lock().unwrap().clone();
    let ready = state.lector.lock().unwrap().is_some();
    let speaking = state
        .lector
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|l| l.is_speaking());

    let mut items: Vec<Box<dyn tauri::menu::IsMenuItem<tauri::Wry>>> = Vec::new();

    // The first line is a status line, not an action. With no window this is the
    // only place the app can say what it is doing or why it is not working.
    if !selection::has_accessibility(false) {
        items.push(Box::new(MenuItem::with_id(
            app,
            "accessibility",
            "Grant Accessibility permission…",
            true,
            None::<&str>,
        )?));
        items.push(Box::new(PredefinedMenuItem::separator(app)?));
    } else if !ready {
        items.push(Box::new(MenuItem::with_id(
            app,
            "novoice",
            "No voice installed — choose one below",
            false,
            None::<&str>,
        )?));
        items.push(Box::new(PredefinedMenuItem::separator(app)?));
    }

    // One item that changes verb, rather than two where one is always dead.
    items.push(Box::new(MenuItem::with_id(
        app,
        "window",
        "Open Lector",
        true,
        None::<&str>,
    )?));
    items.push(Box::new(MenuItem::with_id(
        app,
        "toggle",
        if speaking {
            "Stop Speaking"
        } else {
            "Speak Selection"
        },
        ready,
        Some("Alt+Shift+Space"),
    )?));
    items.push(Box::new(PredefinedMenuItem::separator(app)?));

    let voices = Submenu::new(app, "Voice", true)?;
    for m in catalog::CATALOG {
        let installed = state.installed_dir(m).is_some();
        if installing.contains(m.id) {
            voices.append(&MenuItem::with_id(
                app,
                format!("busy:{}", m.id),
                format!("{} — downloading…", m.label),
                false,
                None::<&str>,
            )?)?;
        } else if !installed {
            voices.append(&MenuItem::with_id(
                app,
                format!("install:{}", m.id),
                format!("Download {} ({} MB)", m.label, m.mb),
                true,
                None::<&str>,
            )?)?;
        } else {
            // One row per speaker, so choosing a voice is one click rather than
            // a model choice followed by a speaker choice.
            for sp in m.speakers {
                let on = chosen.model_id == m.id && chosen.speaker == sp.sid;
                let label = if m.speakers.len() == 1 {
                    m.label.to_string()
                } else {
                    format!("{} — {}", m.label, sp.name)
                };
                voices.append(&CheckMenuItem::with_id(
                    app,
                    format!("voice:{}:{}", m.id, sp.sid),
                    label,
                    true,
                    on,
                    None::<&str>,
                )?)?;
            }
        }
    }
    items.push(Box::new(voices));

    let speed = Submenu::new(app, "Speed", ready)?;
    for (v, label) in SPEEDS {
        speed.append(&CheckMenuItem::with_id(
            app,
            format!("speed:{v}"),
            format!("{label}  ({v}x)"),
            true,
            (chosen.speed - v).abs() < 0.01,
            None::<&str>,
        )?)?;
    }
    items.push(Box::new(speed));

    items.push(Box::new(PredefinedMenuItem::separator(app)?));
    items.push(Box::new(MenuItem::with_id(
        app,
        "quit",
        "Quit Lector",
        true,
        Some("Cmd+Q"),
    )?));

    let refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> =
        items.iter().map(|b| b.as_ref()).collect();
    Menu::with_items(app, &refs)
}

/// Brings the window back. Closing it leaves Lector running in the menu bar, so
/// this is how it returns rather than by relaunching the app.
fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn rebuild_menu(app: &AppHandle) {
    if let Some(tray) = app.tray_by_id("lector") {
        if let Ok(menu) = menu_for(app) {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    // A template image -- black plus alpha -- so macOS tints it for light and
    // dark menu bars. The bundle icon is a colour icon and would look wrong here.
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/trayTemplate@2x.png"))
        .expect("tray icon");

    TrayIconBuilder::with_id("lector")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("Lector")
        .menu(&menu_for(app)?)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref().to_string();

            if let Some(model_id) = id.strip_prefix("install:") {
                install_model(app.clone(), model_id.to_string());
                rebuild_menu(app);
                return;
            }
            if let Some(rest) = id.strip_prefix("voice:") {
                if let Some((model_id, sid)) = rest.rsplit_once(':') {
                    if let Ok(sid) = sid.parse::<i32>() {
                        let (app, model_id) = (app.clone(), model_id.to_string());
                        // load_engine can block on a model load; keep the menu
                        // handler off that path.
                        std::thread::spawn(move || {
                            app.state::<Arc<App>>().select_voice(&app, &model_id, sid)
                        });
                    }
                }
                return;
            }
            if let Some(v) = id.strip_prefix("speed:") {
                if let Ok(v) = v.parse::<f32>() {
                    let app = app.clone();
                    std::thread::spawn(move || app.state::<Arc<App>>().set_speed(&app, v));
                }
                return;
            }
            match id.as_str() {
                "window" => show_window(app),
                "toggle" => {
                    let app = app.clone();
                    std::thread::spawn(move || {
                        app.state::<Arc<App>>().toggle();
                        // The item's verb depends on whether it is speaking, so
                        // the menu has to be rebuilt once the state settles.
                        rebuild_menu(&app);
                    });
                }
                "accessibility" => {
                    // Shows the system dialog, which has the button that opens
                    // the right settings pane.
                    std::thread::spawn(|| {
                        selection::has_accessibility(true);
                    });
                }
                "quit" => app.exit(0),
                _ => {}
            }
        })
        .build(app)?;
    Ok(())
}
