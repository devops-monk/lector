//! Reading a web article.
//!
//! The honest framing, and the one the window should use: this narrates *an
//! article*, not a page. Navigation, cookie notices, share buttons and comment
//! threads are not read, and a page that is mostly those will import as very
//! little -- which is the correct outcome, not a failure to hide.
//!
//! This is also one of only two places Lector makes an outbound request, the
//! other being model downloads. It fetches *in*; it never sends anything out,
//! and the text of a document never leaves the machine.

use std::io::Read;

use crate::html::{self, Options};
use crate::{epub, Book, Chapter};

/// How much of a page to read. Generous for an article, small enough that a
/// mistyped URL pointing at a file cannot exhaust memory.
const MAX_BYTES: usize = 8 * 1024 * 1024;

/// Fetches a URL and imports it as a one-chapter book.
pub fn import(url: &str) -> Result<Book, String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("that does not look like a web address".into());
    }
    let resp = ureq::get(url)
        .set("User-Agent", "Lector")
        .call()
        .map_err(|e| format!("could not fetch that page: {e}"))?;

    let kind = resp.content_type().to_ascii_lowercase();
    if !kind.contains("html") && !kind.contains("text") && !kind.is_empty() {
        return Err(format!("that address is {kind}, not a page to read"));
    }

    let mut body = Vec::new();
    resp.into_reader()
        .take(MAX_BYTES as u64)
        .read_to_end(&mut body)
        .map_err(|e| format!("could not read that page: {e}"))?;

    from_html(url, &String::from_utf8_lossy(&body))
}

/// Extracts an article from HTML already in hand.
pub fn from_html(url: &str, page: &str) -> Result<Book, String> {
    let title = title_of(page).unwrap_or_else(|| host_of(url));

    // `article` first, then `main`: the two elements that mean "this is the
    // content". A page with neither is read whole, minus its furniture.
    let mut best = html::to_text_with(
        page,
        Options {
            within: Some("article"),
            web: true,
        },
    );
    for tag in ["main", "body"] {
        if best.text.chars().count() >= 400 {
            break;
        }
        let next = html::to_text_with(
            page,
            Options {
                within: Some(tag),
                web: true,
            },
        );
        if next.text.chars().count() > best.text.chars().count() {
            best = next;
        }
    }

    if best.text.trim().chars().count() < 40 {
        return Err("there was no readable article on that page".into());
    }

    Ok(Book {
        id: epub::id_for(url.as_bytes()),
        title,
        author: host_of(url),
        chapters: vec![Chapter {
            title: host_of(url),
            text: best.text,
        }],
        warnings: best.errors,
    })
}

/// The document's `<title>`, trimmed of the site name publishers append to it.
fn title_of(page: &str) -> Option<String> {
    let lower = page.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open = lower[start..].find('>')? + start + 1;
    let end = lower[open..].find("</title")? + open;
    let raw: String = page[open..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let t = html::to_text(&format!("<p>{raw}</p>")).text;
    // "Headline | The Paper" -- the part before the separator is the article.
    let t = t
        .split([" | ", " — ", " – "].as_slice()[0])
        .next()
        .unwrap_or(&t)
        .trim()
        .to_string();
    (!t.is_empty()).then_some(t)
}

fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
        .trim_start_matches("www.")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_article_is_read_and_the_page_around_it_is_not() {
        let page = "<html><head><title>The Headline | The Paper</title></head><body>
            <nav><a href=\"/\">Home</a><a href=\"/world\">World</a></nav>
            <article><p>The article runs to several sentences, which is what makes it an article
            rather than a caption. It continues for long enough to clear the length floor that
            separates a page with content from a page with only navigation on it.</p></article>
            <footer><p>Terms and conditions</p></footer></body></html>";
        let b = from_html("https://www.example.com/story", page).unwrap();
        assert_eq!(b.title, "The Headline");
        assert_eq!(b.author, "example.com");
        assert!(b.chapters[0].text.starts_with("The article runs"));
        assert!(!b.chapters[0].text.contains("Terms and conditions"));
        assert!(!b.chapters[0].text.contains("Home"));
    }

    #[test]
    fn a_page_with_no_article_element_still_reads() {
        let page = "<html><head><title>Notes</title></head><body>
            <nav>Menu</nav><p>This page has no article element at all, but it does have prose,
            and enough of it that refusing to read the page would be the wrong answer here.</p>
            </body></html>";
        let b = from_html("https://example.com/notes", page).unwrap();
        assert!(b.chapters[0].text.starts_with("This page has no article"));
        assert!(!b.chapters[0].text.contains("Menu"));
    }

    #[test]
    fn a_page_with_nothing_to_read_says_so() {
        let page = "<html><body><nav>Home About Contact</nav></body></html>";
        assert!(from_html("https://example.com", page).is_err());
    }

    #[test]
    fn a_url_is_required_to_be_one() {
        assert!(import("not a url").is_err());
        assert!(import("file:///etc/passwd").is_err());
    }

    #[test]
    fn the_host_names_the_source() {
        assert_eq!(host_of("https://www.bbc.co.uk/news/x"), "bbc.co.uk");
        assert_eq!(host_of("http://example.com"), "example.com");
    }
}
