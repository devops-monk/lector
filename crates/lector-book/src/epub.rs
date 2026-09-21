//! EPUB import.
//!
//! An EPUB is a zip containing `META-INF/container.xml`, which names an OPF
//! package document, which names everything else. Three files are parsed here:
//! the container (to find the OPF), the OPF (metadata, manifest, spine), and
//! whichever navigation document exists -- EPUB 3's XHTML `nav` or EPUB 2's
//! NCX. Both are read, because books in the wild ship one, the other, or a
//! broken version of each.
//!
//! The spine is the reading order and the only thing that decides what is read.
//! The navigation document contributes chapter *titles* and nothing else, so a
//! missing or malformed one costs names, never content.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::html::{self, entity};
use crate::{Book, Chapter};

type Zip = zip::ZipArchive<std::io::Cursor<Vec<u8>>>;

/// Imports an EPUB from a file.
pub fn import(path: &Path) -> Result<Book, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    from_bytes(&bytes)
}

/// Imports an EPUB already in memory.
///
/// The whole file is held at once. Books are single-digit megabytes and this is
/// a one-shot import, so streaming from disk would buy nothing and cost the
/// ability to hash the bytes for an id.
pub fn from_bytes(bytes: &[u8]) -> Result<Book, String> {
    let id = id_for(bytes);
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec()))
        .map_err(|e| format!("not a readable epub: {e}"))?;

    let opf_path = opf_path(&mut zip)?;
    let opf = read_text(&mut zip, &opf_path)
        .ok_or("the epub names a package file it does not contain")?;
    let pkg = parse_opf(&opf);

    let base = parent_of(&opf_path);
    // Titles are a lookup by document path, so whichever navigation document
    // exists can fill the same map and the spine loop need not care which.
    let titles = nav_titles(&mut zip, &pkg, &base);

    let mut chapters = Vec::new();
    let mut warnings = 0usize;
    for idref in &pkg.spine {
        let Some(item) = pkg.manifest.get(idref) else {
            continue;
        };
        if !item.media_type.contains("html") && !item.media_type.is_empty() {
            continue;
        }
        let path = resolve(&base, &item.href);
        let Some(xhtml) = read_text(&mut zip, &path) else {
            // A spine entry pointing at a missing file is a broken book, not a
            // reason to refuse the other nineteen chapters.
            warnings += 1;
            continue;
        };
        let out = html::to_text(&xhtml);
        warnings += out.errors;
        if out.text.trim().is_empty() {
            continue; // cover pages, blank title pages
        }
        let entries = titles.get(&path).map(Vec::as_slice).unwrap_or(&[]);
        split_chapters(&out, entries, &mut chapters);
    }

    if chapters.is_empty() {
        return Err("no readable text found in this epub".into());
    }

    Ok(Book {
        id,
        title: pkg.title.unwrap_or_else(|| "Untitled".into()),
        author: pkg.author.unwrap_or_default(),
        chapters,
        warnings,
    })
}

/// The cover image, if the book declares one.
///
/// Separate from [`from_bytes`] so importing a book does not drag every
/// illustration in it through memory to find one picture.
pub fn cover(bytes: &[u8]) -> Option<(String, Vec<u8>)> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).ok()?;
    let opf_path = opf_path(&mut zip).ok()?;
    let opf = read_text(&mut zip, &opf_path)?;
    let pkg = parse_opf(&opf);
    let base = parent_of(&opf_path);

    let item = pkg
        .cover_id
        .and_then(|id| pkg.manifest.get(&id).cloned())
        .or_else(|| {
            // EPUB 3 marks it with a property; EPUB 2 only ever did it by naming
            // things "cover", so that guess stays as the last resort.
            pkg.manifest
                .values()
                .find(|i| i.properties.contains("cover-image"))
                .or_else(|| {
                    pkg.manifest.values().find(|i| {
                        i.media_type.starts_with("image/")
                            && i.href.to_lowercase().contains("cover")
                    })
                })
                .cloned()
        })?;

    let path = resolve(&base, &item.href);
    let idx = index_of(&mut zip, &path)?;
    let mut buf = Vec::new();
    zip.by_index(idx).ok()?.read_to_end(&mut buf).ok()?;
    Some((item.media_type, buf))
}

