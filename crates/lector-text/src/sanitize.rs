//! Markdown prose -> text a TTS engine can read aloud.
//!
//! Ported from `companion-tts/src/speech/transform.ts`, which makes *speech*
//! decisions rather than merely stripping markup: a code fence becomes the spoken
//! words "Code block." instead of silence, a table is declined rather than read
//! cell by cell, and a file path is reduced to its basename instead of being
//! spelled out one slash at a time.
//!
//! The rules are ordered and the order matters; each one assumes the ones above
//! it have already run.

/// Which of the aggressive rules apply. They are right for narrating an agent's
/// output and wrong for a Studio script, where the author wrote what they meant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SanitizeOptions {
    /// Replace a fenced code block with the phrase "Code block."
    pub announce_code: bool,
    /// Replace a markdown table with the phrase "Table omitted."
    pub omit_tables: bool,
    /// Reduce `/a/b/c.rs` to `c.rs` and `https://x.com/y` to `x.com`.
    pub shorten_paths: bool,
}

impl SanitizeOptions {
    /// Narrating an agent: every rule on. Unreadable constructs are frequent and
    /// the listener wants the gist, not the characters.
    pub const NARRATION: Self = Self {
        announce_code: true,
        omit_tables: true,
        shorten_paths: true,
    };
    /// Long-form reading: decline tables, but a prose document's code and links
    /// are usually worth hearing.
    pub const READING: Self = Self {
        announce_code: true,
        omit_tables: true,
        shorten_paths: false,
    };
    /// A script someone wrote to be spoken. Strip markup, change nothing else.
    pub const VERBATIM: Self = Self {
        announce_code: false,
        omit_tables: false,
        shorten_paths: false,
    };
}

impl Default for SanitizeOptions {
    fn default() -> Self {
        Self::READING
    }
}

/// True for characters that carry spoken content. A chunk with none of these is
/// a click, not a word -- see `is_speakable`.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// Whether this text would produce any sound worth hearing.
///
/// From `VoiceStudio/backend/services/chunked_tts.py` `_SPEAKABLE_RE`: a chunk of
/// pure punctuation ("---", ")") synthesizes to a click, so it is merged into a
/// neighbour rather than spoken alone.
pub fn is_speakable(s: &str) -> bool {
    s.chars().any(is_word_char)
}

fn is_emoji(c: char) -> bool {
    matches!(c as u32,
        0x1F1E6..=0x1F1FF | 0x1F300..=0x1FAFF | 0x2600..=0x27BF
        | 0x2190..=0x21FF  | 0x2B00..=0x2BFF  | 0xFE0F | 0x200D)
}

/// Targeted Windows-1252 mojibake repairs.
///
/// These byte sequences never occur in legitimate prose, so applying them
/// unconditionally is safe. A blanket latin1->utf8 round-trip is FORBIDDEN: it
/// corrupts the correctly-encoded majority to fix a handful of lines.
const MOJIBAKE: &[(&str, &str)] = &[
    ("\u{00e2}\u{20ac}\u{2014}", "\u{2014}"),
    ("\u{00e2}\u{20ac}\u{2013}", "\u{2013}"),
    ("\u{00e2}\u{20ac}\u{0153}", "\""),
    ("\u{00e2}\u{20ac}\u{2122}", "'"),
    ("\u{00e2}\u{20ac}\u{02dc}", "'"),
    ("\u{00e2}\u{20ac}\u{00a6}", "\u{2026}"),
];

/// Arrows read as "to" rather than being spelled or skipped.
const ARROWS: &[&str] = &["\u{2192}", "->", "\u{21d2}", "=>"];

pub fn sanitize(input: &str, opts: SanitizeOptions) -> String {
    let mut s = input.to_string();

    for (from, to) in MOJIBAKE {
        if s.contains(from) {
            s = s.replace(from, to);
        }
    }

    s = strip_fenced_code(&s, opts.announce_code);
    if opts.omit_tables {
        s = omit_tables(&s);
    }

    let mut out = String::with_capacity(s.len());
    for line in s.lines() {
        out.push_str(&sanitize_line(line, opts));
        out.push('\n');
    }

    collapse_whitespace(&out)
}

