//! Plain text and Markdown.
//!
//! Nearly free, because `lector-text` already knows how to speak Markdown. The
//! only real question is where the chapters are, and the answer differs by
//! format: Markdown says so with its headings, and plain text only hints.

use crate::{Book, Chapter};

/// Imports a `.txt` or `.md` file.
pub fn import(path: &std::path::Path) -> Result<Book, String> {
    let raw = std::fs::read(path).map_err(|e| format!("cannot read that file: {e}"))?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let markdown = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "md" | "markdown"));
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Untitled".into());
    Ok(from_text(
        &crate::epub::id_for(&raw),
        &name,
        &text,
        markdown,
    ))
}

/// Builds a book from text already in hand.
pub fn from_text(id: &str, name: &str, text: &str, markdown: bool) -> Book {
    let chapters = if markdown {
        by_headings(text)
    } else {
        by_chapter_lines(text)
    };
    Book {
        id: id.to_string(),
        // The file's name, not its first heading. A heading is a chapter here
        // -- it is what the document was split on -- and promoting it to the
        // book's title would leave chapter one unnamed.
        title: name.to_string(),
        author: String::new(),
        chapters: if chapters.is_empty() {
            vec![Chapter {
                title: name.to_string(),
                text: text.trim().to_string(),
            }]
        } else {
            chapters
        },
        warnings: 0,
    }
}

/// Splits Markdown at its shallowest heading level.
///
/// The shallowest rather than always `#`: plenty of documents start at `##`
/// because `#` was the page title, and splitting at a level the file never uses
/// produces one enormous chapter.
fn by_headings(text: &str) -> Vec<Chapter> {
    let mut levels: Vec<usize> = text.lines().filter_map(heading_level).collect();
    levels.sort_unstable();
    levels.dedup();
    if levels.is_empty() {
        return by_chapter_lines(text);
    }
    // The shallowest level that actually divides the document. A file with one
    // `# Title` and six `## Sections` splits at the sections: dividing at a
    // level used once produces one chapter, which is not a division at all.
    for level in &levels {
        let out = split_at(text, *level);
        if out.len() > 1 {
            return out;
        }
    }
    split_at(text, levels[0])
}

fn split_at(text: &str, level: usize) -> Vec<Chapter> {
    let mut out: Vec<Chapter> = Vec::new();
    let mut title = String::new();
    let mut body = String::new();
    for line in text.lines() {
        if heading_level(line) == Some(level) {
            push(&mut out, &title, &body);
            title = line.trim_start_matches('#').trim().to_string();
            body.clear();
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    push(&mut out, &title, &body);
    out
}

/// `## Heading` -- the level, or nothing if this is not an ATX heading.
fn heading_level(line: &str) -> Option<usize> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    // A heading needs a space after its hashes; `#tag` is not one.
    (1..=6).contains(&hashes).then_some(hashes).filter(|_| {
        line.as_bytes()
            .get(hashes)
            .is_some_and(|b| b.is_ascii_whitespace())
    })
}

/// Splits plain text at lines that announce a chapter.
///
/// Not at blank lines: those are paragraphs, and a novel would arrive as two
/// thousand chapters. A file with no such lines stays one chapter, which is
/// the honest answer -- inventing divisions would be worse than having none.
fn by_chapter_lines(text: &str) -> Vec<Chapter> {
    let heads: Vec<usize> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| is_chapter_line(l))
        .map(|(i, _)| i)
        .collect();
    if heads.len() < 2 {
        return Vec::new();
    }

    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    // Anything before the first heading is front matter and keeps its own entry
    // rather than being silently dropped.
    if heads[0] > 0 {
        push(&mut out, "Beginning", &lines[..heads[0]].join("\n"));
    }
    for (n, start) in heads.iter().enumerate() {
        let end = heads.get(n + 1).copied().unwrap_or(lines.len());
        push(
            &mut out,
            lines[*start].trim(),
            &lines[*start + 1..end].join("\n"),
        );
    }
    out
}

