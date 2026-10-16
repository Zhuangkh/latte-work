use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

#[derive(Debug)]
pub(super) struct Guard;
pub(super) fn validate_path(_: &Path) -> io::Result<()> {
    Ok(())
}
pub(super) fn is_link(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
pub(super) fn prepare_state(path: &Path) -> io::Result<std::path::PathBuf> {
    let directory = crate::fs::PrivateDir::prepare(path)?;
    Ok(directory.path().to_owned())
}

pub(super) fn prepare_dir(path: &Path) -> io::Result<Guard> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(Guard)
}

pub(super) fn protect_file(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

pub(super) fn verify_open_file(file: &File) -> io::Result<()> {
    if file.metadata()?.is_file() && file.metadata()?.permissions().mode() & 0o777 == 0o600 {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "file is not private",
        ))
    }
}

pub(super) fn protect_open_file(file: &File) -> io::Result<()> {
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

pub(crate) fn open_lock_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)
}

pub(super) fn open_regular_file(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file must be regular",
        ));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fs::PrivateDir, lock::FileLock};
    use std::{io::Write, os::unix::fs::symlink};

    #[test]
    fn private_files_remain_private_after_atomic_replacement() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("private");
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        let directory = PrivateDir::prepare(&path).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let destination = path.join("credential.json");
        fs::write(&destination, b"old").unwrap();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o644)).unwrap();
        directory.protect_file("credential.json").unwrap();
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let mut file = directory.tempfile("launch-", ".json").unwrap();
        assert_eq!(
            file.as_file().metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        file.write_all(b"new").unwrap();
        file.persist(&destination).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn rejects_links_and_names_that_escape_without_changing_the_target() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::set_permissions(outside.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let link = root.path().join("link");
        symlink(outside.path(), &link).unwrap();
        assert!(PrivateDir::prepare(&link).is_err());
        assert!(PrivateDir::prepare(&link.join("")).is_err());
        assert!(PrivateDir::prepare(&link.join(".")).is_err());
        assert_eq!(
            fs::metadata(outside.path()).unwrap().permissions().mode() & 0o777,
            0o755
        );
        let dangling = root.path().join("dangling");
        symlink(outside.path().join("missing"), &dangling).unwrap();
        assert!(PrivateDir::prepare(&dangling).is_err());
        assert!(!outside.path().join("missing").exists());
        let target = outside.path().join("credential");
        fs::write(&target, b"unchanged").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        let directory = PrivateDir::prepare(root.path()).unwrap();
        symlink(&target, root.path().join("credential")).unwrap();
        assert!(directory.protect_file("credential").is_err());
        assert!(FileLock::try_acquire(&root.path().join("credential")).is_err());
        for name in [
            "../credential",
            "/credential",
            "sub/file",
            "sub\\file",
            ".",
            "",
        ] {
            assert!(directory.protect_file(name).is_err());
        }
        assert!(directory.tempfile("../", ".json").is_err());
        assert!(directory.tempfile("launch-", "/escape").is_err());
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(fs::read(&target).unwrap(), b"unchanged");
    }
}
