//! Raw Windows pipe operations. Only ipc and the one-shot private pipe API may
//! expose authenticated, guarded streams.
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::{Listener, Stream, connect_file_once, connect_once, listen, verify_peer};
