//! Free books, from two catalogues.
//!
//! Project Gutenberg is the searchable one: seventy-five thousand books, an
//! OPDS feed, and no account. Standard Ebooks is a small curated shelf of the
//! same public-domain texts, typeset properly. Its full OPDS feed now requires
//! a patron account, so only the new-releases feed is used and the window
//! should say "recently released" rather than implying it can be searched.
//!
//! Both are Atom, so one parser serves both; they differ only in where the
//! download link is. There is no checksum to verify against -- neither
//! catalogue publishes one -- so what stands in for verification is parsing:
//! a download that does not import as a readable book never reaches the
//! library. That is the property actually worth having.

use std::io::Read;

use serde::{Deserialize, Serialize};

use crate::{epub, Book};

const GUTENBERG_SEARCH: &str = "https://www.gutenberg.org/ebooks/search/?format=opds&query=";
const STANDARD_EBOOKS_NEW: &str = "https://standardebooks.org/feeds/atom/new-releases";

/// A book offered by a catalogue, not yet downloaded.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Listing {
    /// Unique within a session, and what a progress event is keyed on.
    pub id: String,
    pub title: String,
    pub author: String,
    /// Where the catalogue's name goes, so a reader knows which shelf this is.
    pub source: String,
    /// A sentence about the book, when the catalogue offers one.
    pub note: String,
    /// Size in bytes, or zero when the catalogue does not say.
    pub bytes: u64,
    pub url: String,
}

/// Searches Project Gutenberg.
pub fn search(query: &str) -> Result<Vec<Listing>, String> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let feed = fetch_text(&format!("{GUTENBERG_SEARCH}{}", escape_query(q)))?;
    Ok(gutenberg(&feed))
}

/// The Standard Ebooks new-releases shelf.
pub fn new_releases() -> Result<Vec<Listing>, String> {
    let feed = fetch_text(STANDARD_EBOOKS_NEW)?;
    Ok(standard_ebooks(&feed))
}

