//! Sentence splitting and chunk packing.
//!
//! This is where listening quality lives. Two opposing pressures:
//!
//!   - a chunk synthesized alone gets no prosodic context, so a bare "Okay."
//!     lands as a clipped bark. `V1R4` solves this by merging short sentences
//!     until a floor of ~100 chars.
//!   - the first chunk sets the latency the user actually feels. Measured on an
//!     M1 Pro, Piper takes ~470 ms to first audio when chunk 1 is a 3-second
//!     sentence.
//!
//! So: every chunk after the first honours the floor; the **first chunk of an
//! utterance ignores it** and goes out as one sentence, however short.

use icu_segmenter::options::SentenceBreakInvariantOptions;
use icu_segmenter::SentenceSegmenter;

/// Sentences shorter than this are merged into the next one -- except the first.
pub const MIN_CHUNK_CHARS: usize = 100;
/// Stop adding sentences to a chunk once it reaches this.
pub const TARGET_CHUNK_CHARS: usize = 400;
/// A single sentence longer than this is hard-split at a space.
pub const MAX_CHUNK_CHARS: usize = 800;

/// Periods that end an abbreviation rather than a sentence.
///
/// ICU's sentence break handles much of this, but not domain terms. Kept small
/// and English-first on purpose; every entry is a rule someone must maintain.
const ABBREVIATIONS: &[&str] = &[
    "dr", "mr", "mrs", "ms", "prof", "sr", "jr", "st", "vs", "etc", "e.g", "i.e", "inc", "ltd",
    "fig", "no", "approx", "al", "ca", "cf", "jan", "feb", "mar", "apr", "jun", "jul", "aug",
    "sep", "sept", "oct", "nov", "dec",
];

fn ends_with_abbreviation(s: &str) -> bool {
    let t = s.trim_end();
    let Some(stripped) = t.strip_suffix('.') else {
        return false;
    };
    let last: String = stripped
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '.')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let last = last.to_ascii_lowercase();
    if ABBREVIATIONS.contains(&last.as_str()) {
        return true;
    }
    // A single initial: "J. R. R. Tolkien".
    if last.chars().count() == 1 && last.chars().all(|c| c.is_alphabetic()) {
        return true;
    }
    // A decimal point: "3.14".
    stripped.chars().last().is_some_and(|c| c.is_ascii_digit())
        && stripped
            .chars()
            .rev()
            .nth(1)
            .is_some_and(|c| c.is_ascii_digit())
}

/// Splits text into sentences using the Unicode sentence-break algorithm, then
/// re-joins any break that ICU put after an abbreviation.
pub fn split_sentences(text: &str) -> Vec<String> {
    let segmenter = SentenceSegmenter::new(SentenceBreakInvariantOptions::default());
    let mut out: Vec<String> = Vec::new();
    let mut prev = 0usize;

    for bp in segmenter.segment_str(text).skip(1) {
        let piece = &text[prev..bp];
        prev = bp;
        if piece.trim().is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if ends_with_abbreviation(last) => last.push_str(piece),
            _ => out.push(piece.to_string()),
        }
    }
    if prev < text.len() {
        let tail = &text[prev..];
        if !tail.trim().is_empty() {
            match out.last_mut() {
                Some(last) if ends_with_abbreviation(last) => last.push_str(tail),
                _ => out.push(tail.to_string()),
            }
        }
    }
    out.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Packs sentences into chunks for synthesis.
///
/// `first_chunk_fast` is the latency exception: when true, the first chunk is a
/// single sentence regardless of length, so the hotkey speaks almost instantly.
/// Set it false for export, where nothing is waiting and prosody wins.
pub fn pack_chunks(sentences: &[String], first_chunk_fast: bool) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut cur = String::new();
    let len = |s: &str| s.chars().count();

    for sentence in sentences {
        for piece in hard_split(sentence) {
            // The latency exception: the very first sentence goes out alone.
            if first_chunk_fast && chunks.is_empty() && cur.is_empty() {
                chunks.push(piece);
                continue;
            }

            if cur.is_empty() {
                cur = piece;
            } else if len(&cur) + 1 + len(&piece) <= TARGET_CHUNK_CHARS {
                cur.push(' ');
                cur.push_str(&piece);
            } else {
                // Adding this would overshoot the target, so close the current
                // chunk even if it has not reached the floor yet.
                chunks.push(std::mem::take(&mut cur));
                cur = piece;
            }

            // Flush once past the floor. This is V1R4's rule: a chunk is allowed
            // to be short, but not so short that it loses its prosodic footing.
            if len(&cur) >= MIN_CHUNK_CHARS {
                chunks.push(std::mem::take(&mut cur));
            }
        }
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }

    merge_unspeakable(chunks)
}

/// Splits a sentence that exceeds MAX_CHUNK_CHARS at the last space in budget.
fn hard_split(s: &str) -> Vec<String> {
    if s.chars().count() <= MAX_CHUNK_CHARS {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    let mut rest = s;
    while rest.chars().count() > MAX_CHUNK_CHARS {
        let budget: usize = rest
            .char_indices()
            .nth(MAX_CHUNK_CHARS)
            .map(|(i, _)| i)
            .unwrap_or(rest.len());
        // Only cut at a space if one exists reasonably deep into the budget;
        // otherwise a pathological run of non-spaces would emit slivers.
        let cut = rest[..budget]
            .rfind(' ')
            .filter(|i| *i > budget / 3)
            .unwrap_or(budget);
        let (head, tail) = rest.split_at(cut);
        out.push(head.trim().to_string());
        rest = tail.trim_start();
    }
    let rest = rest.trim();
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

/// A chunk with no word characters is a click, not a word. Merge it backwards,
/// or forwards if it is first.
fn merge_unspeakable(chunks: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in chunks {
        if !crate::sanitize::is_speakable(&c) {
            if let Some(last) = out.last_mut() {
                last.push(' ');
                last.push_str(&c);
                continue;
            }
        }
        out.push(c);
    }
    // A leading unspeakable chunk has no predecessor; fold it into its successor.
    if out.len() >= 2 && !crate::sanitize::is_speakable(&out[0]) {
        let head = out.remove(0);
        out[0] = format!("{head} {}", out[0]);
    }
    out.retain(|c| crate::sanitize::is_speakable(c));
    out
}
