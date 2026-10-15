//! Host OS primitives shared by storage and the server, without protocol or GUI dependencies.
//! Product paths, retries and lifecycle decisions belong to callers.
mod control;
pub mod dirs;
pub mod fs;
pub mod ipc;
pub mod lock;
pub mod paths;
#[cfg(windows)]
pub mod pipe;
pub mod process;
/// Windows helper-only ConPTY primitives; the owning Server supplies a Job/deadline.
#[cfg(windows)]
pub mod terminal {
    pub use latte_work_windows_security::{ConsoleChild, PseudoConsole};
}