/// A stable id for a file, used as its directory name.
///
/// The first megabyte plus the length rather than the whole file: an EPUB's
/// first megabyte contains its container, package and early chapters, so two
/// different books agreeing on it is not a realistic accident, and hashing 40 MB
/// of images to name a directory is not worth the wait.
pub fn id_for(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update((bytes.len() as u64).to_le_bytes());
    h.update(&bytes[..bytes.len().min(1 << 20)]);
    let d = h.finalize();
    d.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

// ---- the package document ------------------------------------------------

#[derive(Clone, Default, Debug)]
struct Item {
    href: String,
    media_type: String,
    properties: String,
}

#[derive(Default, Debug)]
struct Package {
    title: Option<String>,
    author: Option<String>,
    cover_id: Option<String>,
    manifest: HashMap<String, Item>,
    spine: Vec<String>,
    /// idref of the NCX, from the spine's `toc` attribute.
    ncx_id: Option<String>,
}

fn parse_opf(xml: &str) -> Package {
    let mut p = Package::default();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().check_end_names = false;
    // What element's text we are currently inside, and what it has said so
    // far. Accumulated rather than taken on the first text event: "Bread &amp;
    // Circuses" arrives as three events, and committing the first would put
    // "Bread" on the shelf.
    let mut field: Option<&'static str> = None;
    let mut buf = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = e.local_name();
                match name.as_ref() {
                    "title" if p.title.is_none() => {
                        field = Some("title");
                        buf.clear();
                    }
                    "creator" if p.author.is_none() => {
                        field = Some("author");
                        buf.clear();
                    }
                    "item" => {
                        let id = attr(&e, "id").unwrap_or_default();
                        if !id.is_empty() {
                            p.manifest.insert(
                                id,
                                Item {
                                    href: decode(&attr(&e, "href").unwrap_or_default()),
                                    media_type: attr(&e, "media-type").unwrap_or_default(),
                                    properties: attr(&e, "properties").unwrap_or_default(),
                                },
                            );
                        }
                    }
                    "itemref" => {
                        // linear="no" is the spec's way of saying "not part of
                        // the reading order" -- cover pages and ad pages.
                        if attr(&e, "linear").as_deref() != Some("no") {
                            if let Some(idref) = attr(&e, "idref") {
                                p.spine.push(idref);
                            }
                        }
                    }
                    "spine" => p.ncx_id = attr(&e, "toc"),
                    "meta" if attr(&e, "name").as_deref() == Some("cover") => {
                        p.cover_id = attr(&e, "content");
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(e)) if field.is_some() => buf.push_str(e.html_content().as_ref()),
            Ok(Event::GeneralRef(r)) if field.is_some() => {
                if let Some(t) = entity(&r) {
                    buf.push_str(&t);
                }
            }
            Ok(Event::End(_)) => {
                let Some(f) = field.take() else { continue };
                let t = buf.split_whitespace().collect::<Vec<_>>().join(" ");
                if t.is_empty() {
                    continue;
                }
                match f {
                    "title" => p.title = Some(t),
                    _ => p.author = Some(t),
                }
            }
            Ok(_) => {}
        }
    }
    p
}

// ---- navigation ----------------------------------------------------------

/// Cuts one document into chapters at the points its table of contents names.
///
/// A chapter is frequently not a file: Gutenberg ships sixty chapters in
/// fifteen documents and addresses them by fragment. Treating documents as
/// chapters would give a shelf of fifteen enormous entries named after
/// whatever caption happened to come first, so the fragments do the cutting.
fn split_chapters(
    out: &html::Extracted,
    entries: &[(Option<String>, String)],
    into: &mut Vec<Chapter>,
) {
    let mut cuts: Vec<(usize, String)> = entries
        .iter()
        .filter_map(|(frag, label)| match frag {
            None => Some((0, label.clone())),
            Some(f) => out.anchors.get(f).map(|at| (*at, label.clone())),
        })
        .collect();
    cuts.sort_by_key(|(at, _)| *at);
    cuts.dedup_by_key(|(at, _)| *at);

    // Text before the first named cut still has to go somewhere. It is usually
    // a heading or a frontispiece, and dropping it would lose content silently.
    if cuts.first().map(|(at, _)| *at) != Some(0) {
        let head = cuts.first().map(|(at, _)| *at).unwrap_or(out.text.len());
        let lead = out.text[..head].trim();
        if !lead.is_empty() {
            cuts.insert(0, (0, first_line_title(lead, into.len())));
        }
    }
    if cuts.is_empty() {
        cuts.push((0, first_line_title(&out.text, into.len())));
    }

    for i in 0..cuts.len() {
        let start = cuts[i].0;
        let end = cuts.get(i + 1).map(|(at, _)| *at).unwrap_or(out.text.len());
        let text = out.text[start..end].trim();
        if text.is_empty() {
            continue;
        }
        into.push(Chapter {
            title: cuts[i].1.clone(),
            text: text.to_string(),
        });
    }
}

