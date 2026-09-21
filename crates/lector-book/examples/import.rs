//! Imports an EPUB and prints what would be read.
//!
//!   cargo run -p lector-book --example import -- book.epub [chapter]
//!
//! The point is to see the extraction before hearing it: a bad strip is obvious
//! in a second on screen and takes thirty seconds to notice by ear.

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: import <book.epub> [chapter]");
    let which: Option<usize> = args.next().and_then(|s| s.parse().ok());

    let book = lector_book::epub::import(std::path::Path::new(&path)).expect("import failed");
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
