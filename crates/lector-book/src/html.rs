//! XHTML to speakable text.
//!
//! Not a general HTML-to-text converter: it answers one question, which is what
//! a voice should say. That makes some of its choices look wrong for rendering
//! and right here -- a footnote marker is a useful thing to see and a jarring
//! thing to hear.
//!
//! Two failure modes drove the design:
//!
//! *Silent truncation.* Real EPUB 2 files are full of XHTML that is not
//! well-formed XML. A parser that stops at the first error produces a chapter
//! that simply ends, and a book that stops talking halfway sounds like a bug in
//! the audio, not in the import. This recovers and keeps going, and reports how
//! many errors it swallowed so the caller can say so.
//!
//! *Spurious spaces.* Emitting a space after every text node turns
//! `wo<em>rd</em>` into "wo rd" and `word<a>1</a>,` into "word ,". Text nodes
//! already carry the whitespace the author wrote; this preserves it and
//! normalizes runs at the end, which is what a browser does.

use std::collections::HashMap;

use quick_xml::events::{BytesRef, Event};
use quick_xml::Reader;

/// Resolves one `&entity;` or `&#1234;` reference to its text.
///
/// quick-xml delivers these as their own events rather than inside the text,
/// so that the caller can choose XML's five entities or HTML's full set. An
/// ebook is HTML, so it gets the full set, plus numeric references, which no
/// table can cover.
pub(crate) fn entity(r: &BytesRef) -> Option<String> {
    let name = r.clone().into_inner().into_owned();
    if let Some(num) = name.strip_prefix('#') {
        let c = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(c).map(String::from);
    }
    quick_xml::escape::resolve_predefined_entity(&name).map(str::to_string)
}

/// What came out of one XHTML file.
pub struct Extracted {
    pub text: String,
    /// Where each `id` in the document landed in `text`.
    ///
    /// Needed because a chapter is very often not a file. Gutenberg puts sixty
    /// chapters in fifteen documents and addresses them by fragment, so without
    /// these offsets the table of contents cannot be turned into chapters at
    /// all -- it can only name whole files, wrongly.
    pub anchors: std::collections::HashMap<String, usize>,
    /// XML errors recovered from. Zero for a well-formed file; a large number
    /// means the extraction is probably poor and the caller should say so.
    pub errors: usize,
}

/// Elements whose entire subtree is silent.
const SKIP: &[&str] = &[
    "script", "style", "head", "svg", "figure", "table", "rt", "rp",
];

/// Elements that end a line of speech. Paragraph structure is most of what
/// sentence splitting has to work with, so it has to survive the strip.
const BLOCK: &[&str] = &[
    "p",
    "div",
    "br",
    "li",
    "tr",
    "blockquote",
    "section",
    "article",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "figcaption",
    "hr",
    "dd",
    "dt",
];

/// `epub:type` values that mark a note rather than the text.
const NOTE_TYPES: &[&str] = &[
    "footnote",
    "endnote",
    "note",
    "rearnote",
    "noteref",
    "annotation",
];

fn is(tag: &str, set: &[&str]) -> bool {
    set.iter().any(|t| t.eq_ignore_ascii_case(tag))
}

/// True if this element carries an `epub:type` naming it a note.
///
/// Matched on the local name so both `epub:type` and a namespace-stripped
/// `type` are caught; EPUBs in the wild write it both ways.
fn is_note(e: &quick_xml::events::BytesStart) -> bool {
    e.attributes().flatten().any(|a| {
        if !a.key.local_name().as_ref().eq_ignore_ascii_case("type") {
            return false;
        }
        let v = a.value.to_ascii_lowercase();
        v.split_whitespace().any(|w| NOTE_TYPES.contains(&w))
    })
}

/// A bare footnote marker: "1", "[12]", "(3)", "*".
///
/// Checked on the collected text of a `sup` rather than on the markup, because
/// the markup varies endlessly and the thing being detected is what it *says*.
fn is_marker(s: &str) -> bool {
    let t = s
        .trim()
        .trim_matches(|c| matches!(c, '[' | ']' | '(' | ')' | '{' | '}'));
    !t.is_empty() && (t.chars().all(|c| c.is_ascii_digit()) || t == "*" || t == "†" || t == "‡")
}

