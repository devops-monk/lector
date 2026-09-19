//! Getting a model onto disk, safely.
//!
//! The ordering is the whole point: **download, verify, unpack, in that order and
//! never any other**. A model that fails its checksum installs nothing at all --
//! not a partial directory, not a stray `.part`, nothing. Anything less means a
//! corrupt download becomes a voice that loads and then crashes somewhere far
//! away from the cause.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::catalog::Model;

/// Progress during an install. `total` is zero when the server declines to say.
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub received: u64,
    pub total: u64,
    pub phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Downloading,
    Verifying,
    Unpacking,
    Done,
}

/// Set to cancel an install in flight. Checked between reads and between archive
/// entries, so a cancel takes effect in well under a second.
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

fn part_path(root: &Path, m: &Model) -> PathBuf {
    root.join(format!("{}.part", m.id))
}

fn tmp_path(root: &Path, m: &Model) -> PathBuf {
    root.join(format!("{}.tmp", m.id))
}

pub fn is_installed(root: &Path, m: &Model) -> bool {
    root.join(m.id).join("tokens.txt").exists()
}

/// Removes every trace of an interrupted or failed install.
///
/// Called on every failure path. A `.part` left behind is litter the UI cannot
/// explain and the user cannot clear.
pub fn wipe(root: &Path, m: &Model) {
    let _ = std::fs::remove_file(part_path(root, m));
    let _ = std::fs::remove_dir_all(tmp_path(root, m));
}

/// Downloads, verifies and unpacks a model. Returns the installed directory.
///
/// Resumes from an existing `.part` when the server supports ranges -- a 349 MB
/// download dropped at 90% should not start over. The hash is always computed
/// over the whole finished file, never carried across the interruption.
pub fn install(
    root: &Path,
    m: &Model,
    cancel: &Cancel,
    mut on_progress: impl FnMut(Progress),
) -> Result<PathBuf, String> {
    let dest = root.join(m.id);
    if is_installed(root, m) {
        on_progress(Progress { received: 0, total: 0, phase: Phase::Done });
        return Ok(dest);
    }
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;

    let part = part_path(root, m);
    let have = std::fs::metadata(&part).map(|x| x.len()).unwrap_or(0);

    let result = (|| -> Result<(), String> {
        download(&part, m, have, cancel, &mut on_progress)?;

        on_progress(Progress { received: 0, total: 0, phase: Phase::Verifying });
        let actual = hash_file(&part)?;
        if actual != m.sha256 {
            return Err(format!(
                "{} failed its checksum. Nothing was installed.\nexpected {}\n     got {actual}",
                m.label, m.sha256
            ));
        }

        on_progress(Progress { received: 0, total: 0, phase: Phase::Unpacking });
        let tmp = tmp_path(root, m);
        let _ = std::fs::remove_dir_all(&tmp);
        unpack(&part, &tmp, cancel)?;

        // Only now does the model appear under its real name, so a half-extracted
        // directory is never visible to anything looking for an installed voice.
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
        Ok(())
    })();

    match result {
        Ok(()) => {
            let _ = std::fs::remove_file(&part);
            let _ = std::fs::remove_dir_all(tmp_path(root, m));
            on_progress(Progress { received: 0, total: 0, phase: Phase::Done });
            Ok(dest)
        }
        Err(e) => {
            wipe(root, m);
            Err(e)
        }
    }
}

fn download(
    part: &Path,
    m: &Model,
    resume_from: u64,
    cancel: &Cancel,
    on_progress: &mut impl FnMut(Progress),
) -> Result<(), String> {
    let mut req = ureq::get(m.url);
    if resume_from > 0 {
        req = req.set("Range", &format!("bytes={resume_from}-"));
    }
    let resp = req.call().map_err(|e| e.to_string())?;

    // 206 means the server honoured the range; anything else means start over.
    let resuming = resp.status() == 206 && resume_from > 0;
    let declared: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let total = if resuming { resume_from + declared } else { declared };

    let mut file = if resuming {
        std::fs::OpenOptions::new().append(true).open(part).map_err(|e| e.to_string())?
    } else {
        std::fs::File::create(part).map_err(|e| e.to_string())?
    };

    let mut received = if resuming { resume_from } else { 0 };
    let mut reader = resp.into_reader();
    let mut buf = vec![0u8; 64 * 1024];
    // Emit at roughly 1% granularity; per-chunk events flood the UI.
    let step = (total / 100).max(256 * 1024);
    let mut next_report = received + step;

    loop {
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        received += n as u64;
        if received >= next_report {
            next_report = received + step;
            on_progress(Progress { received, total, phase: Phase::Downloading });
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    on_progress(Progress { received, total, phase: Phase::Downloading });
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Unpacks into `dest`, stripping the archive's own top-level directory.
fn unpack(archive: &Path, dest: &Path, cancel: &Cancel) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut tar = tar::Archive::new(bzip2::read::BzDecoder::new(file));
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;

    for entry in tar.entries().map_err(|e| e.to_string())? {
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();

        // Zip-slip: an archive must never write outside the directory we chose.
        if path.components().any(|c| matches!(c, std::path::Component::ParentDir))
            || path.is_absolute()
        {
            return Err(format!("archive contains an unsafe path: {}", path.display()));
        }

        // Strip the leading component, which is the archive's own name.
        let stripped: PathBuf = path.components().skip(1).collect();
        if stripped.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(&stripped);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        entry.unpack(&out).map_err(|e| e.to_string())?;
    }
    Ok(())
}
