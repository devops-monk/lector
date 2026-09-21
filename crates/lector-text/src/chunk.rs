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
use std::ops::Range;

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

/// Byte ranges of each sentence in `text`, in order, already trimmed.
///
/// Spans rather than owned strings because everything downstream needs to know
/// *where* a sentence came from: the follow-along view highlights a range of
/// the source, and a resume marker records a position in it. Building strings
/// here and recovering positions later would mean two descriptions of the same
/// split, which is exactly the kind of pair that drifts.
pub fn sentence_spans(text: &str) -> Vec<Range<usize>> {
    let segmenter = SentenceSegmenter::new(SentenceBreakInvariantOptions::default());
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut prev = 0usize;

    let push = |out: &mut Vec<Range<usize>>, start: usize, end: usize| {
        // Trim by walking the bytes, so the range stays valid UTF-8 boundaries.
        let piece = &text[start..end];
        let lead = piece.len() - piece.trim_start().len();
        let trail = piece.len() - piece.trim_end().len();
        if lead + trail >= piece.len() {
            return; // all whitespace
        }
        let (s, e) = (start + lead, end - trail);
        match out.last_mut() {
            // ICU breaks after "Dr." and "e.g."; re-join by extending the
            // previous span rather than concatenating strings.
            Some(last) if ends_with_abbreviation(&text[last.clone()]) => last.end = e,
            _ => out.push(s..e),
        }
    };

    for bp in segmenter.segment_str(text).skip(1) {
        push(&mut out, prev, bp);
        prev = bp;
    }
    if prev < text.len() {
        push(&mut out, prev, text.len());
    }
    out
}

/// Splits text into sentences using the Unicode sentence-break algorithm, then
/// re-joins any break that ICU put after an abbreviation.
pub fn split_sentences(text: &str) -> Vec<String> {
    sentence_spans(text)
        .into_iter()
        .map(|r| text[r].to_string())
        .collect()
}

/// Packs sentences into chunks for synthesis.
///
/// `first_chunk_fast` is the latency exception: when true, the first chunk is a
/// single sentence regardless of length, so the hotkey speaks almost instantly.
/// Set it false for export, where nothing is waiting and prosody wins.
pub fn pack_chunks(sentences: &[String], first_chunk_fast: bool) -> Vec<String> {
    // Reconstruct a source so there is one packer rather than two. Joining with
    // a single space is what the old string-based packer did between sentences,
    // so behaviour is unchanged.
    let source = sentences.join(" ");
    let spans = {
        let mut at = 0usize;
        let mut v = Vec::with_capacity(sentences.len());
        for s in sentences {
            v.push(at..at + s.len());
            at += s.len() + 1; // the joining space
        }
        v
    };
    pack_units(&source, &spans, first_chunk_fast)
        .into_iter()
        .map(|u| u.text)
        .collect()
}

/// One synthesizable chunk, and where it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unit {
    /// Exactly what is handed to the synthesizer.
    pub text: String,
    /// Indices into the section's sentence list that this covers. Usually one;
    /// two or more when short sentences were merged for prosody.
    pub sentences: Range<usize>,
    /// Byte range of the source this covers, for highlighting.
    pub span: Range<usize>,
}

