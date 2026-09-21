//! Does the position clock tell the truth?
//!
//! The whole highlight design rests on one claim: a mark fires when the sound
//! card has *rendered* the frames before it, not when the synthesizer produced
//! them. Synthesis runs several chunks ahead, so if this is wrong the highlight
//! sits ahead of the voice and everything built on it is wrong too.
//!
//! This prints, for each chunk, when it was synthesized and when it was
//! reported audible. The second column must lag the first, and must line up
//! with what you hear.
//!
//!   cargo run -p lector-engine --bin clock

use std::sync::mpsc;
use std::time::Instant;

use lector_engine::{Lector, Position, Voice};
use lector_text::SanitizeOptions;

const MODEL: &str = "models/vits-piper-en_US-amy-medium-int8";

const TEXT: &str = "This is the first chunk, and it should be highlighted first. \
     Here is a second chunk, which follows the first one after a short gap. \
     The third chunk arrives later still, and by now synthesis is running well ahead of playback. \
     A fourth chunk proves the clock has not drifted. \
     The fifth and final chunk should be reported as audible only when you actually hear these words.";

fn main() {
    let voice = Voice::from_dir(std::path::Path::new(MODEL), 0).expect("voice");
    let (tx, rx) = mpsc::channel::<(Position, f64)>();

    let start = Instant::now();
    let t0 = start;
    let lector = Lector::with_voice_and_position(voice, move |p| {
        let _ = tx.send((p, t0.elapsed().as_secs_f64()));
    })
    .expect("engine");

    let chunks = lector_text::prepare(TEXT, SanitizeOptions::READING, false);
    println!("{} chunks\n", chunks.len());
    for (i, c) in chunks.iter().enumerate() {
        println!("  [{i}] {}", &c[..c.len().min(70)]);
    }
    println!();

    let spoke_at = Instant::now();
    lector.speak(TEXT, SanitizeOptions::READING, false);

    println!("{:>6}  {:>10}  text", "chunk", "audible at");
    println!("{}", "-".repeat(64));
    let mut seen = 0;
    while seen < chunks.len() {
        match rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok((p, at)) => {
                let c = &chunks[p.index];
                println!(
                    "{:>6}  {:>9.2}s  {}",
                    format!("{}/{}", p.index + 1, p.total),
                    at,
                    &c[..c.len().min(46)]
                );
                seen += 1;
            }
            Err(_) => {
                println!("  timed out after {seen} marks");
                break;
            }
        }
    }

    // The verdict, computed rather than eyeballed.
    //
    // Synthesis runs at roughly RTF 0.15, so if marks fired when a chunk was
    // *generated* the whole run would be over in a couple of seconds. Spacing
    // that matches speech instead is what proves the clock counts rendered
    // frames. Anything under ~2s per chunk means the marks are running ahead of
    // the voice and the highlight will too.
    let elapsed = spoke_at.elapsed().as_secs_f64();
    let per_chunk = elapsed / chunks.len() as f64;
    println!(
        "\n{elapsed:.2}s for {} chunks = {per_chunk:.2}s each",
        chunks.len()
    );
    if per_chunk > 2.0 {
        println!("PASS: paced by playback, not synthesis");
    } else {
        println!("FAIL: too fast to be real playback -- marks are firing at synthesis time");
    }
    std::thread::sleep(std::time::Duration::from_secs(4));

    println!("\n=== pause/resume ===");
    lector.speak(TEXT, SanitizeOptions::READING, false);
    std::thread::sleep(std::time::Duration::from_millis(2500));
    lector.pause();
    println!("  paused (should be silent for 3s)");
    std::thread::sleep(std::time::Duration::from_secs(3));
    println!("  resuming -- it must continue mid-sentence, not restart");
    lector.resume();
    std::thread::sleep(std::time::Duration::from_secs(6));
    lector.stop();
    println!("  done");
}