/// Downloads a listing and imports it, or fails without leaving anything behind.
///
/// Held in memory rather than streamed to a file: these are single-digit
/// megabytes, and the bytes are needed whole anyway -- to parse, and to derive
/// the id that decides whether this book is already on the shelf.
pub fn fetch(
    listing: &Listing,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<(Book, Vec<u8>), String> {
    let resp = ureq::get(&listing.url)
        .set("User-Agent", "Lector")
        .call()
        .map_err(|e| format!("could not download that book: {e}"))?;

    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(listing.bytes);

    let mut bytes = Vec::with_capacity(total.max(1 << 16) as usize);
    let mut reader = resp.into_reader().take(64 * 1024 * 1024);
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("the download was interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        on_progress(bytes.len() as u64, total);
    }

    // Parsing is the verification. Neither catalogue publishes a checksum, and
    // a hash would only prove the bytes arrived intact -- this proves they are
    // a book Lector can actually read, which is the thing that matters.
    let book = epub::from_bytes(&bytes)?;
    Ok((book, bytes))
}

// ---- feed parsing --------------------------------------------------------

/// Gutenberg's search results.
///
/// Book entries are those whose subsection link is `/ebooks/<number>.opds`.
/// The feed also carries navigation entries -- "Authors", "Subjects" -- which
/// look like books until you notice their links are not numbered.
fn gutenberg(feed: &str) -> Vec<Listing> {
    let mut out = Vec::new();
    for e in entries(feed) {
        let Some(id) = links(&e)
            .into_iter()
            .find_map(|(_, href, _, _)| gutenberg_id(&href))
        else {
            continue;
        };
        let title = tag(&e, "title").unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        out.push(Listing {
            // The EPUB with images, which is the one the cache serves under
            // this name and the one every other reader uses.
            url: format!("https://www.gutenberg.org/cache/epub/{id}/pg{id}.epub"),
            id: format!("gutenberg-{id}"),
            title,
            // Gutenberg puts the author in `content`, not in `author` -- that
            // element names the catalogue itself.
            author: tag(&e, "content").unwrap_or_default(),
            source: "Project Gutenberg".into(),
            note: String::new(),
            bytes: 0,
        });
    }
    out
}

fn gutenberg_id(href: &str) -> Option<String> {
    let rest = href.strip_prefix("/ebooks/")?.strip_suffix(".opds")?;
    rest.chars()
        .all(|c| c.is_ascii_digit())
        .then(|| rest.to_string())
        .filter(|s| !s.is_empty())
}

/// Standard Ebooks' new releases.
///
/// Each entry offers several files; the plain `.epub` is the one to take.
/// `_advanced.epub` uses EPUB 3 features no plain reader needs, and
/// `.kepub.epub` is Kobo-specific.
fn standard_ebooks(feed: &str) -> Vec<Listing> {
    let mut out = Vec::new();
    for e in entries(feed) {
        let Some((href, _, length)) = links(&e)
            .into_iter()
            .filter(|(rel, href, kind, _)| {
                rel == "enclosure"
                    && kind.contains("epub")
                    && !href.contains("_advanced")
                    && !href.contains(".kepub")
            })
            .map(|(_, href, kind, length)| (href, kind, length))
            .next()
        else {
            continue;
        };
        let title = tag(&e, "title").unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        out.push(Listing {
            id: format!("se-{}", slug(&href)),
            title,
            author: tag(&e, "name").unwrap_or_default(),
            source: "Standard Ebooks".into(),
            note: tag(&e, "summary").unwrap_or_default(),
            bytes: length,
            url: href,
        });
    }
    out
}

/// The bodies of every `<entry>` in a feed.
///
/// String slicing rather than an XML parse: these are two known feeds, the
/// shape being read is four fields deep, and the parser is already earning its
/// keep on documents that are genuinely hostile. A feed that changes shape
/// yields no results, which is visible, rather than wrong results, which is not.
fn entries(feed: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = feed;
    while let Some(start) = rest.find("<entry>") {
        let after = &rest[start + 7..];
        let Some(end) = after.find("</entry>") else {
            break;
        };
        out.push(after[..end].to_string());
        rest = &after[end..];
    }
    out
}

/// The text of the first `<name>` element, unescaped.
fn tag(entry: &str, name: &str) -> Option<String> {
    let open = format!("<{name}");
    let start = entry.find(&open)?;
    let body = &entry[start..];
    let open_end = body.find('>')? + 1;
    // A self-closing element has no text.
    if body[..open_end].ends_with("/>") {
        return None;
    }
    let close = body.find(&format!("</{name}>"))?;
    let raw = &body[open_end..close];
    let text = crate::html::to_text(&format!("<p>{raw}</p>")).text;
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

/// Every `<link>` in an entry as (rel, href, type, length).
fn links(entry: &str) -> Vec<(String, String, String, u64)> {
    let mut out = Vec::new();
    let mut rest = entry;
    while let Some(start) = rest.find("<link") {
        let after = &rest[start..];
        let Some(end) = after.find('>') else { break };
        let tagtext = &after[..end];
        out.push((
            attr(tagtext, "rel").unwrap_or_default(),
            attr(tagtext, "href").unwrap_or_default(),
            attr(tagtext, "type").unwrap_or_default(),
            attr(tagtext, "length")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
        ));
        rest = &after[end..];
    }
    out
}

fn attr(tagtext: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let start = tagtext.find(&key)? + key.len();
    let end = tagtext[start..].find('"')? + start;
    Some(tagtext[start..end].replace("&amp;", "&"))
}

/// A short stable name for a download URL, for keying progress events.
fn slug(url: &str) -> String {
    url.rsplit('/')
        .next()
        .unwrap_or(url)
        .split(['?', '.'])
        .next()
        .unwrap_or(url)
        .to_string()
}

/// Percent-escapes a search query. Only what a query can contain needs it.
fn escape_query(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 8);
    for b in q.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn fetch_text(url: &str) -> Result<String, String> {
    let resp = ureq::get(url)
        .set("User-Agent", "Lector")
        .call()
        .map_err(|e| format!("could not reach the catalogue: {e}"))?;
    let mut body = Vec::new();
    resp.into_reader()
        .take(16 * 1024 * 1024)
        .read_to_end(&mut body)
        .map_err(|e| format!("could not read the catalogue: {e}"))?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUT: &str = r#"<feed>
      <entry><title>Authors</title><content type="text">10 author names match.</content>
        <link rel="subsection" href="/ebooks/authors/search.opds/?query=austen"/></entry>
      <entry><title>Pride and Prejudice</title><content type="text">Jane Austen</content>
        <link rel="subsection" href="/ebooks/1342.opds"/>
        <link rel="http://opds-spec.org/image/thumbnail" href="data:image/png;base64,AAAA"/></entry>
      <entry><title>Sense &amp; Sensibility</title><content type="text">Jane Austen</content>
        <link rel="subsection" href="/ebooks/161.opds"/></entry>
    </feed>"#;

    #[test]
    fn gutenberg_navigation_entries_are_not_books() {
        let out = gutenberg(GUT);
        assert_eq!(out.len(), 2, "{out:#?}");
        assert_eq!(out[0].title, "Pride and Prejudice");
        assert_eq!(out[0].author, "Jane Austen");
        assert_eq!(
            out[0].url,
            "https://www.gutenberg.org/cache/epub/1342/pg1342.epub"
        );
        assert_eq!(out[0].id, "gutenberg-1342");
    }

    #[test]
    fn gutenberg_titles_are_unescaped() {
        assert_eq!(gutenberg(GUT)[1].title, "Sense & Sensibility");
    }

    #[test]
    fn only_numbered_ebook_links_are_books() {
        assert_eq!(gutenberg_id("/ebooks/1342.opds").as_deref(), Some("1342"));
        assert_eq!(gutenberg_id("/ebooks/authors/search.opds/?query=x"), None);
        assert_eq!(gutenberg_id("/ebooks/.opds"), None);
        assert_eq!(gutenberg_id("https://example.com/1342.opds"), None);
    }

    const SE: &str = r#"<feed><entry>
        <title>Contending Forces</title>
        <author><name>Pauline E. Hopkins</name></author>
        <summary type="text">A young woman reckons with her past.</summary>
        <link href="https://standardebooks.org/ebooks/x/y" rel="alternate" type="application/xhtml+xml"/>
        <link href="https://standardebooks.org/ebooks/x/y/downloads/x_y.epub?source=feed" length="529162" rel="enclosure" type="application/epub+zip"/>
        <link href="https://standardebooks.org/ebooks/x/y/downloads/x_y_advanced.epub?source=feed" length="586550" rel="enclosure" type="application/epub+zip"/>
        <link href="https://standardebooks.org/ebooks/x/y/downloads/x_y.kepub.epub?source=feed" length="463356" rel="enclosure" type="application/epub+zip"/>
      </entry></feed>"#;

    #[test]
    fn standard_ebooks_takes_the_plain_epub() {
        let out = standard_ebooks(SE);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "Contending Forces");
        assert_eq!(out[0].author, "Pauline E. Hopkins");
        assert_eq!(out[0].bytes, 529_162);
        assert!(
            out[0].url.ends_with("x_y.epub?source=feed"),
            "{}",
            out[0].url
        );
        assert!(out[0].note.starts_with("A young woman"));
    }

    #[test]
    fn a_feed_that_changed_shape_yields_nothing_rather_than_nonsense() {
        assert!(gutenberg("<feed></feed>").is_empty());
        assert!(standard_ebooks("not xml at all").is_empty());
    }

    #[test]
    fn queries_are_escaped() {
        assert_eq!(escape_query("jane austen"), "jane+austen");
        assert_eq!(escape_query("a&b"), "a%26b");
        assert_eq!(escape_query("café"), "caf%C3%A9");
    }

    #[test]
    fn an_empty_search_does_not_reach_the_network() {
        assert_eq!(search("   ").unwrap().len(), 0);
    }
}
