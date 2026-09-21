//! The library surface: importing books, listing them, and reading one.
//!
//! The reading position for a book lives in the library, next to the book,
//! rather than in [`crate::reading`], which remains the bookmark for text that
//! was pasted in and belongs to nothing. Both are written by the same position
//! callback; which one is written depends only on whether a book is open.

use std::sync::Arc;

use lector_book::catalogue::{self, Listing};
use lector_book::{epub, pdf, text, web, Book, Shelf};
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

/// Imports a document chosen from a file dialog.
///
/// The picker is opened from Rust rather than the webview: the webview's file
/// input hands back a `File`, not a path, and the importer needs the bytes of
/// the real file to derive the document's id from them.
///
/// PDF does not import here. It goes through a preview the reader has to
/// accept first, because PDF extraction is a reconstruction and a bad one
/// narrates confident nonsense. See [`preview_pdf`].
#[tauri::command]
pub fn import_book(app: AppHandle) {
    std::thread::spawn(move || {
        let picked = app
            .dialog()
            .file()
            .add_filter("Documents", &["epub", "txt", "md", "markdown", "pdf"])
            .blocking_pick_file();

        let Some(file) = picked else {
            return; // cancelled, which is not an error
        };
        let Some(path) = file.as_path().map(|p| p.to_path_buf()) else {
            return;
        };

        // Extraction of a large PDF takes a second or two and everything else
        // is quicker, but the window has nothing to show in the meantime and a
        // button that does nothing is indistinguishable from a broken one.
        let _ = app.emit(
            "importing",
            serde_json::json!({
                "name": path.file_name().map(|n| n.to_string_lossy().to_string())
            }),
        );

        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();

        if ext == "pdf" {
            match pdf::preview(&path) {
                Ok(x) => {
                    let _ = app.emit(
                        "pdf_preview",
                        serde_json::json!({
                            "path": path.to_string_lossy(),
                            "name": path.file_stem().map(|n| n.to_string_lossy().to_string()),
                            "text": x.text,
                            "total_pages": x.total_pages,
                            "failed_pages": x.failed_pages.len(),
                        }),
                    );
                }
                Err(e) => fail(&app, e),
            }
            return;
        }

        let state = app.state::<Arc<App>>().inner().clone();
        let imported = match ext.as_str() {
            "epub" => std::fs::read(&path)
                .map_err(|e| format!("cannot read that file: {e}"))
                .and_then(|bytes| epub::from_bytes(&bytes).map(|b| (b, bytes))),
            "txt" | "md" | "markdown" => text::import(&path).and_then(|b| {
                std::fs::read(&path)
                    .map(|raw| (b, raw))
                    .map_err(|e| e.to_string())
            }),
            other => Err(format!("Lector cannot read .{other} files")),
        };

        match imported.and_then(|(book, bytes)| state.library.add(&book, &bytes)) {
            Ok(row) => {
                let _ = app.emit("imported", &row);
            }
            Err(e) => fail(&app, e),
        }
    });
}

/// Adds a PDF whose extraction has been shown and accepted.
///
/// The text comes back from the window rather than being re-extracted, so what
/// is stored is exactly what was reviewed.
#[tauri::command]
pub fn accept_pdf(app: AppHandle, path: String, text: String) {
    std::thread::spawn(move || {
        let state = app.state::<Arc<App>>().inner().clone();
        let p = std::path::PathBuf::from(&path);
        let result = pdf::import(&p, &text)
            .and_then(|book| {
                std::fs::read(&p)
                    .map(|raw| (book, raw))
                    .map_err(|e| e.to_string())
            })
            .and_then(|(book, raw)| state.library.add(&book, &raw));
        match result {
            Ok(row) => {
                let _ = app.emit("imported", &row);
            }
            Err(e) => fail(&app, e),
        }
    });
}

/// Imports a web article.
///
/// One of only two things in Lector that reach the network, the other being
/// model downloads. It fetches in; nothing about the document goes out.
#[tauri::command]
pub fn import_url(app: AppHandle, url: String) {
    std::thread::spawn(move || {
        let state = app.state::<Arc<App>>().inner().clone();
        let result = web::import(url.trim()).and_then(|book: Book| {
            // A page has no file to keep, so the article's own text is what is
            // stored beside it -- enough to re-import it without the network.
            let source = serde_json::to_vec(&book).unwrap_or_default();
            state.library.add(&book, &source)
        });
        match result {
            Ok(row) => {
                let _ = app.emit("imported", &row);
            }
            Err(e) => fail(&app, e),
        }
    });
}

/// Searches the free catalogues.
///
/// An empty query is not an empty result: it is the Standard Ebooks shelf,
/// which is what a reader who has not typed anything should be looking at.
#[tauri::command]
pub fn browse(query: String) -> Result<Vec<Listing>, String> {
    if query.trim().is_empty() {
        catalogue::new_releases()
    } else {
        catalogue::search(&query)
    }
}

/// Downloads a listing and adds it to the library.
#[tauri::command]
pub fn get_book(app: AppHandle, listing: Listing) {
    std::thread::spawn(move || {
        let state = app.state::<Arc<App>>().inner().clone();
        let id = listing.id.clone();

        // Throttled to whole percents: a book arrives in 64 KB reads, and an
        // event per read floods the channel to redraw the same bar.
        let mut last = 0u64;
        let result = catalogue::fetch(&listing, |got, total| {
            let pct = (got * 100).checked_div(total).unwrap_or(0);
            if pct != last {
                last = pct;
                let _ = app.emit(
                    "progress",
                    serde_json::json!({
                        "id": id, "received": got, "total": total, "phase": "Downloading",
                    }),
                );
            }
        })
        .and_then(|(book, bytes)| state.library.add(&book, &bytes));

        match result {
            Ok(row) => {
                let _ = app.emit(
                    "progress",
                    serde_json::json!({ "id": listing.id, "phase": "Done" }),
                );
                let _ = app.emit("imported", &row);
            }
            // Reported against the listing, not against "import": the window
            // has a button waiting on this id and has to be able to find it.
            Err(e) => {
                let _ = app.emit(
                    "failed",
                    serde_json::json!({ "id": listing.id, "error": e }),
                );
            }
        }
    });
}

fn fail(app: &AppHandle, error: String) {
    let _ = app.emit(
        "failed",
        serde_json::json!({"id": "import", "error": error}),
    );
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
