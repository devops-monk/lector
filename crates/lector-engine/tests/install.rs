//! The installer's contract is negative: when anything goes wrong, nothing is
//! installed. These tests assert the absence of things, which is the part that
//! actually protects the user.

use std::path::Path;

use lector_engine::catalog::{Model, Speaker};
use lector_engine::install::{install, is_installed, Cancel, Phase};
use lector_engine::voice::Engine;

const SPEAKERS: &[Speaker] = &[Speaker {
    name: "amy",
    sid: 0,
    note: "",
}];

fn tmpdir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("lector-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Points at a real, tiny archive but declares a hash it cannot have.
fn wrong_hash_model() -> Model {
    Model {
        id: "vits-piper-en_US-amy-medium-int8",
        engine: Engine::Piper,
        label: "Piper",
        accent: "American",
        mb: 21,
        url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-piper-en_US-amy-medium-int8.tar.bz2",
        sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        speakers: SPEAKERS,
        recommended: false,
        tradeoff: "",
    }
}

#[test]
#[ignore = "downloads 21 MB; run with --ignored"]
fn a_bad_checksum_installs_nothing() {
    let root = tmpdir("badhash");
    let m = wrong_hash_model();

    let err = install(&root, &m, &Cancel::new(), |_| {}).unwrap_err();
    assert!(err.contains("checksum"), "unexpected error: {err}");

    assert!(
        !is_installed(&root, &m),
        "a failed install left a usable model"
    );
    assert!(!root.join(m.id).exists(), "left the model directory behind");
    assert!(
        !root.join(format!("{}.part", m.id)).exists(),
        "left a .part behind"
    );
    assert!(
        !root.join(format!("{}.tmp", m.id)).exists(),
        "left a .tmp behind"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
#[ignore = "downloads 21 MB; run with --ignored"]
fn a_good_model_installs_and_is_idempotent() {
    let root = tmpdir("good");
    let m = lector_engine::catalog::model("vits-piper-en_US-amy-medium-int8").unwrap();

    let mut saw_done = false;
    let dir = install(&root, m, &Cancel::new(), |p| {
        saw_done |= p.phase == Phase::Done
    })
    .unwrap();
    assert!(saw_done);
    assert!(is_installed(&root, m));
    assert!(dir.join("tokens.txt").exists());
    // The archive's own top-level directory must have been stripped.
    assert!(
        !dir.join(m.id).exists(),
        "archive top-level dir was not stripped"
    );

    // A second install is a no-op, not a re-download.
    let again = install(&root, m, &Cancel::new(), |_| {}).unwrap();
    assert_eq!(again, dir);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn cancelling_before_the_first_read_installs_nothing() {
    let root = tmpdir("cancel");
    let m = lector_engine::catalog::model("kokoro-multi-lang-v1_0").unwrap();
    let cancel = Cancel::new();
    cancel.cancel();

    let err = install(&root, m, &cancel, |_| {}).unwrap_err();
    assert!(err.contains("cancel"), "unexpected error: {err}");
    assert!(!is_installed(&root, m));
    assert!(!root.join(format!("{}.part", m.id)).exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn catalog_entries_look_sane() {
    for m in lector_engine::catalog::CATALOG {
        assert_eq!(m.sha256.len(), 64, "{}: sha256 wrong length", m.id);
        assert!(
            m.sha256.chars().all(|c| c.is_ascii_hexdigit()),
            "{}: sha256 not hex",
            m.id
        );
        assert!(
            m.url.ends_with(".tar.bz2"),
            "{}: unexpected archive type",
            m.id
        );
        assert!(m.url.contains(m.id), "{}: url does not match id", m.id);
        assert!(!m.speakers.is_empty(), "{}: no speakers", m.id);
        assert!(m.mb > 0, "{}: no size", m.id);
    }
    let ids: Vec<_> = lector_engine::catalog::CATALOG
        .iter()
        .map(|m| m.id)
        .collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "duplicate model ids");

    assert!(
        lector_engine::catalog::model(lector_engine::catalog::DEFAULT_MODEL).is_some(),
        "the default model is not in the catalog"
    );
}

#[test]
fn is_installed_is_false_for_an_empty_root() {
    let root = Path::new("/nonexistent-lector-root");
    let m = lector_engine::catalog::model("kokoro-multi-lang-v1_0").unwrap();
    assert!(!is_installed(root, m));
}
