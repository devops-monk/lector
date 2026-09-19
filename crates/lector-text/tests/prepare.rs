//! The rules in `lector-text` are the ones most likely to be subtly wrong and the
//! cheapest to pin down, so they get real coverage rather than a smoke test.

use lector_text::sanitize::SanitizeOptions;
use lector_text::*;

const N: SanitizeOptions = SanitizeOptions::NARRATION;
const V: SanitizeOptions = SanitizeOptions::VERBATIM;

// ---------- sanitize ----------

#[test]
fn strips_emphasis_but_keeps_words() {
    assert_eq!(sanitize("this is **bold** and *italic* and ~~gone~~", N), "this is bold and italic and gone");
}

#[test]
fn inline_code_keeps_its_content() {
    assert_eq!(sanitize("run `cargo test` now", N), "run cargo test now");
}

#[test]
fn fenced_code_becomes_one_spoken_phrase() {
    let md = "before\n```rust\nfn main() {}\n```\nafter";
    assert_eq!(sanitize(md, N), "before\nCode block.\nafter");
}

#[test]
fn consecutive_fences_announce_once() {
    let md = "```\na\n```\n```\nb\n```";
    assert_eq!(sanitize(md, N), "Code block.");
}

#[test]
fn fenced_code_can_be_dropped_silently() {
    let md = "before\n```\nx\n```\nafter";
    assert_eq!(sanitize(md, V), "before\nafter");
}

#[test]
fn tables_are_declined_not_read() {
    let md = "intro\n| a | b |\n|---|---|\n| 1 | 2 |\noutro";
    assert_eq!(sanitize(md, N), "intro\nTable omitted.\noutro");
}

#[test]
fn a_single_pipe_line_is_not_a_table() {
    assert_eq!(sanitize("| not a table", N), "| not a table");
}

#[test]
fn links_keep_the_label_and_lose_the_url() {
    assert_eq!(sanitize("see [the docs](https://example.com/x) here", N), "see the docs here");
    assert_eq!(sanitize("![alt text](img.png)", N), "alt text");
}

#[test]
fn headings_and_bullets_get_a_full_stop() {
    assert_eq!(sanitize("# Title", N), "Title.");
    assert_eq!(sanitize("- first\n- second", N), "first.\nsecond.");
    assert_eq!(sanitize("1. one\n2) two", N), "one.\ntwo.");
}

#[test]
fn heading_that_already_ends_in_punctuation_is_left_alone() {
    assert_eq!(sanitize("## Really?", N), "Really?");
}

#[test]
fn horizontal_rules_are_silent_but_leave_a_break() {
    // The rule itself must never be spoken. It collapses to a paragraph break
    // rather than vanishing, because a later phase uses blank lines to decide
    // how long a gap to leave between chunks -- and a section break earns one.
    let out = sanitize("a\n---\nb", N);
    assert!(!out.contains('-'), "the rule was spoken: {out:?}");
    assert_eq!(out, "a\n\nb");
}

#[test]
fn emoji_are_dropped() {
    assert_eq!(sanitize("done \u{1F389} ok", N), "done ok");
}

#[test]
fn arrows_are_spoken_as_to() {
    assert_eq!(sanitize("a -> b", N), "a to b");
}

#[test]
fn paths_shorten_to_basename_for_narration_only() {
    assert_eq!(sanitize("edited /Users/me/src/main.rs today", N), "edited main.rs today");
    assert_eq!(sanitize("edited /Users/me/src/main.rs today", V), "edited /Users/me/src/main.rs today");
}

#[test]
fn urls_shorten_to_hostname_keeping_the_sentence_stop() {
    // The trailing period must survive, or two sentences run together.
    assert_eq!(sanitize("see https://www.example.com/a/b. Next.", N), "see example.com. Next.");
}

#[test]
fn mojibake_is_repaired() {
    assert_eq!(sanitize("it\u{00e2}\u{20ac}\u{2122}s fine", N), "it's fine");
}

