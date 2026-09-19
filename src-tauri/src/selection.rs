//! Reading the text the user has selected, in whatever app they are in.
//!
//! macOS offers no way to ask another application for its selection without
//! Accessibility access, so this does what every tool in this space does: save
//! the clipboard, synthesize the copy keystroke, read what landed, and put the
//! old clipboard back. The restore is the part people skip, and skipping it
//! means the app silently eats whatever the user had copied.

use std::time::Duration;

use arboard::Clipboard;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

/// How long to wait for the frontmost app to service the copy. Too short and we
/// read the previous clipboard contents and speak the wrong thing.
const COPY_SETTLE: Duration = Duration::from_millis(120);

/// Whether macOS will let us synthesize the copy keystroke.
///
/// Without this the hotkey fails silently, which reads as "the app is broken"
/// rather than "the app needs permission". Passing `prompt` shows the system
/// dialog with the button that opens the right settings pane.
#[cfg(target_os = "macos")]
pub fn has_accessibility(prompt: bool) -> bool {
    use std::ffi::c_void;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
        static kAXTrustedCheckOptionPrompt: *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFBooleanTrue: *const c_void;
        fn CFDictionaryCreate(
            allocator: *const c_void,
            keys: *const *const c_void,
            values: *const *const c_void,
            num_values: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> *const c_void;
        fn CFRelease(cf: *const c_void);
    }

    unsafe {
        if !prompt {
            return AXIsProcessTrusted();
        }
        // Null callbacks are fine: both entries are process-lifetime constants,
        // so there is nothing for the dictionary to retain or release.
        let keys = [kAXTrustedCheckOptionPrompt];
        let values = [kCFBooleanTrue];
        let options = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            std::ptr::null(),
            std::ptr::null(),
        );
        let trusted = AXIsProcessTrustedWithOptions(options);
        if !options.is_null() {
            CFRelease(options);
        }
        trusted
    }
}

#[cfg(not(target_os = "macos"))]
pub fn has_accessibility(_prompt: bool) -> bool {
    true
}

pub fn selected_text() -> Result<String, String> {
    if !has_accessibility(false) {
        // Prompting here, at the moment the user actually pressed the key, is
        // the only time the request makes sense to them.
        has_accessibility(true);
        return Err("Lector needs Accessibility permission to read the selection".into());
    }

    let mut clipboard = Clipboard::new().map_err(|e| e.to_string())?;
    let saved = clipboard.get_text().ok();

    // A distinctive sentinel lets us tell "the app copied nothing" apart from
    // "the app copied the same text that was already there".
    let sentinel = format!("\u{0}lector-{}", std::process::id());
    let _ = clipboard.set_text(sentinel.clone());

    press_copy()?;
    std::thread::sleep(COPY_SETTLE);

    let got = clipboard.get_text().unwrap_or_default();

    // Restore before returning, including on the failure paths below.
    match &saved {
        Some(prev) => {
            let _ = clipboard.set_text(prev.clone());
        }
        // There was nothing to restore, but our sentinel must not be left behind.
        None => {
            let _ = clipboard.clear();
        }
    }

    if got == sentinel || got.trim().is_empty() {
        return Err("no text selected".into());
    }
    Ok(got)
}

#[cfg(target_os = "macos")]
fn press_copy() -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("{e}. Lector needs Accessibility permission."))?;
    enigo.key(Key::Meta, Direction::Press).map_err(|e| e.to_string())?;
    enigo.key(Key::Unicode('c'), Direction::Click).map_err(|e| e.to_string())?;
    enigo.key(Key::Meta, Direction::Release).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn press_copy() -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| e.to_string())?;
    enigo.key(Key::Control, Direction::Press).map_err(|e| e.to_string())?;
    enigo.key(Key::Unicode('c'), Direction::Click).map_err(|e| e.to_string())?;
    enigo.key(Key::Control, Direction::Release).map_err(|e| e.to_string())?;
    Ok(())
}
