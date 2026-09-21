//! The library surface: importing books, listing them, and reading one.
//!
//! The reading position for a book lives in the library, next to the book,
//! rather than in [`crate::reading`], which remains the bookmark for text that
//! was pasted in and belongs to nothing. Both are written by the same position
//! callback; which one is written depends only on whether a book is open.

use std::sync::Arc;

use lector_book::{epub, Shelf};
use lector_text::SanitizeOptions;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::App;

/// What the window shows when a book is opened: its chapters by name, and
/// where the voice had got to.
#[derive(Serialize, Clone)]
pub struct BookView {
    pub id: String,
    pub title: String,
    pub author: String,
    pub chapters: Vec<String>,
    pub chapter: usize,
    pub unit: usize,
    pub warnings: usize,
}

/// Which book and chapter is open, so the position callback knows where to
/// record progress.
#[derive(Clone, Debug, Default)]
pub struct Open {
    pub book: String,
    pub chapter: usize,
}

#[tauri::command]
pub fn library(state: State<Arc<App>>) -> Vec<Shelf> {
    state.library.shelf()
}

/// Imports a book chosen from a file dialog.
///
/// The picker is opened from Rust rather than the webview: the webview's file
/// input hands back a `File`, not a path, and the importer needs the bytes of
/// the real file to derive the book's id from them.
#[tauri::command]
pub fn import_book(app: AppHandle) {
    std::thread::spawn(move || {
        let Some(file) = app
            .dialog()
            .file()
            .add_filter("Ebooks", &["epub"])
            .blocking_pick_file()
        else {
            return; // cancelled, which is not an error
        };
        let Some(path) = file.as_path().map(|p| p.to_path_buf()) else {
            return;
        };

        let state = app.state::<Arc<App>>().inner().clone();
        let _ = app.emit("importing", serde_json::json!({ "name": path.file_name().map(|n| n.to_string_lossy().to_string()) }));

        let result = std::fs::read(&path)
            .map_err(|e| format!("cannot read that file: {e}"))
            .and_then(|bytes| epub::from_bytes(&bytes).map(|b| (b, bytes)))
            .and_then(|(book, bytes)| state.library.add(&book, &bytes));

        match result {
            Ok(row) => {
                let _ = app.emit("imported", &row);
            }
            Err(e) => {
                let _ = app.emit("failed", serde_json::json!({"id": "import", "error": e}));
            }
        }
    });
}

/// Opens a book for reading. Does not start the voice.
#[tauri::command]
pub fn open_book(id: String, state: State<Arc<App>>) -> Option<BookView> {
    let book = state.library.get(&id)?;
    state.library.touch(&id);
    let p = state.library.progress(&id);
    Some(BookView {
        chapters: book.chapters.iter().map(|c| c.title.clone()).collect(),
        chapter: p.chapter.min(book.chapters.len().saturating_sub(1)),
        unit: p.unit,
        id: book.id,
        title: book.title,
        author: book.author,
        warnings: book.warnings,
    })
}

/// Enumerates one chapter and starts reading it from `from`.
///
/// The same single-enumeration rule as pasted text: the chunks handed to the
/// engine are the chunks returned to the window, so a seek cannot re-chunk the
/// chapter into something the highlight no longer matches.
#[tauri::command]
pub fn read_chapter(
    id: String,
    chapter: usize,
    from: usize,
    state: State<Arc<App>>,
) -> Vec<String> {
    let Some(book) = state.library.get(&id) else {
        return Vec::new();
    };
    let Some(ch) = book.chapters.get(chapter) else {
        return Vec::new();
    };
    let chunks = Arc::new(lector_text::prepare(
        &ch.text,
        SanitizeOptions::READING,
        false,
    ));
    *state.doc.lock().unwrap() = Some(chunks.clone());
    *state.open.lock().unwrap() = Some(Open {
        book: id.clone(),
        chapter,
    });
    // A book has its own position store, so the pasted-text bookmark must not
    // also claim this reading -- two offers to resume would appear next launch.
    state.bookmark.clear();
    state.library.set_progress(&id, chapter, from);

    let from = from.min(chunks.len().saturating_sub(1));
    if let Some(l) = state.lector.lock().unwrap().as_ref() {
        l.read(chunks.clone(), from);
    }
    (*chunks).clone()
}

/// The chapter's chunks without reading it, for showing a chapter before
/// pressing play.
#[tauri::command]
pub fn chapter_text(id: String, chapter: usize, state: State<Arc<App>>) -> Vec<String> {
    let Some(book) = state.library.get(&id) else {
        return Vec::new();
    };
    book.chapters
        .get(chapter)
        .map(|ch| lector_text::prepare(&ch.text, SanitizeOptions::READING, false))
        .unwrap_or_default()
}

/// A book's cover as a data URL, or nothing if it has none.
///
/// Fetched per book rather than carried on the shelf: covers are tens of
/// kilobytes each and the shelf is re-read on every change.
#[tauri::command]
pub fn cover(id: String, state: State<Arc<App>>) -> Option<String> {
    let row = state.library.shelf().into_iter().find(|r| r.id == id)?;
    let name = row.cover?;
    let bytes = std::fs::read(state.library.dir(&id).join(&name)).ok()?;
    let mime = match name.rsplit('.').next() {
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "image/jpeg",
    };
    Some(format!("data:{mime};base64,{}", base64(&bytes)))
}

#[tauri::command]
pub fn remove_book(app: AppHandle, id: String) -> Result<(), String> {
    let state = app.state::<Arc<App>>().inner().clone();
    if state
        .open
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|o| o.book == id)
    {
        state.stop();
        *state.open.lock().unwrap() = None;
    }
    state.library.remove(&id)?;
    let _ = app.emit("library", ());
    Ok(())
}

/// Leaves the book, so the next pasted text is not recorded against it.
#[tauri::command]
pub fn close_book(state: State<Arc<App>>) {
    *state.open.lock().unwrap() = None;
}

/// Base64 for the cover data URL.
///
/// Written out rather than pulled in: it is eleven lines, it is the only use in
/// the app, and a dependency here would be a dependency in every build.
fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        out.push(A[(n >> 18 & 63) as usize] as char);
        out.push(A[(n >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 {
            A[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            A[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_the_spec_examples() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64(b"foobar"), "Zm9vYmFy");
        // A byte over 127, which is where a sloppy implementation sign-extends.
        assert_eq!(super::base64(&[0xff, 0x00, 0xff]), "/wD/");
    }
}