#[test]
fn speakability_is_about_word_characters() {
    assert!(is_speakable("hi"));
    assert!(!is_speakable("---"));
    assert!(!is_speakable(" ) "));
}

// ---------- sentence splitting ----------

#[test]
fn splits_on_sentence_boundaries() {
    assert_eq!(split_sentences("One. Two! Three?"), vec!["One.", "Two!", "Three?"]);
}

#[test]
fn abbreviations_do_not_end_a_sentence() {
    assert_eq!(split_sentences("Dr. Smith arrived. Then he left."), vec!["Dr. Smith arrived.", "Then he left."]);
    assert_eq!(split_sentences("Use e.g. this one. Done."), vec!["Use e.g. this one.", "Done."]);
}

#[test]
fn decimals_do_not_end_a_sentence() {
    assert_eq!(split_sentences("Pi is 3.14 exactly. Yes."), vec!["Pi is 3.14 exactly.", "Yes."]);
}

#[test]
fn initials_do_not_end_a_sentence() {
    assert_eq!(split_sentences("J. R. Tolkien wrote it. Later."), vec!["J. R. Tolkien wrote it.", "Later."]);
}

// ---------- chunking ----------

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

#[test]
fn first_chunk_goes_out_alone_when_latency_matters() {
    let sents = s(&["Okay.", "Now a much longer follow up sentence that carries the real content."]);
    let c = pack_chunks(&sents, true);
    assert_eq!(c[0], "Okay.", "the first chunk must not wait for the floor");
}

#[test]
fn without_the_exception_short_sentences_merge() {
    let sents = s(&["Okay.", "Sure.", "Fine."]);
    let c = pack_chunks(&sents, false);
    assert_eq!(c, vec!["Okay. Sure. Fine."], "short sentences alone would bark");
}

#[test]
fn chunks_respect_the_target_ceiling() {
    let long: Vec<String> = (0..40).map(|i| format!("This is sentence number {i} in a long document.")).collect();
    for c in pack_chunks(&long, false) {
        assert!(c.chars().count() <= TARGET_CHUNK_CHARS, "chunk overshot target: {}", c.chars().count());
    }
}

#[test]
fn a_monster_sentence_is_hard_split_at_a_space() {
    let monster = "word ".repeat(400);
    let c = pack_chunks(&s(&[&monster]), false);
    assert!(c.len() > 1);
    for piece in &c {
        assert!(piece.chars().count() <= MAX_CHUNK_CHARS);
        assert!(!piece.starts_with(' ') && !piece.ends_with(' '));
    }
    let rejoined: String = c.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
    let original: String = monster.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(rejoined, original, "hard split dropped words");
}

#[test]
fn punctuation_only_chunks_are_never_synthesized_alone() {
    let c = pack_chunks(&s(&["Hello there.", "---", "Goodbye now."]), false);
    assert!(c.iter().all(|x| is_speakable(x)), "a click would be synthesized: {c:?}");
}

#[test]
fn nothing_is_lost_between_sentences_and_chunks() {
    let text = "First one here. Second one follows. Third arrives late.";
    let words: Vec<&str> = text.split_whitespace().filter(|w| !w.is_empty()).collect();
    let chunks = prepare(text, N, false);
    let back: Vec<String> = chunks.join(" ").split_whitespace().map(|s| s.to_string()).collect();
    assert_eq!(back.len(), words.len(), "words lost: {chunks:?}");
}

#[test]
fn empty_and_whitespace_input_produce_no_chunks() {
    assert!(prepare("", N, true).is_empty());
    assert!(prepare("   \n\n  ", N, true).is_empty());
    assert!(prepare("```\nonly code\n```", V, true).is_empty());
}

#[test]
fn cjk_text_survives_the_pipeline() {
    let c = prepare("\u{4ECA}\u{65E5}\u{306F}\u{6674}\u{308C}\u{3002}\u{660E}\u{65E5}\u{306F}\u{96E8}\u{3002}", N, false);
    assert!(!c.is_empty());
    assert!(c.join("").contains('\u{6674}'));
}