/// Strips one XHTML file down to what should be spoken.
pub fn to_text(xhtml: &str) -> Extracted {
    let mut reader = Reader::from_str(xhtml);
    let cfg = reader.config_mut();
    // Both are the recovery: real EPUB 2 files mismatch end tags and use HTML
    // void elements that never close. Neither should end a chapter.
    cfg.check_end_names = false;
    cfg.allow_unmatched_ends = true;

    let mut out = String::with_capacity(xhtml.len() / 2);
    // (id, offset into `out`) as encountered. Translated to offsets into the
    // normalized text at the end, since normalizing moves everything.
    let mut raw_anchors: Vec<(String, usize)> = Vec::new();
    let mut errors = 0usize;
    let mut depth = 0usize;
    // Depth at which the current skipped subtree began. Tracking the depth
    // rather than a flag is what makes nested skips -- a table inside an aside
    // -- close in the right place.
    let mut skip_from: Option<usize> = None;
    // `sup` is collected aside rather than emitted, because whether it is
    // speech or a footnote marker is only knowable once it has ended.
    let mut sup: Option<String> = None;
    let mut last_pos = 0u64;

    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                depth += 1;
                if skip_from.is_none() {
                    note_anchor(&e, out.len(), &mut raw_anchors);
                }
                let tag = e.local_name();
                let tag = tag.as_ref();
                if skip_from.is_none() && (is(tag, SKIP) || is_note(&e)) {
                    skip_from = Some(depth);
                } else if skip_from.is_none() {
                    if tag.eq_ignore_ascii_case("sup") {
                        sup = Some(String::new());
                    } else if is(tag, BLOCK) {
                        out.push('\n');
                    }
                }
            }
            Ok(Event::End(e)) => {
                let tag = e.local_name();
                let tag = tag.as_ref();
                if skip_from == Some(depth) {
                    skip_from = None;
                } else if tag.eq_ignore_ascii_case("sup") {
                    if let Some(s) = sup.take() {
                        if !is_marker(&s) {
                            out.push_str(&s);
                        }
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Empty(e)) => {
                if skip_from.is_none() {
                    note_anchor(&e, out.len(), &mut raw_anchors);
                    if is(e.local_name().as_ref(), BLOCK) {
                        out.push('\n');
                    }
                }
            }
            Ok(Event::Text(e)) => {
                if skip_from.is_some() {
                    continue;
                }
                // html_content, not the XML unescape: EPUB XHTML is full of
                // &nbsp; and &mdash;, which strict XML does not define and
                // which would otherwise be dropped or left as literal text.
                let t = e.html_content();
                // Every whitespace character in a text node becomes a space,
                // newlines included. The line breaks that matter are the ones
                // block tags put there; the ones in the source are indentation,
                // and treating them as breaks splits a wrapped paragraph into
                // half-sentences the splitter then has to guess about.
                let cleaned: String = t
                    .chars()
                    .map(|c| {
                        if c.is_whitespace() || c.is_control() {
                            ' '
                        } else {
                            c
                        }
                    })
                    .collect();
                match sup.as_mut() {
                    Some(buf) => buf.push_str(&cleaned),
                    None => out.push_str(&cleaned),
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if skip_from.is_some() {
                    continue;
                }
                let Some(text) = entity(&r) else { continue };
                match sup.as_mut() {
                    Some(buf) => buf.push_str(&text),
                    None => out.push_str(&text),
                }
            }
            Ok(_) => {}
            Err(_) => {
                errors += 1;
                // quick-xml leaves the cursor where it failed on some errors.
                // Without this the loop would spin on a single bad byte
                // forever, which is a worse failure than a truncated chapter.
                if reader.buffer_position() <= last_pos {
                    break;
                }
                if errors > 500 {
                    break;
                }
            }
        }
        last_pos = reader.buffer_position();
    }

    let (text, anchors) = normalize(&out, &raw_anchors);
    Extracted {
        text,
        anchors,
        errors,
    }
}

/// Records an element's `id`, if it has one, against where its content starts.
fn note_anchor(e: &quick_xml::events::BytesStart, at: usize, into: &mut Vec<(String, usize)>) {
    for a in e.attributes().flatten() {
        if a.key.local_name().as_ref().eq_ignore_ascii_case("id") && !a.value.is_empty() {
            into.push((a.value.clone().into_owned(), at));
            return;
        }
    }
}

