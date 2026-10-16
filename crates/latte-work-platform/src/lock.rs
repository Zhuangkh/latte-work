//! OS locks retained by owned file handles. No retries, truncation or lock-file removal.
use std::{
    fs::{File, TryLockError},
    io,
    path::Path,
};

/// An acquired exclusive lock. Dropping the guard releases the handle and lock.
/// All participants must retain the same lock file; replacing it defeats coordination.
#[derive(Debug)]
pub struct FileLock {
    _file: File,
}

impl FileLock {
    /// Try once. `WouldBlock` means contention; `Error` preserves the underlying I/O error.
    /// This coordinates processes; callers establish directory privacy separately.
    pub fn try_acquire(path: &Path) -> Result<Self, TryLockError> {
        let file = open(path).map_err(TryLockError::Error)?;
        file.try_lock()?;
        Ok(Self { _file: file })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Closing alone can retain the lock while a duplicated Unix descriptor is alive.
        // Teardown is best-effort and must not panic; the owned handle still closes next.
        let _ = self._file.unlock();
    }
}

fn open(path: &Path) -> io::Result<File> {
    match path.symlink_metadata() {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "lock path must be a regular file",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let file = crate::fs::open_lock_file(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "lock handle must refer to a regular file",
        ));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    #[test]
    fn lock_preserves_contents_and_releases_without_removing_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("server.lock");
        std::fs::write(&path, b"sentinel").unwrap();
        let held = FileLock::try_acquire(&path).unwrap();
        assert!(matches!(
            FileLock::try_acquire(&path),
            Err(TryLockError::WouldBlock)
        ));
        let independent = FileLock::try_acquire(&directory.path().join("upgrade.lock")).unwrap();
        drop(independent);
        drop(held);
        assert!(path.is_file());
        // Windows locks also block reads through other handles until release.
        assert_eq!(std::fs::read(&path).unwrap(), b"sentinel");
        let _reacquired = FileLock::try_acquire(&path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn dropping_guard_releases_lock_while_duplicate_remains_open() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("server.lock");
        let held = FileLock::try_acquire(&path).unwrap();
        let duplicate = held._file.try_clone().unwrap();
        assert!(matches!(
            FileLock::try_acquire(&path),
            Err(TryLockError::WouldBlock)
        ));
        drop(held);
        let _reacquired = FileLock::try_acquire(&path).unwrap();
        drop(duplicate);
    }

    #[cfg(unix)]
    #[test]
    fn dropping_duplicate_does_not_release_live_guard() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("server.lock");
        let held = FileLock::try_acquire(&path).unwrap();
        let duplicate = held._file.try_clone().unwrap();
        drop(duplicate);
        assert!(matches!(
            FileLock::try_acquire(&path),
            Err(TryLockError::WouldBlock)
        ));
        drop(held);
        let _reacquired = FileLock::try_acquire(&path).unwrap();
    }

    #[test]
    fn lock_preserves_io_errors_instead_of_reporting_contention() {
        let directory = tempfile::tempdir().unwrap();
        for path in [
            directory.path().to_owned(),
            directory.path().join("missing/server.lock"),
        ] {
            assert!(matches!(
                FileLock::try_acquire(&path),
                Err(TryLockError::Error(_))
            ));
        }
    }

    #[test]
    fn lock_is_exclusive_across_processes_and_released_after_exit_without_drop() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("server.lock");
        let held = FileLock::try_acquire(&path).unwrap();
        run_child(&path, "contend");
        drop(held);
        run_child(&path, "exit");
        let _reacquired = FileLock::try_acquire(&path).unwrap();
    }

    fn run_child(path: &Path, mode: &str) {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lock::tests::lock_subprocess", "--nocapture"])
            .env("LATTE_PLATFORM_LOCK_TEST_PATH", path)
            .env("LATTE_PLATFORM_LOCK_TEST_MODE", mode)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "lock subprocess {mode}: {status}");
                return;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("lock subprocess {mode} timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    // Runs inside this test executable, so it needs neither a shell nor a platform fixture.
    #[test]
    fn lock_subprocess() {
        let Some(path) = std::env::var_os("LATTE_PLATFORM_LOCK_TEST_PATH") else {
            return;
        };
        match std::env::var("LATTE_PLATFORM_LOCK_TEST_MODE")
            .unwrap()
            .as_str()
        {
            "contend" => assert!(matches!(
                FileLock::try_acquire(Path::new(&path)),
                Err(TryLockError::WouldBlock)
            )),
            "exit" => {
                let _held = FileLock::try_acquire(Path::new(&path)).unwrap();
                std::process::exit(0); // Skip Rust destructors; the OS must release the lock.
            }
            mode => panic!("unknown fixture mode: {mode}"),
        }
    }
}
