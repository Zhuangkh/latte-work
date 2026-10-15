//! Server-owned location policy. OS directory lookup lives in latte-work-platform.
#[cfg(unix)]
#[path = "host_paths/unix.rs"]
mod backend;
#[cfg(windows)]
#[path = "host_paths/windows.rs"]
mod backend;

pub use backend::{PATH_SYNTAX, default_state_dir, launch_directory, provider_directory};
