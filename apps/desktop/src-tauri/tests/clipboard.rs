//! Injected RAII acquisition and file data exercise the production reader's
//! lifecycle without reading or changing the user's system clipboard.
#![cfg(windows)]

#[path = "../src/paste/windows_clipboard.rs"]
mod windows_clipboard;

use clipboard_win::{ErrorCode, Getter, SysResult};
use std::{cell::Cell, path::PathBuf, rc::Rc};

#[derive(Default)]
struct ClipboardState {
    open: Cell<bool>,
    acquisitions: Cell<usize>,
    releases: Cell<usize>,
    reads: Cell<usize>,
}

struct ClipboardGuard(Rc<ClipboardState>);

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        assert!(self.0.open.replace(false));
        self.0.releases.set(self.0.releases.get() + 1);
    }
}

impl ClipboardState {
    fn acquire(self: &Rc<Self>, busy: bool) -> Result<ClipboardGuard, ()> {
        self.acquisitions.set(self.acquisitions.get() + 1);
        if busy {
            return Err(());
        }
        assert!(!self.open.replace(true));
        Ok(ClipboardGuard(Rc::clone(self)))
    }

    fn assert_released(&self) {
        assert!(!self.open.get());
        assert_eq!(self.acquisitions.get(), 1);
        assert_eq!(self.reads.get(), 1);
        assert_eq!(self.releases.get(), 1);
    }
}

struct FileFixture {
    state: Rc<ClipboardState>,
    paths: Vec<PathBuf>,
    fail: bool,
}

impl FileFixture {
    fn new(state: &Rc<ClipboardState>, paths: &[&str], fail: bool) -> Self {
        Self {
            state: Rc::clone(state),
            paths: paths.iter().map(PathBuf::from).collect(),
            fail,
        }
    }
}

impl Getter<Vec<PathBuf>> for FileFixture {
    fn read_clipboard(&self, out: &mut Vec<PathBuf>) -> SysResult<usize> {
        assert!(self.state.open.get(), "clipboard must be open during read");
        self.state.reads.set(self.state.reads.get() + 1);
        out.extend(self.paths.iter().cloned());
        if self.fail {
            Err(ErrorCode::last_system())
        } else {
            Ok(out.len())
        }
    }
}

#[test]
fn file_read_holds_guard_and_releases_it_after_success() {
    let state = Rc::new(ClipboardState::default());
    let fixture = FileFixture::new(&state, &[r"C:\fixtures\drawing.dwg"], false);
    assert_eq!(
        windows_clipboard::file_paths(|| state.acquire(false), fixture).unwrap(),
        [r"C:\fixtures\drawing.dwg"]
    );
    state.assert_released();
}

#[test]
fn multiple_paths_preserve_order_and_unicode() {
    let state = Rc::new(ClipboardState::default());
    let fixture = FileFixture::new(
        &state,
        &[r"C:\fixtures\图纸.dwg", r"C:\fixtures\second file.txt"],
        false,
    );
    assert_eq!(
        windows_clipboard::file_paths(|| state.acquire(false), fixture).unwrap(),
        [r"C:\fixtures\图纸.dwg", r"C:\fixtures\second file.txt"]
    );
    state.assert_released();
}

#[test]
fn empty_file_list_releases_guard() {
    let state = Rc::new(ClipboardState::default());
    let fixture = FileFixture::new(&state, &[], false);
    assert!(
        windows_clipboard::file_paths(|| state.acquire(false), fixture)
            .unwrap()
            .is_empty()
    );
    state.assert_released();
}

#[test]
fn read_error_discards_partial_paths_and_releases_guard() {
    let state = Rc::new(ClipboardState::default());
    let fixture = FileFixture::new(&state, &[r"C:\fixtures\partial.dwg"], true);
    assert!(
        windows_clipboard::file_paths(|| state.acquire(false), fixture)
            .unwrap()
            .is_empty()
    );
    state.assert_released();
}

#[test]
fn file_limit_is_preserved_and_guard_released() {
    for count in [20, 21] {
        let state = Rc::new(ClipboardState::default());
        let fixture = FileFixture::new(&state, &vec![r"C:\fixtures\drawing.dwg"; count], false);
        let result = windows_clipboard::file_paths(|| state.acquire(false), fixture);
        if count == 20 {
            assert_eq!(result.unwrap().len(), 20);
        } else {
            assert_eq!(result.unwrap_err(), "一次最多粘贴 20 个附件");
        }
        state.assert_released();
    }
}

#[test]
fn invalid_unicode_releases_guard_and_returns_error() {
    use std::os::windows::ffi::OsStringExt;
    let state = Rc::new(ClipboardState::default());
    let mut fixture = FileFixture::new(&state, &[], false);
    fixture
        .paths
        .push(std::ffi::OsString::from_wide(&[0xd800]).into());
    assert_eq!(
        windows_clipboard::file_paths(|| state.acquire(false), fixture).unwrap_err(),
        "剪贴板文件路径不是有效 UTF-8"
    );
    state.assert_released();
}

#[test]
fn acquisition_failure_never_calls_reader_or_retries() {
    let state = Rc::new(ClipboardState::default());
    let fixture = FileFixture::new(&state, &[r"C:\fixtures\drawing.dwg"], false);
    assert!(
        windows_clipboard::file_paths(|| state.acquire(true), fixture)
            .unwrap()
            .is_empty()
    );
    assert!(!state.open.get());
    assert_eq!(state.acquisitions.get(), 1);
    assert_eq!(state.reads.get(), 0);
    assert_eq!(state.releases.get(), 0);
}
