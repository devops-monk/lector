//! Text preparation for speech: markdown in, speakable chunks out.
//!
//! Deliberately pure and dependency-light. Everything here is a function of its
//! input, which is what makes the chunking rules -- the part most likely to be
//! subtly wrong -- cheap to test.

pub mod chunk;
pub mod sanitize;

pub use chunk::{
    pack_chunks, split_sentences, MAX_CHUNK_CHARS, MIN_CHUNK_CHARS, TARGET_CHUNK_CHARS,
};
pub use sanitize::{is_speakable, sanitize, SanitizeOptions};

/// The whole pipeline: raw text to the chunks a synthesizer will be handed.
pub fn prepare(input: &str, opts: SanitizeOptions, first_chunk_fast: bool) -> Vec<String> {
    let clean = sanitize(input, opts);
    let sentences = split_sentences(&clean);
    pack_chunks(&sentences, first_chunk_fast)
}
