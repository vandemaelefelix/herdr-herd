//! Single-owner advisory lock so only one `control` watchdog runs per session.
//! Uses `std::fs::File::try_lock` (stable since Rust 1.89) — no external crate.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;

/// Held for the controller's lifetime. The advisory lock is released when this
/// guard drops (the file is closed) — so a crash frees it too.
pub struct LockGuard {
    _file: File,
}

/// Try to take an exclusive, non-blocking advisory lock on `path`. Returns
/// `Ok(Some(guard))` if acquired, `Ok(None)` if another process holds it (a
/// normal outcome — a second controller should just exit).
pub fn acquire(path: &Path) -> io::Result<Option<LockGuard>> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(LockGuard { _file: file })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_grants_the_lock_then_blocks_a_second_holder() {
        let path = std::env::temp_dir().join(format!("herdr-pets-lock-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let first = acquire(&path).unwrap();
        assert!(first.is_some(), "first acquire takes the lock");

        let second = acquire(&path).unwrap();
        assert!(
            second.is_none(),
            "second acquire is blocked while the first is held"
        );

        drop(first);
        let third = acquire(&path).unwrap();
        assert!(third.is_some(), "dropping the guard frees the lock");

        drop(third);
        let _ = std::fs::remove_file(&path);
    }
}
