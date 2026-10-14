//! PTY capability is selected at this module boundary.
#[cfg(unix)]
#[path = "terminal/unix.rs"]
mod backend;
#[cfg(windows)]
#[path = "terminal/windows.rs"]
mod backend;
pub use backend::{SharedTerminals, Terminals, write};
