//! Minimal reviewed WebView2 boundary for browser page tools (find, capture and
//! the DOM-search fallback). Only `win32` may use unsafe; exported operations
//! take borrowed WebView2 interfaces and return owned data.
#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod find_session;
pub use find_session::{FindRequest, FindSession};

#[cfg(windows)]
#[allow(unsafe_code)]
mod win32;

#[cfg(windows)]
pub use win32::{FindOutcome, capture_png, execute_script, find, stop_find};
