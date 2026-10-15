use std::{io, path::PathBuf};

pub const PATH_SYNTAX: latte_work_protocol::PathSyntax = latte_work_protocol::PathSyntax::Posix;

pub fn provider_directory(state: &std::path::Path) -> io::Result<PathBuf> {
    Ok(state.to_owned())
}
pub fn launch_directory(settings: &std::path::Path) -> PathBuf {
    settings.to_owned()
}

pub fn default_state_dir() -> io::Result<PathBuf> {
    // Keep the existing path on both Linux and macOS, independent of XDG settings.
    Ok(latte_work_platform::dirs::home_dir()?.join(".local/share/latte-work"))
}