/// Fenced code blocks become one spoken phrase, or nothing.
///
/// Consecutive fences collapse into a single announcement: an agent that writes
/// four snippets in a row should not say "Code block" four times.
fn strip_fenced_code(s: &str, announce: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_fence = false;
    let mut announced = false;

    for line in s.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            if in_fence && announce && !announced {
                out.push_str("Code block.\n");
                announced = true;
            }
            continue;
        }
        if in_fence {
            continue;
        }
        if !line.trim().is_empty() {
            announced = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// A run of two or more pipe-rows is a table. Reading one aloud cell by cell is
/// unbearable, so it is declined out loud instead.
fn omit_tables(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let is_row = |l: &str| {
        let t = l.trim();
        t.starts_with('|') && t.matches('|').count() >= 2
    };

    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < lines.len() {
        if is_row(lines[i]) {
            let start = i;
            while i < lines.len() && is_row(lines[i]) {
                i += 1;
            }
            if i - start >= 2 {
                out.push_str("Table omitted.\n");
                continue;
            }
            for l in &lines[start..i] {
                out.push_str(l);
                out.push('\n');
            }
            continue;
        }
        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }
    out
}

/// Headings and list items get a terminal period so the engine gives them a full
/// stop; without it they run into the next line with rising intonation.
fn terminate(s: &str) -> String {
    let t = s.trim_end();
    if t.is_empty() || t.ends_with(['.', '!', '?', ':', ';', ',']) {
        t.to_string()
    } else {
        format!("{t}.")
    }
}

fn sanitize_line(line: &str, opts: SanitizeOptions) -> String {
    let mut s = line.to_string();
    let trimmed = s.trim_start().to_string();

    // Horizontal rules are silent.
    let bare: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    if bare.len() >= 3
        && (bare.chars().all(|c| c == '-')
            || bare.chars().all(|c| c == '*')
            || bare.chars().all(|c| c == '_'))
    {
        return String::new();
    }

    let mut needs_stop = false;

    // Heading markers, blockquote markers, list bullets: drop the marker, keep
    // the text, and remember that this line should end in a full stop.
    if let Some(rest) = strip_leading_marker(&trimmed) {
        s = rest;
        needs_stop = true;
    }

    s = inline_markup(&s, opts);

    if needs_stop {
        s = terminate(&s);
    }
    s
}

/// Strips `#`, `>`, `-`, `*`, `+` and `1.` style line markers.
fn strip_leading_marker(line: &str) -> Option<String> {
    let t = line.trim_start();
    for p in [
        "###### ", "##### ", "#### ", "### ", "## ", "# ", "> ", "- ", "* ", "+ ",
    ] {
        if let Some(rest) = t.strip_prefix(p) {
            return Some(rest.to_string());
        }
    }
    // Ordered list: "1. ", "12) "
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() && digits.len() <= 3 {
        let rest = &t[digits.len()..];
        for p in [". ", ") "] {
            if let Some(r) = rest.strip_prefix(p) {
                return Some(r.to_string());
            }
        }
    }
    None
}

fn inline_markup(s: &str, opts: SanitizeOptions) -> String {
    let mut s = s.to_string();

    // Images and links: keep the visible label, drop the target.
    s = replace_links(&s);

    // Inline code: keep the content, drop the backticks.
    s = s.replace('`', "");

    for a in ARROWS {
        s = s.replace(a, " to ");
    }

    if opts.shorten_paths {
        s = shorten_urls_and_paths(&s);
    }

    // Emphasis markers, longest first so `***` doesn't leave a stray `*`.
    for m in ["***", "**", "___", "__", "~~"] {
        s = s.replace(m, "");
    }
    s = s.replace('*', "");

    s.chars().filter(|c| !is_emoji(*c)).collect()
}

/// `[label](url)` and `![alt](src)` -> `label`.
fn replace_links(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'[' || (b[i] == b'!' && i + 1 < b.len() && b[i + 1] == b'[') {
            let open = if b[i] == b'!' { i + 1 } else { i };
            if let Some(close) = find_byte(b, open + 1, b']') {
                if close + 1 < b.len() && b[close + 1] == b'(' {
                    if let Some(paren) = find_byte(b, close + 2, b')') {
                        out.push_str(&s[open + 1..close]);
                        i = paren + 1;
                        continue;
                    }
                }
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn find_byte(b: &[u8], from: usize, needle: u8) -> Option<usize> {
    (from..b.len()).find(|&i| b[i] == needle)
}

/// A bare URL becomes its hostname; a path becomes its basename.
///
/// This is the single biggest win for agent narration, where a voice otherwise
/// spends twenty seconds spelling out `/Users/.../src/components/`.
fn shorten_urls_and_paths(s: &str) -> String {
    s.split_whitespace()
        .map(|tok| {
            let (lead, core, trail) = split_punct(tok);
            let short = if let Some(rest) = core
                .strip_prefix("https://")
                .or_else(|| core.strip_prefix("http://"))
            {
                rest.split('/')
                    .next()
                    .unwrap_or(rest)
                    .trim_start_matches("www.")
                    .to_string()
            } else if core.contains('/') && !core.contains(' ') && core.len() > 1 {
                core.rsplit('/')
                    .find(|p| !p.is_empty())
                    .unwrap_or(core)
                    .to_string()
            } else {
                core.to_string()
            };
            format!("{lead}{short}{trail}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Splits trailing sentence punctuation off a token so shortening a URL does not
/// swallow the full stop that ends the sentence.
fn split_punct(tok: &str) -> (&str, &str, &str) {
    let start = tok.len() - tok.trim_start_matches(['(', '[', '"', '\'']).len();
    let end = tok
        .trim_end_matches([')', ']', '"', '\'', '.', ',', ';', ':', '!', '?'])
        .len();
    if end <= start {
        return ("", tok, "");
    }
    (&tok[..start], &tok[start..end], &tok[end..])
}

/// Collapses runs of spaces and blank lines, and drops lines with nothing to say.
fn collapse_whitespace(s: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in s.lines() {
        let mut l = String::with_capacity(line.len());
        let mut space = false;
        for c in line.chars() {
            if c.is_whitespace() {
                space = true;
            } else {
                if space && !l.is_empty() {
                    l.push(' ');
                }
                space = false;
                l.push(c);
            }
        }
        let l = l.trim().to_string();
        if l.is_empty() {
            if lines.last().map(|p: &String| p.is_empty()) == Some(false) {
                lines.push(String::new());
            }
        } else {
            lines.push(l);
        }
    }
    while lines.last().map(|l| l.is_empty()) == Some(true) {
        lines.pop();
    }
    lines.join("\n")
}
