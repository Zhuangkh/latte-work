//! System directory locations. Callers choose product subdirectories.
use std::{io, path::PathBuf};

pub fn home_dir() -> io::Result<PathBuf> {
    dirs::home_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "home directory unavailable"))
}

pub fn local_data_dir() -> io::Result<PathBuf> {
    dirs::data_local_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "local data directory unavailable"))
}
