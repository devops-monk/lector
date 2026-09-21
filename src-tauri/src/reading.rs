//! Where the voice had got to when Lector last closed.
//!
//! Separate from [`crate::settings`] on purpose: settings are preferences, and
//! this is a bookmark. It is also the one piece of state that changes while the
//! app is merely running, so it is the one piece that needs a write policy
//! rather than just a writer.
//!
//! The policy is: keep it in memory, write at most once every five seconds, and
//! write once more on the way out. A bookmark is worth a few seconds of
//! staleness; it is not worth a file write every time a sentence ends.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const DEBOUNCE: Duration = Duration::from_secs(5);

/// A document and how far into it the voice had read.
///
/// The text is stored, not a reference to it: there is no library yet, so there
/// is nothing else to point at. Phase 2 replaces this with a book id, and the
/// index survives unchanged because it already means "unit in the enumeration"
/// rather than "offset in some text".
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Reading {
    pub text: String,
    pub index: usize,
    pub total: usize,
}

impl Reading {
    fn path(dir: &Path) -> PathBuf {
        dir.join("reading.json")
    }
}

/// Holds the bookmark and decides when it earns a disk write.
pub struct Bookmark {
    dir: PathBuf,
    state: Mutex<Option<Reading>>,
    /// `None` until the first write, so the first position lands immediately
    /// rather than five seconds in -- which is exactly the window in which
    /// someone tries the feature by quitting straight away.
    last_write: Mutex<Option<Instant>>,
}

impl Bookmark {
    /// Reads whatever the last session left behind. A missing or unparseable
    /// file is simply no bookmark, never an error: losing a position is a
    /// nuisance, and refusing to start over it would be worse.
    pub fn load(dir: &Path) -> Self {
        let state = std::fs::read_to_string(Reading::path(dir))
            .ok()
            .and_then(|s| serde_json::from_str::<Reading>(&s).ok())
            .filter(|r| !r.text.is_empty());
        Self {
            dir: dir.to_path_buf(),
            state: Mutex::new(state),
            last_write: Mutex::new(None),
        }
    }

    pub fn get(&self) -> Option<Reading> {
        self.state.lock().unwrap().clone()
    }

    /// Remembers a whole document, from the start.
    pub fn begin(&self, text: &str, index: usize, total: usize) {
        *self.state.lock().unwrap() = Some(Reading {
            text: text.to_string(),
            index,
            total,
        });
        self.flush();
    }

    /// Records a new position, writing only if the debounce has elapsed.
    ///
    /// Called from the engine's position callback, which fires every few
    /// seconds of speech and must not block on a filesystem.
    pub fn mark(&self, index: usize) {
        {
            let mut g = self.state.lock().unwrap();
            let Some(r) = g.as_mut() else { return };
            if r.index == index {
                return; // nothing changed; do not touch the file
            }
            r.index = index;
        }
        let due = self
            .last_write
            .lock()
            .unwrap()
            .is_none_or(|t| t.elapsed() >= DEBOUNCE);
        if due {
            self.flush();
        }
    }

    pub fn clear(&self) {
        *self.state.lock().unwrap() = None;
        let _ = std::fs::remove_file(Reading::path(&self.dir));
    }

    /// Writes now, via a temporary file and a rename, so an interrupted write
    /// cannot leave JSON that fails to parse and silently loses the bookmark.
    pub fn flush(&self) {
        let Some(r) = self.get() else { return };
        let _ = std::fs::create_dir_all(&self.dir);
        let tmp = self.dir.join("reading.json.tmp");
        let Ok(text) = serde_json::to_string_pretty(&r) else {
            return;
        };
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, Reading::path(&self.dir));
        }
        *self.last_write.lock().unwrap() = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!("lector-bm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn a_position_survives_a_reload() {
        let dir = tmp();
        let bm = Bookmark::load(&dir);
        assert!(bm.get().is_none());
        bm.begin("hello there", 0, 4);
        bm.mark(2);
        bm.flush();

        let again = Bookmark::load(&dir);
        let r = again.get().expect("bookmark");
        assert_eq!(r.index, 2);
        assert_eq!(r.total, 4);
        assert_eq!(r.text, "hello there");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn marking_the_same_index_does_not_rewrite() {
        let dir = tmp().join("same");
        let bm = Bookmark::load(&dir);
        bm.begin("x", 1, 2);
        let first = *bm.last_write.lock().unwrap();
        bm.mark(1);
        assert_eq!(first, *bm.last_write.lock().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_position_within_the_debounce_is_kept_in_memory() {
        let dir = tmp().join("debounce");
        let bm = Bookmark::load(&dir);
        bm.begin("x", 0, 9);
        bm.mark(5);
        // Written already (begin flushed), so this one is only in memory --
        // which is the whole point, and why quitting has to flush.
        assert_eq!(bm.get().unwrap().index, 5);
        assert_eq!(Bookmark::load(&dir).get().unwrap().index, 0);
        bm.flush();
        assert_eq!(Bookmark::load(&dir).get().unwrap().index, 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clearing_removes_the_file() {
        let dir = tmp().join("clear");
        let bm = Bookmark::load(&dir);
        bm.begin("x", 3, 9);
        bm.clear();
        assert!(bm.get().is_none());
        assert!(Bookmark::load(&dir).get().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