/// Table of contents entries, keyed by the zip path of the document they point
/// into, in the order the navigation lists them.
///
/// Both navigation formats are tried and merged, EPUB 3's first. A book that
/// ships both usually has a better nav than NCX, and a book that ships a broken
/// one still gets titles from the other.
type Toc = HashMap<String, Vec<(Option<String>, String)>>;

fn nav_titles(zip: &mut Zip, pkg: &Package, base: &str) -> Toc {
    let mut out = Toc::new();

    if let Some(item) = pkg.manifest.values().find(|i| i.properties.contains("nav")) {
        let path = resolve(base, &item.href);
        if let Some(xml) = read_text(zip, &path) {
            collect_links(&xml, &parent_of(&path), &mut out);
        }
    }

    let ncx = pkg
        .ncx_id
        .as_ref()
        .and_then(|id| pkg.manifest.get(id))
        .or_else(|| {
            pkg.manifest
                .values()
                .find(|i| i.media_type.contains("dtbncx"))
        });
    if let Some(item) = ncx {
        let path = resolve(base, &item.href);
        if let Some(xml) = read_text(zip, &path) {
            collect_ncx(&xml, &parent_of(&path), &mut out);
        }
    }

    out
}

/// EPUB 3 nav: `<a href="ch1.xhtml">Chapter One</a>`.
///
/// Only the contents nav is read. The same document usually also carries a
/// `page-list`, and taking that too turns a novel into one chapter per printed
/// page -- Pride and Prejudice becomes 477 chapters named `{vii}` through
/// `{476}`. A document with no typed nav at all is read whole, since then there
/// is nothing else it could be.
fn collect_links(xml: &str, base: &str, out: &mut Toc) {
    let typed = xml.contains("type=\"toc\"");
    let mut reader = Reader::from_str(xml);
    reader.config_mut().check_end_names = false;
    let mut pending: Option<(String, String)> = None;
    let mut collecting = !typed && !xml.contains("<nav");

    loop {
        match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(Event::Start(e)) if e.local_name().as_ref() == "nav" => {
                let kind = attr(&e, "type").unwrap_or_default().to_ascii_lowercase();
                collecting = if typed {
                    kind.split_whitespace().any(|k| k == "toc")
                } else {
                    // No contents nav declared: take the first nav that is not
                    // one of the machine-readable ones.
                    !kind
                        .split_whitespace()
                        .any(|k| matches!(k, "page-list" | "landmarks" | "lot" | "loi" | "lov"))
                };
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "nav" => collecting = false,
            Ok(Event::Start(e)) if collecting && e.local_name().as_ref() == "a" => {
                if let Some(href) = attr(&e, "href") {
                    pending = Some((decode(&href), String::new()));
                }
            }
            Ok(Event::Text(e)) => {
                if let Some((_, label)) = pending.as_mut() {
                    label.push_str(e.html_content().as_ref());
                }
            }
            Ok(Event::GeneralRef(r)) => {
                // "Bread &amp; Circuses" is a chapter title, and dropping the
                // ampersand would be visible on the shelf.
                if let (Some((_, label)), Some(t)) = (pending.as_mut(), entity(&r)) {
                    label.push_str(&t);
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "a" => {
                if let Some((href, label)) = pending.take() {
                    let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !label.is_empty() {
                        add(out, base, &href, label);
                    }
                }
            }
            Ok(_) => {}
        }
    }
}

/// EPUB 2 NCX: `<navPoint><navLabel><text>…</text></navLabel><content src="…"/></navPoint>`.
///
/// Only `navMap` is read. An NCX usually also carries a `pageList` of printed
/// page numbers -- Gutenberg's Pride and Prejudice has 906 of them against 126
/// real navigation points -- and reading those as chapters turns a novel into a
/// list of page numbers.
///
/// The label precedes the target, so the label is held until a `content`
/// element says what it names.
fn collect_ncx(xml: &str, base: &str, out: &mut Toc) {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().check_end_names = false;
    let mut label = String::new();
    let mut in_text = false;
    let mut in_map = false;

    loop {
        match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(Event::Start(e)) if e.local_name().as_ref() == "navMap" => in_map = true,
            Ok(Event::End(e)) if e.local_name().as_ref() == "navMap" => in_map = false,
            Ok(Event::Start(e)) if in_map && e.local_name().as_ref() == "text" => {
                in_text = true;
                label.clear();
            }
            Ok(Event::Text(e)) if in_text => label.push_str(e.html_content().as_ref()),
            Ok(Event::GeneralRef(r)) if in_text => {
                if let Some(t) = entity(&r) {
                    label.push_str(&t);
                }
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == "text" => in_text = false,
            Ok(Event::Start(e)) | Ok(Event::Empty(e))
                if in_map && e.local_name().as_ref() == "content" =>
            {
                let Some(src) = attr(&e, "src") else { continue };
                let l = label.split_whitespace().collect::<Vec<_>>().join(" ");
                if !l.is_empty() {
                    add(out, base, &decode(&src), l);
                }
            }
            Ok(_) => {}
        }
    }
}

/// Files one entry under the document it points into, keeping its fragment.
///
/// Duplicate targets are dropped rather than appended: books list the same
/// chapter in both a landmarks section and the contents, and a chapter that
/// appeared twice on the shelf would be read twice.
fn add(out: &mut Toc, base: &str, href: &str, label: String) {
    let path = resolve(base, strip_fragment(href));
    let frag = href
        .split_once('#')
        .map(|(_, f)| f.to_string())
        .filter(|f| !f.is_empty());
    let entries = out.entry(path).or_default();
    if entries.iter().any(|(f, _)| *f == frag) {
        return;
    }
    entries.push((frag, label));
}

// ---- zip and paths -------------------------------------------------------

fn opf_path(zip: &mut Zip) -> Result<String, String> {
    let container =
        read_text(zip, "META-INF/container.xml").ok_or("not an epub: no META-INF/container.xml")?;
    let mut reader = Reader::from_str(&container);
    reader.config_mut().check_end_names = false;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) if e.local_name().as_ref() == "rootfile" => {
                if let Some(p) = attr(&e, "full-path") {
                    return Ok(decode(&p));
                }
            }
            Ok(_) => {}
        }
    }
    Err("not an epub: the container names no package document".into())
}

