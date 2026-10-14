use std::{io, path::Path};
pub(super) fn component(name: &str) -> io::Result<()> {
    if name.contains('\0') {
        Err(super::invalid())
    } else {
        Ok(())
    }
}
pub(super) fn absolute(path: &Path) -> io::Result<()> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(super::invalid())
    }
}
pub(super) fn check_project_links(_: &Path, _: &Path) -> io::Result<()> {
    Ok(())
}