/// "CHAPTER IV", "Chapter 12.", "PART TWO" -- a short line that announces one.
fn is_chapter_line(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() || t.chars().count() > 60 {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    let Some(rest) = ["chapter ", "part ", "book ", "canto "]
        .iter()
        .find_map(|p| lower.strip_prefix(p))
    else {
        return false;
    };
    // Something has to follow the word, and it has to look like a number --
    // arabic, roman, or spelled out. "Chapter and verse" is not a heading.
    let word = rest
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(['.', ':', ',']);
    !word.is_empty()
        && (word.chars().all(|c| c.is_ascii_digit())
            || word.chars().all(|c| "ivxlcdm".contains(c))
            || [
                "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
                "eleven", "twelve",
            ]
            .contains(&word))
}

fn push(out: &mut Vec<Chapter>, title: &str, body: &str) {
    let text = body.trim();
    if text.is_empty() {
        return;
    }
    out.push(Chapter {
        title: if title.is_empty() {
            first_line(text)
        } else {
            title.to_string()
        },
        text: text.to_string(),
    });
}

/// A name taken from the text itself, for a section the document did not name.
///
/// Leading hashes are stripped: the line is very often a heading one level up
/// from the one being split on, and "# Notes" is a worse name than "Notes".
fn first_line(text: &str) -> String {
    text.lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .map(|l| l.chars().take(60).collect())
        .unwrap_or_else(|| "Untitled".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_splits_on_its_shallowest_heading() {
        let md = "## One\n\nFirst body.\n\n### Deeper\n\nStill one.\n\n## Two\n\nSecond body.\n";
        let b = from_text("id", "notes", md, true);
        assert_eq!(b.chapters.len(), 2);
        assert_eq!(b.chapters[0].title, "One");
        assert!(b.chapters[0].text.contains("Deeper"));
        assert_eq!(b.chapters[1].title, "Two");
    }

    #[test]
    fn a_hash_without_a_space_is_not_a_heading() {
        assert_eq!(heading_level("# Real"), Some(1));
        assert_eq!(heading_level("#nothashtag"), None);
        assert_eq!(heading_level("####### too deep"), None);
        assert_eq!(heading_level("plain"), None);
    }

    #[test]
    fn plain_text_splits_on_chapter_lines() {
        let txt = "A preface.\n\nCHAPTER I\n\nFirst.\n\nCHAPTER II\n\nSecond.\n";
        let b = from_text("id", "novel", txt, false);
        assert_eq!(b.chapters.len(), 3);
        assert_eq!(b.chapters[0].title, "Beginning");
        assert_eq!(b.chapters[1].title, "CHAPTER I");
        assert_eq!(b.chapters[2].text, "Second.");
    }

    #[test]
    fn prose_that_merely_mentions_a_chapter_is_not_a_heading() {
        assert!(is_chapter_line("CHAPTER IV"));
        assert!(is_chapter_line("Chapter 12."));
        assert!(is_chapter_line("Part Two"));
        assert!(!is_chapter_line("chapter and verse"));
        assert!(!is_chapter_line("She closed the chapter on all of it"));
        assert!(!is_chapter_line(""));
    }

    #[test]
    fn a_file_with_no_divisions_stays_one_chapter() {
        let b = from_text("id", "letter", "Just some prose.\n\nAnd more of it.", false);
        assert_eq!(b.chapters.len(), 1);
        assert_eq!(b.chapters[0].title, "letter");
        assert!(b.chapters[0].text.contains("And more"));
    }

    #[test]
    fn a_single_chapter_line_is_not_enough_to_split_on() {
        // One match is as likely to be a mention as a heading; two is a pattern.
        let b = from_text("id", "x", "Text.\n\nChapter 1\n\nMore.", false);
        assert_eq!(b.chapters.len(), 1);
    }
}
