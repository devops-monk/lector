//! Searches the free catalogues and optionally downloads the first result.
//!
//!   cargo run -p lector-book --example browse -- austen
//!   cargo run -p lector-book --example browse -- --new
//!   cargo run -p lector-book --example browse -- austen --get

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = args.iter().any(|a| a == "--get");
    let query: String = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");

    let found = if args.iter().any(|a| a == "--new") {
        lector_book::catalogue::new_releases().expect("feed")
    } else {
        lector_book::catalogue::search(&query).expect("search")
    };

    println!("{} results\n", found.len());
    for (i, l) in found.iter().enumerate().take(12) {
        println!("{i:>3}  {:<44}  {}", cut(&l.title, 44), cut(&l.author, 28));
    }

    if get {
        let first = found.first().expect("nothing to download");
        println!("\ndownloading {} …", first.title);
        let (book, bytes) = lector_book::catalogue::fetch(first, |got, total| {
            if total > 0 && got % (256 * 1024) < 64 * 1024 {
                println!("  {}%", got * 100 / total);
            }
        })
        .expect("download");
        println!(
            "got {} KB: {} — {} ({} chapters, {} words)",
            bytes.len() / 1024,
            book.title,
            book.author,
            book.chapters.len(),
            book.words()
        );
    }
}

fn cut(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).chain("…".chars()).collect()
    }
}
