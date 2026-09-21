//! End-to-end import, against EPUBs built in memory.
//!
//! The unit tests cover the parsers one file at a time; these cover the thing
//! that actually breaks, which is the three files disagreeing -- a manifest
//! that names a document the spine orders and the navigation titles.

use std::io::{Cursor, Write};

use lector_book::epub;
use zip::write::SimpleFileOptions;

/// Builds an EPUB from (path, contents) pairs. Stored rather than deflated, so
/// the test does not depend on a compression feature.
fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, body) in files {
        w.start_file(*name, opts).unwrap();
        w.write_all(body).unwrap();
    }
    w.finish().unwrap().into_inner()
}

const CONTAINER: &str = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;

fn opf(extra_manifest: &str, extra_spine: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<package xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0">
  <metadata>
    <dc:title>Bread &amp; Circuses</dc:title>
    <dc:creator>A Writer</dc:creator>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="c1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="text/ch%202.xhtml" media-type="application/xhtml+xml"/>
    {extra_manifest}
  </manifest>
  <spine>
    <itemref idref="c1"/>
    <itemref idref="c2"/>
    {extra_spine}
  </spine>
</package>"#
    )
}

const NAV: &str = r#"<html xmlns:epub="http://www.idpf.org/2007/ops"><body>
<nav epub:type="toc"><ol>
  <li><a href="text/ch1.xhtml">The First Chapter</a></li>
  <li><a href="text/ch%202.xhtml">The Second Chapter</a></li>
</ol></nav></body></html>"#;

fn book() -> Vec<u8> {
    zip_of(&[
        ("META-INF/container.xml", CONTAINER.as_bytes()),
        ("OEBPS/content.opf", opf("", "").as_bytes()),
        ("OEBPS/nav.xhtml", NAV.as_bytes()),
        (
            "OEBPS/text/ch1.xhtml",
            b"<html><body><h1>One</h1><p>It was a bright cold day.<sup><a href=\"#n\">1</a></sup></p>
              <aside epub:type=\"footnote\"><p>A note nobody asked to hear.</p></aside>
              <p>The clocks were striking thirteen.</p></body></html>",
        ),
        (
            "OEBPS/text/ch 2.xhtml",
            b"<html><body><p>Tom &amp; Jerry &mdash; friends.</p></body></html>",
        ),
    ])
}

#[test]
fn imports_metadata_chapters_and_titles() {
    let b = epub::from_bytes(&book()).expect("import");
    assert_eq!(b.title, "Bread & Circuses");
    assert_eq!(b.author, "A Writer");
    assert_eq!(b.chapters.len(), 2);
    assert_eq!(b.chapters[0].title, "The First Chapter");
    assert_eq!(b.chapters[1].title, "The Second Chapter");
}

#[test]
fn footnotes_are_not_spoken() {
    let b = epub::from_bytes(&book()).unwrap();
    let ch = &b.chapters[0].text;
    assert!(!ch.contains("nobody asked"), "{ch:?}");
    // The marker went with it, rather than leaving a stray "1" mid-sentence.
    assert!(ch.contains("bright cold day."), "{ch:?}");
    assert!(!ch.contains("day.1"), "{ch:?}");
    assert!(ch.contains("striking thirteen."), "{ch:?}");
}

#[test]
fn a_percent_escaped_filename_resolves() {
    let b = epub::from_bytes(&book()).unwrap();
    assert_eq!(b.chapters[1].text, "Tom & Jerry — friends.");
}

#[test]
fn a_spine_entry_with_no_file_costs_one_chapter_not_the_book() {
    let bytes = zip_of(&[
        ("META-INF/container.xml", CONTAINER.as_bytes()),
        (
            "OEBPS/content.opf",
            opf(
                r#"<item id="gone" href="text/missing.xhtml" media-type="application/xhtml+xml"/>"#,
                r#"<itemref idref="gone"/>"#,
            )
            .as_bytes(),
        ),
        ("OEBPS/nav.xhtml", NAV.as_bytes()),
        ("OEBPS/text/ch1.xhtml", b"<p>Still here.</p>"),
        ("OEBPS/text/ch 2.xhtml", b"<p>And here.</p>"),
    ]);
    let b = epub::from_bytes(&bytes).unwrap();
    assert_eq!(b.chapters.len(), 2);
    assert!(b.warnings >= 1, "the missing file should be reported");
}

#[test]
fn an_epub2_book_gets_its_titles_from_the_ncx() {
    let opf2 = r#"<?xml version="1.0"?>
<package xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0">
  <metadata><dc:title>Old Book</dc:title></metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="c1" href="ch1.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine toc="ncx"><itemref idref="c1"/></spine>
</package>"#;
    let ncx = r#"<ncx><navMap><navPoint>
      <navLabel><text>Chapter the First</text></navLabel>
      <content src="ch1.html"/>
    </navPoint></navMap></ncx>"#;
    let bytes = zip_of(&[
        ("META-INF/container.xml", CONTAINER.as_bytes()),
        ("OEBPS/content.opf", opf2.as_bytes()),
        ("OEBPS/toc.ncx", ncx.as_bytes()),
        ("OEBPS/ch1.html", b"<p>Call me Ishmael.</p>"),
    ]);
    let b = epub::from_bytes(&bytes).unwrap();
    assert_eq!(b.title, "Old Book");
    assert_eq!(b.chapters[0].title, "Chapter the First");
    assert_eq!(b.chapters[0].text, "Call me Ishmael.");
}

#[test]
fn a_file_that_is_not_an_epub_is_refused_rather_than_half_imported() {
    assert!(epub::from_bytes(b"this is not a zip").is_err());
    let empty = zip_of(&[("hello.txt", b"hi")]);
    assert!(epub::from_bytes(&empty).is_err());
}

#[test]
fn the_same_bytes_always_produce_the_same_id() {
    let b1 = epub::from_bytes(&book()).unwrap();
    let b2 = epub::from_bytes(&book()).unwrap();
    assert_eq!(b1.id, b2.id);
}
