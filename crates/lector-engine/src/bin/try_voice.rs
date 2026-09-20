//! Loads any catalogued voice and speaks a line with it.
//!
//! Exists because each engine family loads differently, and the only way to know
//! a family actually works is to hear it.
//!
//!   cargo run -p lector-engine --bin try_voice -- <model-id> [sid]

use std::time::Instant;

use lector_engine::{
    catalog,
    install::{install, Cancel},
    Lector, Voice,
};
use lector_text::SanitizeOptions;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let id = args.first().cloned().unwrap_or_else(|| {
        eprintln!("models:");
        for m in catalog::CATALOG {
            eprintln!(
                "  {:<50} {:>4} MB  {} speaker(s)",
                m.id,
                m.mb,
                m.speakers.len()
            );
        }
        std::process::exit(1);
    });
    let sid: i32 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(-1);

    let m = catalog::model(&id).expect("unknown model id");
    let dir = install(std::path::Path::new("models"), m, &Cancel::new(), |p| {
        if let Some(pct) = p
            .received
            .checked_mul(100)
            .and_then(|n| n.checked_div(p.total))
        {
            print!("\r  {pct}%   ");
            use std::io::Write;
            let _ = std::io::stdout().flush();
        }
    })
    .expect("install");
    println!("\n{} at {}", m.label, dir.display());

    let sp = if sid >= 0 {
        m.speakers
            .iter()
            .find(|s| s.sid == sid)
            .expect("no such speaker")
    } else {
        &m.speakers[0]
    };
    let voice = Voice::from_dir_for(&dir, m.engine, sp.sid, Some(sp.name)).expect("voice");
    println!(
        "engine={:?} sid={} espeak={} lexicon={} voices.bin={}",
        voice.engine,
        voice.sid,
        voice.data_dir.is_some(),
        voice.lexicon.is_some(),
        voice.voices_bin.is_some()
    );

    let t = Instant::now();
    let l = Lector::with_voice(voice).expect("engine");
    println!("ready in {:?}", t.elapsed());

    let t = Instant::now();
    l.speak(catalog::AUDITION, SanitizeOptions::READING, true);
    while l.level() == 0.0 && t.elapsed().as_secs() < 20 {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    println!("first audio at {} ms", t.elapsed().as_millis());
    std::thread::sleep(std::time::Duration::from_secs(5));
}
