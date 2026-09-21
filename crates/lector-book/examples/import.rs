//! Imports any supported document and prints what would be read.
//!
//!   cargo run -p lector-book --example import -- book.epub
//!   cargo run -p lector-book --example import -- notes.md 2
//!   cargo run -p lector-book --example import -- https://example.com/article
//!
//! The point is to see the extraction before hearing it: a bad strip is obvious
//! in a second on screen and takes thirty seconds to notice by ear. For PDF
//! that is not a convenience but the actual import path -- see `pdf::preview`.

fn main() {
    let mut args = std::env::args().skip(1);
    let arg = args.next().expect("usage: import <path or url> [chapter]");
    let which: Option<usize> = args.next().and_then(|s| s.parse().ok());

    let book = if arg.starts_with("http") {
        lector_book::web::import(&arg).expect("fetch failed")
    } else {
        let p = std::path::Path::new(&arg);
        match p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "pdf" => {
                let x = lector_book::pdf::preview(p).expect("extraction failed");
                if !x.failed_pages.is_empty() {
                    eprintln!(
                        "{} of {} pages could not be read: {:?}",
                        x.failed_pages.len(),
                        x.total_pages,
                        &x.failed_pages[..x.failed_pages.len().min(10)]
                    );
                }
                lector_book::pdf::import(p, &x.text).expect("import failed")
            }
            "epub" => lector_book::epub::import(p).expect("import failed"),
            _ => lector_book::text::import(p).expect("import failed"),
        }
    };

    println!("{} — {}", book.title, book.author);
    println!(
        "{} chapters, {} words, {} warnings\n",
        book.chapters.len(),
        book.words(),
        book.warnings
    );

    match which {
        Some(i) => {
            let c = &book.chapters[i.min(book.chapters.len() - 1)];
            println!("== {} ==\n{}", c.title, c.text);
        }
        None => {
            for (i, c) in book.chapters.iter().enumerate() {
                let head: String = c.text.chars().take(90).collect();
                println!(
                    "{i:>3}  {:<40}  {}",
                    trunc(&c.title, 40),
                    head.replace('\n', " ⏎ ")
                );
            }
        }
    }
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).chain("…".chars()).collect()
    }
}
