//! Restart on reinstall.
//!
//! jdo watches its own executable. When `cargo install` replaces it, the
//! running instance restores the terminal and `exec`s the new binary in place
//! with the same arguments, handing its view state (grouping, folds, selected
//! card) across in `JDO_RESUME` so it comes back where you were.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// Env var carrying view state into the restarted process.
pub const RESUME_VAR: &str = "JDO_RESUME";

const CHECK_EVERY: Duration = Duration::from_secs(1);

/// Identity of the file at the executable's path.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    inode: u64,
    modified: Option<SystemTime>,
    len: u64,
}

pub struct BinaryWatch {
    path: PathBuf,
    original: Stamp,
    /// A changed stamp seen on the last check; it must hold still for one more
    /// check before we trust the new binary is fully written.
    candidate: Option<Stamp>,
    last_check: Instant,
}

impl BinaryWatch {
    /// Must run at startup: once the binary is replaced, Linux reports the
    /// running executable as "... (deleted)".
    pub fn new() -> Option<Self> {
        let path = std::env::current_exe().ok()?;
        let original = stamp(&path)?;
        Some(Self { path, original, candidate: None, last_check: Instant::now() })
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// True once a replacement binary is in place and stable.
    pub fn replaced(&mut self) -> bool {
        if self.last_check.elapsed() < CHECK_EVERY {
            return false;
        }
        self.last_check = Instant::now();
        let current = match stamp(&self.path) {
            Some(current) if current != self.original => current,
            // Unchanged, or mid-replace (briefly missing).
            _ => {
                self.candidate = None;
                return false;
            }
        };
        if self.candidate.as_ref() == Some(&current) {
            return true;
        }
        self.candidate = Some(current);
        false
    }
}

#[cfg(unix)]
fn stamp(path: &PathBuf) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp { inode: meta.ino(), modified: meta.modified().ok(), len: meta.len() })
}

#[cfg(not(unix))]
fn stamp(path: &PathBuf) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp { inode: 0, modified: meta.modified().ok(), len: meta.len() })
}

/// Replace this process with the binary at `path`, same arguments, carrying
/// `state`. Only returns if the exec failed.
#[cfg(unix)]
pub fn exec(path: &PathBuf, state: &str) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(path)
        .args(std::env::args_os().skip(1))
        .env(RESUME_VAR, state)
        .exec()
}

#[cfg(not(unix))]
pub fn exec(_path: &PathBuf, _state: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Unsupported, "restart-in-place needs unix")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_binary_is_detected_once_it_holds_still() {
        let dir = std::env::temp_dir().join(format!("jdo-reload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("jdo");
        std::fs::write(&path, b"old").unwrap();
        let mut watch = BinaryWatch {
            path: path.clone(),
            original: stamp(&path).unwrap(),
            candidate: None,
            last_check: Instant::now() - CHECK_EVERY,
        };
        let mut check = |watch: &mut BinaryWatch| {
            watch.last_check = Instant::now() - CHECK_EVERY;
            watch.replaced()
        };
        assert!(!check(&mut watch));

        // Replace like `cargo install` does: write elsewhere, rename over.
        let staged = dir.join("jdo.new");
        std::fs::write(&staged, b"new build").unwrap();
        std::fs::rename(&staged, &path).unwrap();
        assert!(!check(&mut watch), "first sighting waits for stability");
        assert!(check(&mut watch), "stable replacement triggers a restart");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
