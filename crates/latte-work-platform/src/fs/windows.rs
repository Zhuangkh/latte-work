//! Private directories have exact user-only DACLs; ordinary state inherits ACLs.
use latte_work_windows_security::{ControlDirectory, verify_private_file};
use std::{
    fs::{File, OpenOptions},
    io,
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
};
#[cfg(test)]
#[path = "windows/tests.rs"]
mod tests;

pub(super) fn open_regular_file(path: &Path) -> io::Result<File> {
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
    let path = latte_work_windows_security::local_path(path)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || is_link(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "file must be regular, not a reparse point",
        ));
    }
    Ok(file)
}

pub(super) type Guard = ControlDirectory;
pub(super) fn validate_path(path: &Path) -> io::Result<()> {
    latte_work_windows_security::local_path(path).map(|_| ())
}
pub(super) fn prepare_dir(path: &Path) -> io::Result<Guard> {
    ControlDirectory::create_or_validate(path)
}
pub(super) fn prepare_state(path: &Path) -> io::Result<std::path::PathBuf> {
    latte_work_windows_security::prepare_state_directory(path)
}
pub(super) fn protect_file(path: &Path) -> io::Result<()> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    verify_private_file(&file)
}
pub(super) fn verify_open_file(file: &File) -> io::Result<()> {
    verify_private_file(file)
}
pub(super) fn protect_open_file(file: &File) -> io::Result<()> {
    verify_private_file(file)
}
pub(crate) fn open_lock_file(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || is_link(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "lock is not a regular file",
        ));
    }
    Ok(file)
}
pub(super) fn is_link(metadata: &std::fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}
pub fn directory_identity(path: &Path) -> io::Result<(u64, [u8; 16])> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || is_link(&metadata) {
        return Err(io::Error::other("not an ordinary directory"));
    }
    let identity = latte_work_windows_security::file_identity(&file)?;
    Ok((identity.volume, identity.file_id))
}
