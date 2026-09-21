//! PDF, last and clearly labelled.
//!
//! PDF has no notion of reading order. A page is a set of positioned glyph
//! runs, and turning that back into sentences is a reconstruction, not a read.
//! Multi-column layouts interleave, running heads land mid-sentence, and
//! hyphenation breaks words across lines.
//!
//! That is why importing one goes through [`preview`] first. A bad extraction
//! produces narration that is *confidently wrong*, which is worse than an
//! unsupported format: on screen it is obvious in a second, and by ear it takes
//! thirty of them to notice and longer to be sure.

use crate::{epub, text, Book};

/// Extracts a PDF's text without adding anything to the library.
pub fn preview(path: &std::path::Path) -> Result<Extract, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read that file: {e}"))?;
    from_bytes(&bytes)
}

/// What one extraction attempt produced.
pub struct Extract {
    pub text: String,
    /// Pages that could not be read at all.
    pub failed_pages: Vec<u32>,
    pub total_pages: u32,
}

/// Extracts a PDF **one page at a time**.
///
/// Not `extract_text_from_mem`, which does the whole document in one call and
/// therefore loses all of it to any single bad page. pdf-extract indexes the
/// operands of graphics operators without checking they are there -- a `y`
/// curve with no operands panics at lib.rs:1824 -- and one malformed path in a
/// 700-page book takes the book with it. Per page, the same fault costs one
/// page and the reader is told which.
///
/// The panics are caught rather than avoided because they are in a dependency
/// and there is no way to ask it in advance whether a page will survive.
fn from_bytes(bytes: &[u8]) -> Result<Extract, String> {
    let doc = std::panic::catch_unwind(|| pdf_extract::Document::load_mem(bytes))
        .map_err(|_| "that file could not be opened as a PDF".to_string())?
        .map_err(|e| format!("that file could not be opened as a PDF: {e}"))?;

    let pages = doc.get_pages();
    let total_pages = pages.len() as u32;
    if total_pages == 0 {
        return Err("that PDF has no pages".into());
    }

    let mut text = String::new();
    let mut failed_pages = Vec::new();
    for page_num in pages.keys().copied() {
        let page = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut buf = String::new();
            let mut out = pdf_extract::PlainTextOutput::new(&mut buf);
            pdf_extract::output_doc_page(&doc, &mut out, page_num).map(|_| buf)
        }));
        match page {
            Ok(Ok(s)) => {
                text.push_str(&s);
                text.push('\n');
            }
            // Either outcome is one lost page, not a lost book.
            _ => failed_pages.push(page_num),
        }
    }

    let text = clean(&text);
    if text.trim().chars().count() < 40 {
        return Err(
            "no text could be extracted -- this may be a scanned PDF, which is pictures of \
             words rather than words"
                .into(),
        );
    }
    Ok(Extract {
        text,
        failed_pages,
        total_pages,
    })
}

/// Imports a PDF whose extraction has already been shown and accepted.
pub fn import(path: &std::path::Path, extracted: &str) -> Result<Book, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read that file: {e}"))?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".into());
    Ok(text::from_text(
        &epub::id_for(&bytes),
        &name,
        extracted,
        false,
    ))
}

/// Repairs what extraction reliably breaks.
///
/// Only the two failures that are unambiguous. Column interleaving and stray
/// running heads are *not* repaired here: guessing at those produces text that
/// looks fixed and reads wrong, and the preview exists precisely so a person
/// can judge them instead.
fn clean(raw: &str) -> String {
    let raw = &strip_leaders(raw);
    let mut out = String::with_capacity(raw.len());
    let mut lines = raw.lines().peekable();

    while let Some(line) = lines.next() {
        let t = line.trim_end();
        // A line ending in a hyphen is a word split across lines; join it back
        // without the hyphen and without a space, or the voice says "concen"
        // and then "tration".
        if let Some(stem) = t.strip_suffix('-') {
            if stem.chars().last().is_some_and(|c| c.is_alphabetic()) {
                out.push_str(stem);
                continue;
            }
        }
        out.push_str(t);
        // A line that ends mid-sentence is a wrap, not a paragraph: join it to
        // the next with a space. One that ends in punctuation keeps its break.
        let wraps = !t.is_empty()
            && !t.ends_with(['.', '!', '?', ':', ';', '"', '”', '’'])
            && lines.peek().is_some_and(|n| !n.trim().is_empty());
        out.push(if wraps { ' ' } else { '\n' });
    }

    // Collapse the blank-line runs page breaks leave behind.
    let mut result = String::with_capacity(out.len());
    let mut blanks = 0;
    for line in out.lines() {
        let t = line.trim();
        if t.is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        result.push_str(t);
        result.push('\n');
    }
    result.trim().to_string()
}

/// Removes the dot leaders a table of contents is built from.
///
/// `Dedication.............. 2` is a line of typography, not a sentence, and a
/// voice reads every one of those stops. Three or more in a row is never
/// prose -- an ellipsis is one character or three, and this needs four before
/// it touches anything.
fn strip_leaders(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut dots = 0usize;
    for c in raw.chars() {
        if c == '.' {
            dots += 1;
            continue;
        }
        if dots > 0 {
            if dots >= 4 {
                out.push(' ');
            } else {
                for _ in 0..dots {
                    out.push('.');
                }
            }
            dots = 0;
        }
        out.push(c);
    }
    if dots >= 4 {
        out.push(' ');
    } else {
        for _ in 0..dots {
            out.push('.');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyphenated_line_breaks_are_rejoined() {
        let t = clean("a word that was concen-\ntration broken across lines.");
        assert!(t.contains("concentration"), "{t:?}");
    }

    #[test]
    fn a_trailing_dash_that_is_not_a_word_break_survives() {
        // An em-dash at a line end is punctuation, not hyphenation.
        let t = clean("she paused --\nthen went on.");
        assert!(t.contains("--"), "{t:?}");
    }

    #[test]
    fn wrapped_lines_become_one_sentence() {
        let t = clean("This sentence was wrapped\nacross two lines.");
        assert_eq!(t, "This sentence was wrapped across two lines.");
    }

    #[test]
    fn sentence_ends_keep_their_line_break() {
        let t = clean("One sentence.\nAnother sentence.");
        assert_eq!(t, "One sentence.\nAnother sentence.");
    }

    #[test]
    fn page_break_runs_collapse() {
        let t = clean("End of page.\n\n\n\nNext page.");
        assert_eq!(t, "End of page.\n\nNext page.");
    }

    #[test]
    fn a_file_that_is_not_a_pdf_is_refused() {
        assert!(from_bytes(b"not a pdf at all").is_err());
    }

    #[test]
    fn table_of_contents_leaders_are_not_read_aloud() {
        let t = clean("Dedication.............. 2\nChapter One............ 5");
        assert!(!t.contains(".."), "{t:?}");
        assert!(t.contains("Dedication"), "{t:?}");
        assert!(t.contains('2'), "{t:?}");
    }

    #[test]
    fn ordinary_punctuation_survives() {
        assert_eq!(strip_leaders("One. Two... Three."), "One. Two... Three.");
        assert_eq!(strip_leaders("end."), "end.");
    }

    #[test]
    fn an_empty_buffer_is_refused_rather_than_panicking() {
        assert!(from_bytes(b"").is_err());
    }
}
