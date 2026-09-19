// No console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Lector: select text anywhere, press a key, hear it.
//!
//! There is no window. The app lives in the menu bar, and both entry points --
//! the global hotkey and the macOS Services menu -- collapse to the same call.
//! That is deliberate: a second surface should be a second *caller*, never a
//! second implementation.

mod selection;
#[cfg(target_os = "macos")]
mod services;

use std::sync::{Arc, Mutex};

use lector_engine::Lector;
use lector_text::SanitizeOptions;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Manager, State};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// Mirrors Vox's default, one key over. Vox takes Alt+Space to start listening;
/// Lector takes Alt+Shift+Space to start speaking.
fn hotkey() -> Shortcut {
    Shortcut::new(Some(Modifiers::ALT | Modifiers::SHIFT), Code::Space)
}

pub struct App {
    lector: Mutex<Option<Lector>>,
}

impl App {
    /// Speaks the current selection, or stops if already speaking.
    ///
    /// The toggle is the whole interaction: one key starts, the same key stops.
    fn toggle(&self) {
        let guard = self.lector.lock().unwrap();
        let Some(lector) = guard.as_ref() else {
            eprintln!("lector: engine unavailable");
            return;
        };

        if lector.is_speaking() {
            lector.stop();
            return;
        }

        match selection::selected_text() {
            Ok(text) => lector.speak(&text, SanitizeOptions::READING, true),
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
}

/// Finds the voice to load.
///
/// Phase 1 ships one bundled Piper voice, so this is deliberately simple: look
/// beside the executable first (how it will ship), then in the repo (how it runs
/// in development). The catalog and downloader arrive in phase 2.
fn model_dir(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    const VOICE: &str = "vits-piper-en_US-amy-medium-int8";

    let mut candidates = Vec::new();
    if let Ok(dir) = app.path().resource_dir() {
        candidates.push(dir.join("models").join(VOICE));
    }
    candidates.push(std::path::PathBuf::from("models").join(VOICE));
    candidates.push(std::path::PathBuf::from("../models").join(VOICE));

    candidates.into_iter().find(|p| p.join("tokens.txt").exists())
}

#[tauri::command]
fn speak_text(text: String, state: State<Arc<App>>) {
    if let Some(l) = state.lector.lock().unwrap().as_ref() {
        l.speak(&text, SanitizeOptions::READING, true);
    }
}

#[tauri::command]
fn stop_speaking(state: State<Arc<App>>) {
    state.stop();
}

fn main() {
    let app_state = Arc::new(App { lector: Mutex::new(None) });
    let hotkey_state = app_state.clone();

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |_app, shortcut, event| {
                    // Fire on press only; the release event would toggle straight back.
                    if shortcut == &hotkey() && event.state() == ShortcutState::Pressed {
                        let s = hotkey_state.clone();
                        // The handler runs on the main thread and synthesizing a
                        // keystroke from it would deadlock against the event tap.
                        std::thread::spawn(move || s.toggle());
                    }
                })
                .build(),
        )
        .manage(app_state.clone())
        .invoke_handler(tauri::generate_handler![speak_text, stop_speaking])
        .setup(move |app| {
            // Loading blocks for ~1s (audio device + model warm). Doing it here
            // rather than on first hotkey means the first press is instant.
            match model_dir(&app.handle()) {
                Some(dir) => match Lector::new(&dir) {
                    Ok(l) => *app_state.lector.lock().unwrap() = Some(l),
                    Err(e) => eprintln!("lector: engine failed to start: {e}"),
                },
                None => eprintln!("lector: no voice found; expected models/vits-piper-en_US-amy-medium-int8"),
            }

            app.global_shortcut().register(hotkey())?;
            build_tray(app.handle())?;

            // No windows, so the app must not quit when none are open.
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                services::register(app_state.clone());
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running lector");
}

fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let speak = MenuItem::with_id(app, "speak", "Speak Selection", true, Some("Alt+Shift+Space"))?;
    let stop = MenuItem::with_id(app, "stop", "Stop", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Lector", true, Some("Cmd+Q"))?;
    let menu = Menu::with_items(
        app,
        &[&speak, &stop, &PredefinedMenuItem::separator(app)?, &quit],
    )?;

    TrayIconBuilder::with_id("lector")
        .icon(app.default_window_icon().unwrap().clone())
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let state = app.state::<Arc<App>>();
            match event.id().as_ref() {
                "speak" => {
                    let s = state.inner().clone();
                    std::thread::spawn(move || s.toggle());
                }
                "stop" => state.stop(),
                "quit" => app.exit(0),
                _ => {}
            }
        })
        .build(app)?;
    Ok(())
}