/// Reads a file from the zip as text.
///
/// Falls back to a case-insensitive scan: zip paths are case-sensitive and some
/// EPUB writers disagree with their own manifest about capitalisation, which
/// would otherwise lose a chapter for a reason nobody could guess.
fn read_text(zip: &mut Zip, path: &str) -> Option<String> {
    let idx = index_of(zip, path)?;
    let mut buf = Vec::new();
    zip.by_index(idx).ok()?.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Locates a file in the archive, exactly and then case-insensitively.
///
/// Zip paths are case-sensitive and some EPUB writers disagree with their own
/// manifest about capitalisation, which would otherwise lose a chapter for a
/// reason nobody could guess from the outside.
fn index_of(zip: &mut Zip, path: &str) -> Option<usize> {
    if let Some(i) = zip.index_for_name(path) {
        return Some(i);
    }
    let want = path.to_ascii_lowercase();
    (0..zip.len()).find(|i| {
        zip.by_index_raw(*i)
            .map(|f| f.name().to_ascii_lowercase() == want)
            .unwrap_or(false)
    })
}

fn parent_of(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

/// Joins an href to the directory holding the document that referenced it,
/// resolving `.` and `..` the way a URL would.
fn resolve(base: &str, href: &str) -> String {
    if href.starts_with('/') {
        return href.trim_start_matches('/').to_string();
    }
    let mut parts: Vec<&str> = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/').collect()
    };
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn strip_fragment(href: &str) -> &str {
    href.split('#').next().unwrap_or(href)
}

/// Percent-decoding, because hrefs are URLs and chapter files are allowed
/// spaces in their names.
fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn attr(e: &quick_xml::events::BytesStart, want: &str) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        a.key
            .local_name()
            .as_ref()
            .eq_ignore_ascii_case(want)
            .then(|| a.value.clone().into_owned())
    })
}