/// Collapses the whitespace XHTML leaves behind, carrying the anchors with it.
///
/// One pass rather than a tidy-up followed by an offset fix-up: the anchors
/// have to land on the text they name, and any mapping computed after the fact
/// is a second implementation of the same rules waiting to disagree with the
/// first.
///
/// Blank lines survive, because a blank line is a paragraph break and the
/// sentence splitter uses them. Runs of three or more collapse to one: beyond
/// that it is layout, not structure.
fn normalize(raw: &str, anchors: &[(String, usize)]) -> (String, HashMap<String, usize>) {
    let mut out = String::with_capacity(raw.len());
    let mut mapped: HashMap<String, usize> = HashMap::new();

    // Anchors in the order they appear, so each can be claimed once, by the
    // next character actually emitted after it.
    let mut pending: Vec<&str> = Vec::new();
    let mut ordered: Vec<&(String, usize)> = anchors.iter().collect();
    ordered.sort_by_key(|(_, at)| *at);
    let mut next = 0usize;

    let mut newlines = 0usize;
    let mut space = false;

    for (i, c) in raw.char_indices() {
        while next < ordered.len() && ordered[next].1 <= i {
            pending.push(&ordered[next].0);
            next += 1;
        }
        if c == '\n' {
            newlines += 1;
            continue;
        }
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if !out.is_empty() {
            if newlines > 0 {
                for _ in 0..newlines.min(2) {
                    out.push('\n');
                }
            } else if space {
                out.push(' ');
            }
        }
        newlines = 0;
        space = false;
        // The anchor points at the first character of what it introduces, not
        // at the separator before it.
        for id in pending.drain(..) {
            mapped.entry(id.to_string()).or_insert(out.len());
        }
        out.push(c);
    }
    // Anchors past the last character -- an empty trailing element -- point at
    // the end, which is still a valid place to cut.
    for id in pending {
        mapped.entry(id.to_string()).or_insert(out.len());
    }
    for (id, _) in ordered.iter().skip(next) {
        mapped.entry(id.clone()).or_insert(out.len());
    }

    (out, mapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paragraphs_become_lines() {
        let x = to_text("<html><body><p>One.</p><p>Two.</p></body></html>");
        assert_eq!(x.text, "One.\nTwo.");
        assert_eq!(x.errors, 0);
    }

    #[test]
    fn inline_tags_do_not_split_words() {
        let x = to_text("<p>wo<em>rd</em> and <b>bold</b>.</p>");
        assert_eq!(x.text, "word and bold.");
    }

    #[test]
    fn scripts_and_styles_are_silent() {
        let x = to_text("<head><title>T</title></head><body><style>p{color:red}</style><p>Said.</p><script>var x=1</script></body>");
        assert_eq!(x.text, "Said.");
    }

    #[test]
    fn footnote_markers_are_not_read() {
        let x = to_text("<p>A claim<sup><a href=\"#n1\">12</a></sup> follows.</p>");
        assert_eq!(x.text, "A claim follows.");
    }

    #[test]
    fn superscripts_that_are_words_survive() {
        let x = to_text("<p>The 1<sup>st</sup> of May.</p>");
        assert_eq!(x.text, "The 1st of May.");
    }

    #[test]
    fn footnote_asides_are_skipped_including_their_children() {
        let x = to_text(
            "<body><p>Body.</p><aside epub:type=\"footnote\"><p>Not spoken.</p><p>Nor this.</p></aside><p>More.</p></body>",
        );
        assert_eq!(x.text, "Body.\nMore.");
    }

    #[test]
    fn a_mismatched_end_tag_does_not_end_the_chapter() {
        let x = to_text("<body><p>Before.</p></div><p>After.</p></body>");
        assert!(x.text.contains("Before."), "{:?}", x.text);
        assert!(x.text.contains("After."), "{:?}", x.text);
    }

    #[test]
    fn an_unclosed_void_tag_does_not_end_the_chapter() {
        let x =
            to_text("<body><p>Line one.<br>Line two.</p><img src=\"x.png\"><p>Three.</p></body>");
        assert!(x.text.contains("Three."), "{:?}", x.text);
    }

    #[test]
    fn entities_are_resolved() {
        let x = to_text("<p>Tom &amp; Jerry &#8212; friends.</p>");
        assert_eq!(x.text, "Tom & Jerry — friends.");
    }

    #[test]
    fn indentation_does_not_become_speech() {
        let x = to_text("<body>\n    <p>\n      Spread\n      over lines.\n    </p>\n</body>");
        assert_eq!(x.text, "Spread over lines.");
    }

    #[test]
    fn tables_are_skipped_rather_than_read_as_prose() {
        let x = to_text(
            "<body><p>Text.</p><table><tr><td>1</td><td>2</td></tr></table><p>End.</p></body>",
        );
        assert_eq!(x.text, "Text.\nEnd.");
    }

    #[test]
    fn empty_input_is_empty_not_an_error() {
        let x = to_text("");
        assert_eq!(x.text, "");
        assert_eq!(x.errors, 0);
    }
}
