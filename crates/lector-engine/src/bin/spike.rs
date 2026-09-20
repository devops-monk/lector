//! Phase-0 spike. Proves the four facts the whole plan rests on:
//!
//!   1. sherpa-onnx builds and runs here with no Python and no espeak subprocess
//!   2. `generate_with_config`'s callback really does fire per sentence, so audio
//!      can start before synthesis finishes
//!   3. returning `false` from that callback aborts mid-utterance (cancellation)
//!   4. a long-lived cpal stream can be fed from it
//!
//! It also prints the numbers the chunk sizes in the plan depend on: model load,
//! time-to-first-sample, and RTF. Verba measured RTF 0.14 for this Piper voice
//! and 0.41 for Kokoro fp32 on Apple silicon; a large miss here means something
//! is wrong before anything gets built on top.
//!
//!   cargo run -p lector-engine --bin spike -- "some text to speak"

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use sherpa_onnx::{GenerationConfig, OfflineTts, OfflineTtsConfig};

/// Override with LECTOR_MODEL to measure a different model, and LECTOR_SID to
/// pick a speaker within it.
fn model_dir() -> String {
    std::env::var("LECTOR_MODEL")
        .unwrap_or_else(|_| "models/vits-piper-en_US-amy-medium-int8".into())
}
fn sid() -> i32 {
    std::env::var("LECTOR_SID")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // --cancel proves fact 3: returning false from the callback aborts the
    // utterance mid-flight rather than at the end of the current sentence.
    let cancel_after = args.iter().position(|a| a == "--cancel").map(|i| {
        args.get(i + 1)
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(1)
    });
    let text: String = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .filter(|a| a.parse::<usize>().is_err())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let text = if text.trim().is_empty() {
        "Lector is running entirely on your machine. \
         Nothing you select will ever leave this laptop. \
         This sentence is being spoken while the next one is still being generated."
            .to_string()
    } else {
        text
    };

    // ---- load -------------------------------------------------------------
    // No voices.bin in this directory, so it is a VITS/Piper model rather than
    // Kokoro. That single check is the whole engine discriminator.
    let md = model_dir();
    let dir = std::path::Path::new(&md);
    assert!(dir.join("tokens.txt").exists(), "model not found at {md}");
    let t0 = Instant::now();
    let voice = lector_engine::Voice::from_dir(dir, sid()).expect("voice");
    let cfg = OfflineTtsConfig {
        model: lector_engine::synth::model_config(&voice),
        ..Default::default()
    };
    let tts = OfflineTts::create(&cfg).expect("failed to create OfflineTts");
    let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let model_rate = tts.sample_rate() as u32;
    println!(
        "model loaded in {load_ms:.0} ms  |  rate {model_rate} Hz  |  speakers {}",
        tts.num_speakers()
    );

    // ---- audio out --------------------------------------------------------
    let host = cpal::default_host();
    let device = host.default_output_device().expect("no output device");
    let default_cfg = device.default_output_config().expect("no default config");
    let device_rate = default_cfg.sample_rate().0;
    let channels = default_cfg.channels() as usize;
    println!(
        "output: {} @ {device_rate} Hz, {channels} ch",
        device.name().unwrap_or_default()
    );

    let rb = ringbuf::HeapRb::<f32>::new(device_rate as usize * 30);
    let (mut prod, mut cons) = {
        use ringbuf::traits::Split;
        rb.split()
    };

    // Counts frames the hardware has actually consumed. In the real player this
    // is the position clock that decides when a highlight mark becomes audible.
    let played = Arc::new(AtomicUsize::new(0));
    let starved = Arc::new(AtomicBool::new(false));
    // Only a starve *during* synthesis means we fell behind. The last buffer of
    // an utterance is always partially filled, and flagging that would report a
    // real-time failure on every successful run.
    let generating = Arc::new(AtomicBool::new(true));
    let (played_cb, starved_cb, generating_cb) =
        (played.clone(), starved.clone(), generating.clone());

    let stream = device
        .build_output_stream(
            &default_cfg.config(),
            move |out: &mut [f32], _| {
                use ringbuf::traits::Consumer;
                let frames = out.len() / channels;
                let mut got = 0usize;
                for f in 0..frames {
                    match cons.try_pop() {
                        Some(s) => {
                            for c in 0..channels {
                                out[f * channels + c] = s;
                            }
                            got += 1;
                        }
                        // Underrun: write silence, never garbage. The stream stays
                        // open for the app's lifetime rather than stopping.
                        None => {
                            for c in 0..channels {
                                out[f * channels + c] = 0.0;
                            }
                        }
                    }
                }
                if got < frames && got > 0 && generating_cb.load(Ordering::Relaxed) {
                    starved_cb.store(true, Ordering::Relaxed);
                }
                played_cb.fetch_add(got, Ordering::Relaxed);
            },
            |e| eprintln!("stream error: {e}"),
            None,
        )
        .expect("failed to build output stream");
    stream.play().expect("failed to start stream");

    // ---- synthesize, streaming ------------------------------------------
    let first_sample_at: Arc<Mutex<Option<f64>>> = Arc::new(Mutex::new(None));
    let total_samples = Arc::new(AtomicUsize::new(0));
    let callbacks = Arc::new(AtomicUsize::new(0));
    // Frames actually pushed to the ring. Must be counted, not recomputed: we push
    // floor(n*ratio) per chunk, and a sum of floors is less than floor of the sum.
    let pushed = Arc::new(AtomicUsize::new(0));

    let (fs_cb, ts_cb, cb_cb, push_cb) = (
        first_sample_at.clone(),
        total_samples.clone(),
        callbacks.clone(),
        pushed.clone(),
    );
    let t_gen = Instant::now();
    let ratio = device_rate as f64 / model_rate as f64;

    let cb = move |samples: &[f32], progress: f32| -> bool {
        // The C++ side frees this buffer when we return, so copy before anything
        // else. (offline-tts-*-impl.h warns about exactly this.)
        let n = samples.len();
        cb_cb.fetch_add(1, Ordering::Relaxed);
        ts_cb.fetch_add(n, Ordering::Relaxed);
        if fs_cb.lock().unwrap().is_none() {
            *fs_cb.lock().unwrap() = Some(t_gen.elapsed().as_secs_f64() * 1000.0);
        }

        // Linear resample, model rate -> device rate. Deliberately crude: the real
        // player uses rubato with one reusable sinc resampler per rate pair. This
        // is here only so the spike can be heard.
        use ringbuf::traits::Producer;
        let out_len = (n as f64 * ratio) as usize;
        for i in 0..out_len {
            let src = i as f64 / ratio;
            let j = src.floor() as usize;
            let frac = (src - j as f64) as f32;
            let a = samples.get(j).copied().unwrap_or(0.0);
            let b = samples.get(j + 1).copied().unwrap_or(a);
            if prod.try_push(a + (b - a) * frac).is_ok() {
                push_cb.fetch_add(1, Ordering::Relaxed);
            }
        }
        println!(
            "  chunk {:>2}  {:>6} samples ({:>5.2}s)  progress {:>5.1}%  @ {:>6.0} ms",
            cb_cb.load(Ordering::Relaxed),
            n,
            n as f64 / model_rate as f64,
            progress * 100.0,
            t_gen.elapsed().as_secs_f64() * 1000.0
        );
        // Fact 3: this is the whole cancellation mechanism. In the real engine
        // the bool is `generation.load() == my_generation`, so Stop cuts here,
        // mid-utterance, instead of at the end of the current sentence.
        match cancel_after {
            Some(limit) if cb_cb.load(Ordering::Relaxed) >= limit => {
                println!("  -> cancelling after chunk {limit}");
                false
            }
            _ => true,
        }
    };

    let gen_cfg = GenerationConfig {
        sid: 0,
        speed: 1.0,
        ..Default::default()
    };
    let audio = tts.generate_with_config(&text, &gen_cfg, Some(cb));
    let gen_s = t_gen.elapsed().as_secs_f64();
    generating.store(false, Ordering::Relaxed);
    assert!(audio.is_some(), "generation returned nothing");

    let n = total_samples.load(Ordering::Relaxed);
    let audio_s = n as f64 / model_rate as f64;
    let ttfs = first_sample_at.lock().unwrap().unwrap_or(f64::NAN);

    println!("\n--- results ---");
    println!("model load          {load_ms:>8.0} ms");
    println!("time to 1st sample  {ttfs:>8.0} ms   <- what the hotkey latency feels like");
    println!(
        "audio produced      {audio_s:>8.2} s  in {} callbacks",
        callbacks.load(Ordering::Relaxed)
    );
    println!("synthesis took      {gen_s:>8.2} s");
    println!(
        "RTF                 {:>8.2}      (verba: 0.14 piper int8, 0.41 kokoro fp32)",
        gen_s / audio_s
    );

    // Drain whatever the hardware has not consumed yet, with a deadline so a
    // silent device can never hang the spike.
    let want = pushed.load(Ordering::Relaxed);
    let deadline = Instant::now() + std::time::Duration::from_secs_f64(audio_s + 10.0);
    while played.load(Ordering::Relaxed) < want && Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let done = played.load(Ordering::Relaxed);
    if done < want {
        println!("\nWARNING: drained only {done}/{want} frames before the deadline.");
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    if starved.load(Ordering::Relaxed) {
        println!("\nNOTE: the ring starved mid-synthesis - generation fell behind playback.");
    }
}
