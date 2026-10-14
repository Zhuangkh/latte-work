use super::Identity;
use anyhow::Result;
use std::{
    fs::{File, Metadata, Permissions},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};
pub(super) const UNDO_SUPPORTED: bool = true;
pub(super) fn identity(path: &Path) -> Result<Identity> {
    let metadata = path.metadata()?;
    Ok(Identity::Unix((metadata.dev(), metadata.ino())))
}
pub(super) fn mode(metadata: &Metadata) -> Option<u32> {
    Some(metadata.mode())
}
pub(super) fn restore_mode(file: &File, mode: u32) -> Result<()> {
    file.set_permissions(Permissions::from_mode(mode & 0o777))?;
    Ok(())
}
pub(super) fn require_undo() -> Result<()> {
    Ok(())
}
