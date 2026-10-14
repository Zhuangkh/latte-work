//! Raw Windows pipe operations. Only ipc may expose authenticated, guarded streams.
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::{Listener, Stream, connect_once, listen, verify_peer};
