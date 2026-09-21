//! Ebooks, turned into something the reader can speak.
//!
//! Separate from `lector-text`, which is deliberately pure and
//! dependency-light: EPUB needs zip and XML, and neither belongs in the crate
//! that chunks sentences.
//!
//! Everything here converts into one shape, [`Book`], so adding a format later
//! never changes the reader.

pub mod epub;
pub mod html;
pub mod library;
pub mod pdf;
pub mod text;
pub mod web;

pub use library::{Library, Progress, Shelf};

use serde::{Deserialize, Serialize};

/// A book, ready to read.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Book {
    /// Stable across imports of the same file, and the directory name it lives
    /// in. Derived from the file's bytes, so re-importing a book already in the
    /// library lands on the book already in the library.
    pub id: String,
    pub title: String,
    pub author: String,
    pub chapters: Vec<Chapter>,
    /// XML errors recovered from during import, summed over every chapter.
    /// Kept so the window can say the extraction was rough rather than letting
    /// it be discovered by ear.
    #[serde(default)]
    pub warnings: usize,
}

/// One readable stretch, usually a chapter.
///
/// Holds text rather than sentences: enumerating is the reader's job, and doing
/// it here would mean two enumerations that could disagree about where the
/// voice is. That is the one invariant the whole reading path is built on.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Chapter {
    pub title: String,
    pub text: String,
}

impl Book {
    pub fn words(&self) -> usize {
        self.chapters
            .iter()
            .map(|c| c.text.split_whitespace().count())
            .sum()
    }
}
