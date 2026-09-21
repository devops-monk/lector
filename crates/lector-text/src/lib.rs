//! Text preparation for speech: markdown in, speakable chunks out.
//!
//! Deliberately pure and dependency-light. Everything here is a function of its
//! input, which is what makes the chunking rules -- the part most likely to be
//! subtly wrong -- cheap to test.

pub mod chunk;
pub mod sanitize;

pub use chunk::{
    pack_chunks, pack_units, sentence_spans, split_sentences, Unit, MAX_CHUNK_CHARS,
    MIN_CHUNK_CHARS, TARGET_CHUNK_CHARS,
};
pub use sanitize::{is_speakable, sanitize, SanitizeOptions};

/// One readable stretch of a document -- a chapter, an article, a pasted blob --
/// enumerated once.
///
/// This is the single source of truth readest insists on, made structural: the
/// synthesizer speaks `units[i].text`, the view renders spans of `source`, the
/// highlight is an index into `units`, and a resume marker is that same index.
/// Nothing re-derives any of it, so nothing can disagree about where the voice
/// is.
#[derive(Clone, Debug)]
pub struct Section {
    /// The sanitized text. All spans index into this, not the raw input.
    pub source: String,
    /// Byte range of each sentence, in order.
    pub sentences: Vec<std::ops::Range<usize>>,
    /// The synthesizable chunks, each knowing which sentences it covers.
    pub units: Vec<Unit>,
}

impl Section {
    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }
}

/// Enumerates a section: sanitize, split into sentences, pack into units.
///
/// `first_chunk_fast` should be **false** for anything being read as a
/// document. It changes where chunk boundaries fall, so an enumeration built
/// with it is not the one a mid-document seek would reproduce, and a resume
/// marker recorded against it would point somewhere else. The hotkey path wants
/// it true, because there latency beats determinism.
pub fn enumerate(input: &str, opts: SanitizeOptions, first_chunk_fast: bool) -> Section {
    let source = sanitize(input, opts);
    let sentences = sentence_spans(&source);
    let units = pack_units(&source, &sentences, first_chunk_fast);
    Section {
        source,
        sentences,
        units,
    }
}

/// The whole pipeline: raw text to the chunks a synthesizer will be handed.
///
/// A thin view over [`enumerate`], which is the one implementation.
pub fn prepare(input: &str, opts: SanitizeOptions, first_chunk_fast: bool) -> Vec<String> {
    enumerate(input, opts, first_chunk_fast)
        .units
        .into_iter()
        .map(|u| u.text)
        .collect()
}
