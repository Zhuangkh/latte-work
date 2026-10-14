//! Host OS primitives shared by storage and the server, without protocol or GUI dependencies.
//! Product paths, retries and lifecycle decisions belong to callers.
mod control;
pub mod dirs;
pub mod fs;
pub mod ipc;
pub mod lock;
pub mod paths;
pub mod process;
