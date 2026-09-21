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

/// How much has to arrive before progress is reported again.
const PROGRESS_BYTES: u64 = 128 * 1024;
/// And how long has to pass. Both must hold: one alone is wrong at one end of
/// the connection-speed range or the other.
const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(40);

fn part_path(root: &Path, m: &Model) -> PathBuf {
    root.join(format!("{}.part", m.id))
}

fn tmp_path(root: &Path, m: &Model) -> PathBuf {
    root.join(format!("{}.tmp", m.id))
}

/// What a download needs against what the disk has.
///
/// Structured rather than a formatted string: the window has to compare the
/// two numbers to decide what to say, and a caller that must parse prose to
/// find out how short it is will get it wrong the first time the wording
/// changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Space {
    pub required_bytes: u64,
    pub available_bytes: u64,
}

/// Whether there is room for this model, before the first byte is fetched.
///
/// The archive and its unpacked contents both exist at once, plus the `.tmp`
/// directory the unpack builds before the rename -- so the requirement is
/// about three times the download, and a check for one times it would still
/// fill the disk. A disk with no room left is also how a half-written model
/// happens, which is the failure this exists to avoid.
pub fn check_space(root: &Path, m: &Model) -> Result<(), Space> {
    let required_bytes = (m.mb as u64) * 3 * 1024 * 1024;
    // Walk up to a directory that exists: the models directory is created on
    // first install, and asking about a path that is not there yet fails.
    let mut probe = root.to_path_buf();
    while !probe.exists() {
        match probe.parent() {
            Some(p) => probe = p.to_path_buf(),
            None => return Ok(()), // nothing to ask; let the download decide
        }
    }
    let Some(available_bytes) = available_space(&probe) else {
        return Ok(()); // cannot tell, so do not stand in the way
    };
    if available_bytes >= required_bytes {
        Ok(())
    } else {
        Err(Space {
            required_bytes,
            available_bytes,
        })
    }
}

#[cfg(unix)]
fn available_space(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `c` is a valid NUL-terminated path and `st` is fully written by
    // statvfs before it is read; both go out of scope here.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) != 0 {
            return None;
        }
        // bavail, not bfree: the latter includes blocks reserved for root.
        Some(st.f_bavail as u64 * st.f_frsize as u64)
    }
}

#[cfg(not(unix))]
fn available_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free: u64 = 0;
    // SAFETY: `wide` is NUL-terminated and outlives the call; the two null
    // pointers are documented as optional out-parameters.
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free)
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
        on_progress(Progress {
            received: 0,
            total: 0,
            phase: Phase::Done,
        });
        return Ok(dest);
    }
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;

    let part = part_path(root, m);
    let have = std::fs::metadata(&part).map(|x| x.len()).unwrap_or(0);

    let result = (|| -> Result<(), String> {
        download(&part, m, have, cancel, &mut on_progress)?;

        on_progress(Progress {
            received: 0,
            total: 0,
            phase: Phase::Verifying,
        });
        let actual = hash_file(&part)?;
        if actual != m.sha256 {
            return Err(format!(
                "{} failed its checksum. Nothing was installed.\nexpected {}\n     got {actual}",
                m.label, m.sha256
            ));
        }

        on_progress(Progress {
            received: 0,
            total: 0,
            phase: Phase::Unpacking,
        });
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
            on_progress(Progress {
                received: 0,
                total: 0,
                phase: Phase::Done,
            });
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
    let declared: u64 = resp
        .header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let total = if resuming {
        resume_from + declared
    } else {
        declared
    };

    let mut file = if resuming {
        std::fs::OpenOptions::new()
            .append(true)
            .open(part)
            .map_err(|e| e.to_string())?
    } else {
        std::fs::File::create(part).map_err(|e| e.to_string())?
    };

    let mut received = if resuming { resume_from } else { 0 };
    let mut reader = resp.into_reader();
    let mut buf = vec![0u8; 64 * 1024];
    // Both conditions, not either: bytes alone floods a fast connection and
    // time alone starves a slow one, where a bar that has not moved in twenty
    // seconds is indistinguishable from a hang.
    let mut reported_at = received;
    let mut reported_when = std::time::Instant::now();

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
        if received - reported_at >= PROGRESS_BYTES && reported_when.elapsed() >= PROGRESS_INTERVAL
        {
            reported_at = received;
            reported_when = std::time::Instant::now();
            on_progress(Progress {
                received,
                total,
                phase: Phase::Downloading,
            });
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    on_progress(Progress {
        received,
        total,
        phase: Phase::Downloading,
    });
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
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
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
        if path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
            || path.is_absolute()
        {
            return Err(format!(
                "archive contains an unsafe path: {}",
                path.display()
            ));
        }

        // Strip the archive's own top-level directory.
        //
        // Not simply "skip the first component": some archives prefix every
        // entry with `./`, and skipping that leaves the top-level directory in
        // place, so the model unpacks one level too deep and nothing can find
        // its tokens.txt. Drop any leading `.` first, then the real directory.
        let stripped: PathBuf = path
            .components()
            .filter(|c| !matches!(c, std::path::Component::CurDir))
            .skip(1)
            .collect();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn a_model() -> Model {
        *crate::catalog::CATALOG
            .iter()
            .find(|m| m.id == crate::catalog::DEFAULT_MODEL)
            .unwrap()
    }

    #[test]
    fn a_disk_with_room_passes() {
        // The temp directory is on a real filesystem with room for 21 MB.
        assert_eq!(check_space(&std::env::temp_dir(), &a_model()), Ok(()));
    }

    #[test]
    fn a_path_that_does_not_exist_yet_is_checked_against_its_parent() {
        let deep = std::env::temp_dir().join("lector-no-such-dir/models/and/deeper");
        assert_eq!(check_space(&deep, &a_model()), Ok(()));
    }

    #[test]
    fn the_requirement_allows_for_the_archive_and_its_contents() {
        // An impossible model: 8 TB, so no real disk passes, and the numbers
        // come back structured rather than as prose to parse.
        let mut huge = a_model();
        huge.mb = 8 * 1024 * 1024;
        let err = check_space(&std::env::temp_dir(), &huge).unwrap_err();
        assert_eq!(err.required_bytes, huge.mb as u64 * 3 * 1024 * 1024);
        assert!(err.available_bytes < err.required_bytes);
    }

    #[test]
    fn cancelling_is_visible_to_the_worker() {
        let c = Cancel::new();
        assert!(!c.is_cancelled());
        let copy = c.clone();
        c.cancel();
        assert!(copy.is_cancelled(), "a clone must share the flag");
    }
}