/// Packs sentence spans into units, preserving where each one came from.
///
/// This is the packer; `pack_chunks` is a thin wrapper over it. The rules are
/// unchanged: merge up to a prosodic floor, stop before overshooting the
/// target, hard-split a monster sentence at a space, and never emit a unit with
/// no word characters in it.
pub fn pack_units(source: &str, spans: &[Range<usize>], first_chunk_fast: bool) -> Vec<Unit> {
    let mut units: Vec<Unit> = Vec::new();
    // The unit being built: sentence index it started at, byte range so far.
    let mut cur: Option<(usize, Range<usize>)> = None;
    let chars = |r: &Range<usize>| source[r.clone()].chars().count();

    let flush = |units: &mut Vec<Unit>, cur: &mut Option<(usize, Range<usize>)>, end_si: usize| {
        if let Some((start_si, span)) = cur.take() {
            let text = source[span.clone()].trim().to_string();
            if !text.is_empty() {
                units.push(Unit {
                    text,
                    sentences: start_si..end_si + 1,
                    span,
                });
            }
        }
    };

    for (si, sentence) in spans.iter().enumerate() {
        for piece in hard_split_span(source, sentence) {
            // The latency exception: the very first sentence goes out alone.
            if first_chunk_fast && units.is_empty() && cur.is_none() {
                let text = source[piece.clone()].trim().to_string();
                if !text.is_empty() {
                    units.push(Unit {
                        text,
                        sentences: si..si + 1,
                        span: piece,
                    });
                }
                continue;
            }

            match &mut cur {
                None => cur = Some((si, piece)),
                Some((_, span)) => {
                    let joined = span.start..piece.end;
                    if chars(&joined) <= TARGET_CHUNK_CHARS {
                        *span = joined;
                    } else {
                        // Adding this would overshoot, so close the current unit
                        // even if it has not reached the floor yet.
                        flush(&mut units, &mut cur, si.saturating_sub(1));
                        cur = Some((si, piece));
                    }
                }
            }

            // Flush once past the floor: a unit may be short, but not so short
            // it loses its prosodic footing.
            if cur
                .as_ref()
                .is_some_and(|(_, sp)| chars(sp) >= MIN_CHUNK_CHARS)
            {
                flush(&mut units, &mut cur, si);
            }
        }
    }
    flush(&mut units, &mut cur, spans.len().saturating_sub(1));

    merge_unspeakable_units(source, units)
}

/// Splits a sentence that exceeds MAX_CHUNK_CHARS at the last space in budget.
///
/// Returns byte ranges into `source`, so a split piece still knows where it
/// came from. Most sentences pass through as a single range.
fn hard_split_span(source: &str, span: &Range<usize>) -> Vec<Range<usize>> {
    let text = &source[span.clone()];
    if text.chars().count() <= MAX_CHUNK_CHARS {
        return vec![span.clone()];
    }

    let mut out = Vec::new();
    let mut at = 0usize; // byte offset within `text`
    while text[at..].chars().count() > MAX_CHUNK_CHARS {
        let budget = text[at..]
            .char_indices()
            .nth(MAX_CHUNK_CHARS)
            .map(|(i, _)| at + i)
            .unwrap_or(text.len());
        // Only cut at a space reasonably deep into the budget; otherwise a run
        // of non-spaces would emit slivers.
        let cut = text[at..budget]
            .rfind(' ')
            .map(|i| at + i)
            .filter(|i| *i - at > (budget - at) / 3)
            .unwrap_or(budget);
        out.push(span.start + at..span.start + cut);
        at = cut + text[cut..].len() - text[cut..].trim_start().len();
    }
    if at < text.len() {
        out.push(span.start + at..span.end);
    }
    out
}

/// A unit with no word characters is a click, not a word. Merge it backwards,
/// or forwards if it is first.
fn merge_unspeakable_units(source: &str, units: Vec<Unit>) -> Vec<Unit> {
    let mut out: Vec<Unit> = Vec::new();
    for u in units {
        if !crate::sanitize::is_speakable(&u.text) {
            if let Some(last) = out.last_mut() {
                last.span = last.span.start..u.span.end;
                last.sentences = last.sentences.start..u.sentences.end;
                last.text = source[last.span.clone()].trim().to_string();
                continue;
            }
        }
        out.push(u);
    }
    // A leading unspeakable unit has no predecessor; fold it into its successor.
    if out.len() >= 2 && !crate::sanitize::is_speakable(&out[0].text) {
        let head = out.remove(0);
        out[0].span = head.span.start..out[0].span.end;
        out[0].sentences = head.sentences.start..out[0].sentences.end;
        out[0].text = source[out[0].span.clone()].trim().to_string();
    }
    out.retain(|u| crate::sanitize::is_speakable(&u.text));
    out
}
