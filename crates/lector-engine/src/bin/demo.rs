//! Exercises the engine the way the hotkey surface will.
//!
//!   cargo run -p lector-engine --bin demo             -- speak, then interrupt
//!   cargo run -p lector-engine --bin demo -- "text"   -- speak one thing

use std::time::{Duration, Instant};

use lector_engine::Lector;
use lector_text::SanitizeOptions;

const MODEL: &str = "models/vits-piper-en_US-amy-medium-int8";

fn main() {
    let arg: Vec<String> = std::env::args().skip(1).collect();

    let t0 = Instant::now();
    let lector = Lector::new(std::path::Path::new(MODEL)).expect("engine");
    println!(
        "engine ready in {} ms (device open + model warm)\n",
        t0.elapsed().as_millis()
    );

    if !arg.is_empty() {
        lector.speak(&arg.join(" "), SanitizeOptions::NARRATION, true);
        wait_until_quiet(&lector);
        return;
    }

    // 1. Latency: how long until the first sound?
    let t = Instant::now();
    lector.speak(
        "Lector is running entirely on your machine. \
         This is the second sentence, and it was still being generated when the first one started playing. \
         The third sentence exists mostly so there is something left to interrupt.",
        SanitizeOptions::NARRATION,
        true,
    );
    while lector.level() == 0.0 && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(2));
    }
    println!("first audible sample at {} ms", t.elapsed().as_millis());

    // 2. Interrupt: a second request must cut the first mid-word.
    std::thread::sleep(Duration::from_millis(1200));
    println!("interrupting...");
    let t = Instant::now();
    lector.speak("Interrupted.", SanitizeOptions::NARRATION, true);
    std::thread::sleep(Duration::from_millis(50));
    println!(
        "  new utterance accepted after {} ms",
        t.elapsed().as_millis()
    );
    wait_until_quiet(&lector);

    // 3. Stop: silence must be immediate, not at the end of the sentence.
    println!("\nstop test: speaking, then stopping after 700 ms");
    lector.speak(
        "This sentence is deliberately long so that stopping it partway through is clearly audible to anyone listening.",
        SanitizeOptions::NARRATION,
        true,
    );
    std::thread::sleep(Duration::from_millis(700));
    let t = Instant::now();
    lector.stop();
    let mut quiet_at = None;
    while t.elapsed() < Duration::from_secs(2) {
        if lector.level() == 0.0 {
            quiet_at = Some(t.elapsed());
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    match quiet_at {
        Some(d) => println!("  silent {} ms after stop", d.as_millis()),
        None => println!("  STILL PLAYING 2s after stop -- the cut did not take"),
    }
    std::thread::sleep(Duration::from_millis(300));
}

fn wait_until_quiet(l: &Lector) {
    std::thread::sleep(Duration::from_millis(300));
    while l.is_speaking() {
        std::thread::sleep(Duration::from_millis(50));
    }
    // Let the ring drain.
    let t = Instant::now();
    while l.level() > 0.0 && t.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(200));
}
