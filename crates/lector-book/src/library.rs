//! Where imported books live.
//!
//! Files and JSON, no database. readest runs a whole product on this shape and
//! needs nothing more, and a database would be a second thing that can be out
//! of sync with the directory that actually holds the books.
//!
//! ```text
//! <data>/books/library.json        the shelf
//! <data>/books/<id>/book.json      the parsed chapters
//! <data>/books/<id>/source.epub    the file as imported
//! <data>/books/<id>/cover.<ext>
//! <data>/books/<id>/progress.json  { chapter, unit, updated }
//! ```
//!
//! The shelf is a cache of what the directories contain, not the truth: a
//! hand-deleted book directory drops out of the shelf on the next read rather
//! than leaving an entry that opens onto nothing.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Book;

/// One row on the shelf. Deliberately small -- enough to draw a card without
/// loading a book's text.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Shelf {
    pub id: String,
    pub title: String,
    pub author: String,
    pub chapters: usize,
    pub words: usize,
    /// File name of the cover within the book's directory, if it has one.
    pub cover: Option<String>,
    /// Seconds since the epoch, for ordering by most recently opened.
    pub added: u64,
    pub opened: u64,
    pub warnings: usize,
}

/// How far into a book the voice has read.
///
/// A `(chapter, unit)` pair rather than an EPUB CFI: a CFI addresses a DOM
/// range, which is the right answer for a renderer highlighting live nodes and
/// the wrong one here, where the thing being addressed is a chunk in an
/// enumeration. readest's own search feature uses text offsets for this reason.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Progress {
    pub chapter: usize,
    pub unit: usize,
    pub updated: u64,
}

pub struct Library {
    root: PathBuf,
}

