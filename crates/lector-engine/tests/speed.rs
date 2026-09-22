//! Does asking for a different speed actually produce different audio?
//!
//! Ignored by default because it needs a model on disk. The app's speed
//! control passes a number through four layers to reach sherpa-onnx, and every
//! one of them is a place it can be dropped -- which is exactly what happened
//! once already, where the setting was stored but never reached the job in
//! flight.
//!
//!   cargo test -p lector-engine --test speed -- --ignored --nocapture

use sherpa_onnx::{GenerationConfig, OfflineTts, OfflineTtsConfig};

const TEXT: &str = "The quick brown fox jumps over the lazy dog, and then does it again.";

fn model_dir() -> Option<std::path::PathBuf> {
    ["models", "../models", "../../models"]
        .iter()
        .map(|d| std::path::Path::new(d).join("vits-piper-en_US-amy-medium-int8"))
        .find(|p| p.join("tokens.txt").exists())
}

fn samples_at(tts: &OfflineTts, speed: f32) -> usize {
    let cfg = GenerationConfig {
        sid: 0,
        speed,
        ..Default::default()
    };
    tts.generate_with_config(TEXT, &cfg, None::<fn(&[f32], f32) -> bool>)
        .expect("generation produced nothing")
        .samples()
        .len()
}

#[test]
#[ignore = "needs a model on disk"]
fn speed_changes_how_long_the_audio_is() {
    let Some(dir) = model_dir() else {
        eprintln!("no model installed; skipping");
        return;
    };
    let voice = lector_engine::Voice::from_dir(&dir, 0).expect("voice");
    let tts = OfflineTts::create(&OfflineTtsConfig {
        model: lector_engine::synth::model_config(&voice),
        ..Default::default()
    })
    .expect("tts");

    let slow = samples_at(&tts, 0.8);
    let normal = samples_at(&tts, 1.0);
    let fast = samples_at(&tts, 2.0);
    let rate = tts.sample_rate() as f64;
    println!(
        "0.8x {:.2}s   1.0x {:.2}s   2.0x {:.2}s",
        slow as f64 / rate,
        normal as f64 / rate,
        fast as f64 / rate
    );

    assert!(slow > normal, "0.8x should take longer than 1.0x");
    assert!(fast < normal, "2.0x should be shorter than 1.0x");
    // Roughly proportional, not exactly: the model's own leading and trailing
    // silence does not scale with the speaking rate.
    let ratio = normal as f64 / fast as f64;
    assert!(
        (1.6..2.4).contains(&ratio),
        "2.0x came out {ratio:.2}x shorter, which is not twice as fast"
    );
}