/// A last-resort chapter name, taken from the text itself.
///
/// A chapter with no title in the navigation almost always begins with its
/// heading, so the first short line is a better name than "Section 7".
fn first_line_title(text: &str, index: usize) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && l.chars().count() <= 60)
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("Section {}", index + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_hrefs() {
        assert_eq!(resolve("OEBPS", "ch1.xhtml"), "OEBPS/ch1.xhtml");
        assert_eq!(
            resolve("OEBPS/text", "../images/c.jpg"),
            "OEBPS/images/c.jpg"
        );
        assert_eq!(resolve("OEBPS", "./ch1.xhtml"), "OEBPS/ch1.xhtml");
        assert_eq!(resolve("", "content.opf"), "content.opf");
        assert_eq!(resolve("OEBPS", "/abs.xhtml"), "abs.xhtml");
    }

    #[test]
    fn decodes_percent_escapes_in_filenames() {
        assert_eq!(decode("chapter%201.xhtml"), "chapter 1.xhtml");
        assert_eq!(decode("plain.xhtml"), "plain.xhtml");
        // A stray percent is left alone rather than swallowing the next bytes.
        assert_eq!(decode("100%.xhtml"), "100%.xhtml");
    }

    #[test]
    fn fragments_do_not_become_part_of_the_path() {
        assert_eq!(strip_fragment("ch1.xhtml#s2"), "ch1.xhtml");
        assert_eq!(strip_fragment("ch1.xhtml"), "ch1.xhtml");
    }

    #[test]
    fn parses_a_package_document() {
        let opf = r#"<package xmlns:dc="http://purl.org/dc/elements/1.1/">
          <metadata>
            <dc:title>A Book</dc:title>
            <dc:creator>An Author</dc:creator>
            <meta name="cover" content="cov"/>
          </metadata>
          <manifest>
            <item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
            <item id="cov" href="cover.jpg" media-type="image/jpeg"/>
            <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
          </manifest>
          <spine toc="ncx">
            <itemref idref="cover-page" linear="no"/>
            <itemref idref="c1"/>
          </spine>
        </package>"#;
        let p = parse_opf(opf);
        assert_eq!(p.title.as_deref(), Some("A Book"));
        assert_eq!(p.author.as_deref(), Some("An Author"));
        assert_eq!(p.cover_id.as_deref(), Some("cov"));
        assert_eq!(p.ncx_id.as_deref(), Some("ncx"));
        // linear="no" is not part of the reading order.
        assert_eq!(p.spine, vec!["c1"]);
        assert_eq!(p.manifest["c1"].href, "ch1.xhtml");
    }

    #[test]
    fn reads_titles_from_an_ncx() {
        let ncx = r#"<ncx><navMap>
          <navPoint><navLabel><text>Chapter One</text></navLabel><content src="ch1.xhtml"/></navPoint>
          <navPoint><navLabel><text>Chapter Two</text></navLabel><content src="ch2.xhtml#start"/></navPoint>
        </navMap></ncx>"#;
        let mut m = Toc::new();
        collect_ncx(ncx, "OEBPS", &mut m);
        assert_eq!(
            m["OEBPS/ch1.xhtml"],
            vec![(None, "Chapter One".to_string())]
        );
        assert_eq!(
            m["OEBPS/ch2.xhtml"],
            vec![(Some("start".to_string()), "Chapter Two".to_string())]
        );
    }

    #[test]
    fn reads_titles_from_an_epub3_nav() {
        let nav = r#"<html><body><nav epub:type="toc"><ol>
          <li><a href="ch1.xhtml">One</a></li>
          <li><a href="ch2.xhtml#x">  Two  </a></li>
        </ol></nav></body></html>"#;
        let mut m = Toc::new();
        collect_links(nav, "OEBPS", &mut m);
        assert_eq!(m["OEBPS/ch1.xhtml"], vec![(None, "One".to_string())]);
        assert_eq!(
            m["OEBPS/ch2.xhtml"],
            vec![(Some("x".to_string()), "Two".to_string())]
        );
    }

    #[test]
    fn an_id_is_stable_and_distinguishes_files() {
        let a = id_for(b"the same bytes");
        assert_eq!(a, id_for(b"the same bytes"));
        assert_ne!(a, id_for(b"other bytes"));
        assert_eq!(a.len(), 16);
    }
}