impl Library {
    /// `root` is the books directory; it is created on first write, not here,
    /// so merely asking what is on the shelf never touches the disk.
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("books"),
        }
    }

    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    /// The shelf, newest activity first.
    ///
    /// Entries whose directory has gone are dropped and the shelf rewritten, so
    /// deleting a book by hand is a supported way to delete a book.
    pub fn shelf(&self) -> Vec<Shelf> {
        let raw: Vec<Shelf> = std::fs::read_to_string(self.root.join("library.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let kept: Vec<Shelf> = raw
            .into_iter()
            .filter(|b| self.dir(&b.id).join("book.json").exists())
            .collect();
        let mut out = kept.clone();
        out.sort_by_key(|b| std::cmp::Reverse(b.opened.max(b.added)));
        out
    }

    fn write_shelf(&self, rows: &[Shelf]) -> Result<(), String> {
        write_json(&self.root, "library.json", rows)
    }

    /// Adds a book, or refreshes one already present.
    ///
    /// Re-importing the same file is not an error and does not duplicate: the
    /// id is derived from the bytes, so the second import is the first one.
    pub fn add(&self, book: &Book, source: &[u8]) -> Result<Shelf, String> {
        let dir = self.dir(&book.id);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        std::fs::write(dir.join("source.epub"), source)
            .map_err(|e| format!("cannot store the book: {e}"))?;
        write_json(&dir, "book.json", book)?;

        let cover = crate::epub::cover(source).and_then(|(mime, bytes)| {
            let ext = match mime.as_str() {
                "image/png" => "png",
                "image/gif" => "gif",
                "image/webp" => "webp",
                _ => "jpg",
            };
            let name = format!("cover.{ext}");
            std::fs::write(dir.join(&name), bytes).ok().map(|_| name)
        });

        let now = now();
        let mut rows = self.shelf();
        let existing = rows.iter().position(|r| r.id == book.id);
        let row = Shelf {
            id: book.id.clone(),
            title: book.title.clone(),
            author: book.author.clone(),
            chapters: book.chapters.len(),
            words: book.words(),
            cover,
            added: existing.map_or(now, |i| rows[i].added),
            opened: now,
            warnings: book.warnings,
        };
        match existing {
            Some(i) => rows[i] = row.clone(),
            None => rows.push(row.clone()),
        }
        self.write_shelf(&rows)?;
        Ok(row)
    }

    pub fn get(&self, id: &str) -> Option<Book> {
        let s = std::fs::read_to_string(self.dir(id).join("book.json")).ok()?;
        serde_json::from_str(&s).ok()
    }

    /// Removes a book and everything stored with it.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        // Guard against an id that escapes the books directory. It arrives from
        // the window, and a path traversal here would delete arbitrary
        // directories.
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("not a book id".into());
        }
        let dir = self.dir(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| format!("cannot remove the book: {e}"))?;
        }
        let rows: Vec<Shelf> = self.shelf().into_iter().filter(|r| r.id != id).collect();
        self.write_shelf(&rows)
    }

    pub fn progress(&self, id: &str) -> Progress {
        std::fs::read_to_string(self.dir(id).join("progress.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Records a position, returning whether anything was written.
    ///
    /// Unchanged positions are skipped rather than rewritten: this is called
    /// from the reading clock, and bumping a timestamp because the voice is
    /// still in the same chunk would reorder the shelf for no reason.
    pub fn set_progress(&self, id: &str, chapter: usize, unit: usize) -> bool {
        let cur = self.progress(id);
        if cur.chapter == chapter && cur.unit == unit {
            return false;
        }
        let p = Progress {
            chapter,
            unit,
            updated: now(),
        };
        write_json(&self.dir(id), "progress.json", &p).is_ok()
    }

    /// Notes that a book was opened, so the shelf can lead with it.
    pub fn touch(&self, id: &str) {
        let mut rows = self.shelf();
        if let Some(r) = rows.iter_mut().find(|r| r.id == id) {
            r.opened = now();
            let _ = self.write_shelf(&rows);
        }
    }
}

/// Written to a temporary file and renamed, so an interrupted write cannot
/// leave JSON that fails to parse -- which for `library.json` would read as a
/// library that lost every book.
fn write_json<T: Serialize + ?Sized>(dir: &Path, name: &str, value: &T) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let tmp = dir.join(format!("{name}.tmp"));
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).map_err(|e| format!("cannot write {name}: {e}"))?;
    std::fs::rename(&tmp, dir.join(name)).map_err(|e| format!("cannot write {name}: {e}"))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Chapter;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("lector-lib-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn book(id: &str) -> Book {
        Book {
            id: id.into(),
            title: "A Book".into(),
            author: "An Author".into(),
            chapters: vec![Chapter {
                title: "One".into(),
                text: "Some words here.".into(),
            }],
            warnings: 0,
        }
    }

    #[test]
    fn an_added_book_comes_back() {
        let d = tmpdir("add");
        let lib = Library::new(&d);
        assert!(lib.shelf().is_empty());
        lib.add(&book("abc123"), b"not really an epub").unwrap();

        let shelf = lib.shelf();
        assert_eq!(shelf.len(), 1);
        assert_eq!(shelf[0].title, "A Book");
        assert_eq!(shelf[0].words, 3);
        assert_eq!(lib.get("abc123").unwrap().chapters.len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn re_importing_the_same_book_does_not_duplicate_it() {
        let d = tmpdir("dup");
        let lib = Library::new(&d);
        lib.add(&book("abc123"), b"x").unwrap();
        lib.add(&book("abc123"), b"x").unwrap();
        assert_eq!(lib.shelf().len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn progress_round_trips_and_skips_unchanged_writes() {
        let d = tmpdir("prog");
        let lib = Library::new(&d);
        lib.add(&book("abc123"), b"x").unwrap();
        assert!(lib.set_progress("abc123", 2, 17));
        let p = lib.progress("abc123");
        assert_eq!((p.chapter, p.unit), (2, 17));
        // Same position again: nothing written, so the timestamp holds still.
        assert!(!lib.set_progress("abc123", 2, 17));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_hand_deleted_book_leaves_the_shelf() {
        let d = tmpdir("gone");
        let lib = Library::new(&d);
        lib.add(&book("abc123"), b"x").unwrap();
        std::fs::remove_dir_all(lib.dir("abc123")).unwrap();
        assert!(lib.shelf().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn remove_takes_the_directory_and_the_row() {
        let d = tmpdir("rm");
        let lib = Library::new(&d);
        lib.add(&book("abc123"), b"x").unwrap();
        lib.remove("abc123").unwrap();
        assert!(lib.shelf().is_empty());
        assert!(!lib.dir("abc123").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_id_that_is_not_a_book_id_cannot_delete_anything() {
        let d = tmpdir("evil");
        let lib = Library::new(&d);
        assert!(lib.remove("../../etc").is_err());
        assert!(lib.remove("").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
