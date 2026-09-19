fn main() {
    let md = "# Phase 1 notes\n\nThe engine loads in about 820 ms, so it must be warmed at launch. \
Dr. Smith measured an RTF of 0.15 on the M1 Pro, i.e. well ahead of realtime.\n\n\
- First chunk goes out alone.\n- Everything after merges to a floor.\n\n\
```rust\nfn main() {}\n```\n\n\
See https://www.example.com/docs/a/b for the full table.\n\n\
| a | b |\n|---|---|\n| 1 | 2 |\n";
    for (i, c) in lector_text::prepare(md, lector_text::SanitizeOptions::NARRATION, true).iter().enumerate() {
        println!("[{i}] {:>3} chars | {c}", c.chars().count());
    }
}
