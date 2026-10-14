//! One-shot private named pipes for local helper handshakes (for example the
//! SSH askpass helper). Unix uses filesystem sockets with private directories.
#[cfg(windows)]
#[path = "pipe/windows.rs"]
mod backend;
#[cfg(windows)]
pub use backend::{PrivatePipe, PrivateStream, connect_file_once};
