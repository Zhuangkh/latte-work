use std::{io, path::PathBuf};

pub const PATH_SYNTAX: latte_work_protocol::PathSyntax = latte_work_protocol::PathSyntax::Windows;

pub fn provider_directory(state: &std::path::Path) -> io::Result<PathBuf> {
    if state.join("providers.json").try_exists()? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Windows legacy providers.json must be migrated to private storage explicitly",
        ));
    }
    Ok(state.join("private"))
}
pub fn launch_directory(settings: &std::path::Path) -> PathBuf {
    settings.join("launch")
}

pub fn default_state_dir() -> io::Result<PathBuf> {
    Ok(latte_work_platform::dirs::local_data_dir()?.join("latte-work"))
}
