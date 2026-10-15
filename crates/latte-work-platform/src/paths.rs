//! Native path validation and protocol-relative rendering; never use lossy paths for access.
use std::{
    io,
    path::{Component, Path},
};
#[cfg(unix)]
#[path = "paths/unix.rs"]
mod backend;
#[cfg(windows)]
#[path = "paths/windows.rs"]
mod backend;

pub fn file_name(name: &str) -> io::Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err(invalid());
    }
    backend::component(name)
}
pub fn relative(path: &Path) -> io::Result<()> {
    for part in path.components() {
        match part {
            Component::Normal(name) => backend::component(name.to_str().ok_or_else(invalid)?)?,
            Component::CurDir => {}
            _ => return Err(invalid()),
        }
    }
    Ok(())
}
pub fn relative_text(path: &Path) -> io::Result<String> {
    relative(path)?;
    path.components()
        .map(|part| part.as_os_str().to_str().ok_or_else(invalid))
        .collect::<io::Result<Vec<_>>>()
        .map(|parts| parts.join("/"))
}
pub fn absolute(path: &Path) -> io::Result<()> {
    backend::absolute(path)
}
pub fn check_project_links(root: &Path, relative: &Path) -> io::Result<()> {
    backend::check_project_links(root, relative)
}
/// Whether two existing project directories overlap, using native identity semantics.
pub fn overlap(left: &Path, right: &Path) -> io::Result<bool> {
    backend::overlap(left, right)
}
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "invalid or ambiguous native path",
    )
}
