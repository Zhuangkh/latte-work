//! The only unsafe boundary of this crate. WebView2 calls are generated as raw
//! unsafe methods and must run on the thread that created the webview; every
//! operation reports exactly one result through a callback and bounds its work.
use crate::{
    FindRequest,
    find_session::{FindControl, FindOptions, configure, prepare, stop_if_current},
};
use std::{io, sync::Arc};
use webview2_com::{
    CapturePreviewCompletedHandler, ExecuteScriptCompletedHandler, FindStartCompletedHandler,
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, ICoreWebView2_28, ICoreWebView2Controller,
        ICoreWebView2Environment, ICoreWebView2Environment15, ICoreWebView2Find,
        ICoreWebView2FindOptions,
    },
};
use windows::{
    Win32::{
        System::Com::{IStream, STATFLAG_NONAME, STATSTG, STREAM_SEEK_SET},
        UI::Shell::SHCreateMemStream,
    },
    core::{HSTRING, Interface, PCWSTR},
};

const MAX_CAPTURE: usize = 64 * 1024 * 1024;

/// Result of one page-search step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindOutcome {
    pub found: bool,
    pub match_count: i32,
    pub active_index: i32,
}

/// Searches the page, calling `on_done` exactly once. `restart` begins a new
/// session for the query; otherwise the existing session advances in the
/// requested direction. Older runtimes without the Find interface report
/// `ErrorKind::Unsupported`.
pub fn find(
    controller: &ICoreWebView2Controller,
    environment: &ICoreWebView2Environment,
    query: &str,
    backwards: bool,
    restart: bool,
    request: FindRequest,
    on_done: impl FnOnce(io::Result<FindOutcome>) + Send + 'static,
) {
    let finish = Finish::new(on_done);
    if let Err(error) = request.check() {
        finish.call(Err(error));
        return;
    }
    if query.is_empty() || query.chars().count() > 512 {
        finish.call(Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid find query",
        )));
        return;
    }
    // SAFETY: called on the webview's thread; the borrowed controller stays alive.
    let find = match unsafe {
        controller
            .CoreWebView2()
            .map_err(to_io)
            .and_then(|core| core.cast::<ICoreWebView2_28>().map_err(|_| unsupported()))
    } {
        Ok(view) => {
            // SAFETY: the WebView owns the Find object for its lifetime.
            match unsafe { view.Find() } {
                Ok(find) => find,
                Err(error) => {
                    finish.call(Err(to_io(error)));
                    return;
                }
            }
        }
        Err(error) => {
            finish.call(Err(error));
            return;
        }
    };
    if let Err(error) = prepare(&find, &request, restart, backwards) {
        finish.call(Err(error));
        return;
    }
    if restart {
        let options = match find_options(environment, query) {
            Ok(options) => options,
            Err(error) => {
                finish.call(Err(error));
                return;
            }
        };
        let completion = finish.clone();
        let handler_view = find.clone();
        let handler = FindStartCompletedHandler::create(Box::new(move |result| {
            if let Err(error) = request.check() {
                completion.call(Err(error));
            } else if result.is_err() {
                completion.call(Err(io::Error::other("find request failed")));
            } else {
                completion.call(read_outcome(&handler_view));
            }
            Ok(())
        }));
        // SAFETY: the session lives on the WebView; the completion handler fires
        // on the webview thread and reports exactly once.
        if let Err(error) = unsafe { find.Start(&options, &handler) } {
            finish.call(Err(to_io(error)));
        }
    } else {
        finish.call(request.check().and_then(|()| read_outcome(&find)));
    }
}

/// Stops an active native session. An unavailable interface remains explicit.
pub fn stop_find(controller: &ICoreWebView2Controller, request: &FindRequest) -> io::Result<()> {
    if request.check().is_err() {
        return Ok(());
    }
    // SAFETY: called on the webview's thread with a live borrowed controller.
    let core = unsafe { controller.CoreWebView2() }.map_err(to_io)?;
    let view = core.cast::<ICoreWebView2_28>().map_err(|_| unsupported())?;
    // SAFETY: the WebView owns this Find interface for its lifetime.
    let find = unsafe { view.Find() }.map_err(to_io)?;
    stop_if_current(&find, request)
}

impl FindControl for ICoreWebView2Find {
    fn stop(&self) -> io::Result<()> {
        // SAFETY: this adapter is used only on the owning webview's thread.
        unsafe { self.Stop() }.map_err(to_io)
    }
    fn next(&self) -> io::Result<()> {
        // SAFETY: this adapter is used only on the owning webview's thread.
        unsafe { self.FindNext() }.map_err(to_io)
    }
    fn previous(&self) -> io::Result<()> {
        // SAFETY: this adapter is used only on the owning webview's thread.
        unsafe { self.FindPrevious() }.map_err(to_io)
    }
}

impl FindOptions for ICoreWebView2FindOptions {
    fn term(&self, query: &str) -> io::Result<()> {
        let term = HSTRING::from(query);
        // SAFETY: WebView2 copies the string while this HSTRING is alive.
        unsafe { self.SetFindTerm(PCWSTR(term.as_ptr())) }.map_err(to_io)
    }
    fn highlight_all(&self, value: bool) -> io::Result<()> {
        // SAFETY: the options belong to the owning webview's thread.
        unsafe { self.SetShouldHighlightAllMatches(value) }.map_err(to_io)
    }
    fn match_word(&self, value: bool) -> io::Result<()> {
        // SAFETY: the options belong to the owning webview's thread.
        unsafe { self.SetShouldMatchWord(value) }.map_err(to_io)
    }
    fn case_sensitive(&self, value: bool) -> io::Result<()> {
        // SAFETY: the options belong to the owning webview's thread.
        unsafe { self.SetIsCaseSensitive(value) }.map_err(to_io)
    }
    fn suppress_dialog(&self, value: bool) -> io::Result<()> {
        // SAFETY: the options belong to the owning webview's thread.
        unsafe { self.SetSuppressDefaultFindDialog(value) }.map_err(to_io)
    }
}

/// Captures the visible webview surface as PNG bytes, calling `on_done` once.
pub fn capture_png(
    controller: &ICoreWebView2Controller,
    on_done: impl FnOnce(io::Result<Vec<u8>>) + Send + 'static,
) {
    let finish = Finish::new(on_done);
    // SAFETY: called on the webview's thread with a live controller.
    let core = match unsafe { controller.CoreWebView2() } {
        Ok(core) => core,
        Err(error) => {
            finish.call(Err(to_io(error)));
            return;
        }
    };
    let stream = match memory_stream() {
        Ok(stream) => stream,
        Err(error) => {
            finish.call(Err(error));
            return;
        }
    };
    let handler_stream = stream.clone();
    let completion = finish.clone();
    let handler = CapturePreviewCompletedHandler::create(Box::new(move |result| {
        let outcome = if result.is_err() {
            Err(io::Error::other("capture failed"))
        } else {
            read_memory_stream(&handler_stream)
        };
        completion.call(outcome);
        Ok(())
    }));
    // SAFETY: the memory stream is written only by the completion callback
    // before the handler reads it.
    if let Err(error) = unsafe {
        core.CapturePreview(
            COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
            &stream,
            &handler,
        )
    } {
        finish.call(Err(to_io(error)));
    }
}

/// Runs a script and calls `on_done` once with its JSON-encoded result. Used
/// only for the DOM-search fallback when the runtime lacks the Find interface.
pub fn execute_script(
    controller: &ICoreWebView2Controller,
    script: &str,
    on_done: impl FnOnce(io::Result<String>) + Send + 'static,
) {
    let finish = Finish::new(on_done);
    // SAFETY: called on the webview's thread with a live controller.
    let core = match unsafe { controller.CoreWebView2() } {
        Ok(core) => core,
        Err(error) => {
            finish.call(Err(to_io(error)));
            return;
        }
    };
    let completion = finish.clone();
    let handler = ExecuteScriptCompletedHandler::create(Box::new(move |result, json| {
        completion.call(result.map(|()| json).map_err(to_io));
        Ok(())
    }));
    let script = HSTRING::from(script);
    // SAFETY: the controller executes the script on its own page; the handler
    // only forwards the JSON result it receives.
    if let Err(error) = unsafe { core.ExecuteScript(&script, &handler) } {
        finish.call(Err(to_io(error)));
    }
}

fn find_options(
    environment: &ICoreWebView2Environment,
    query: &str,
) -> io::Result<ICoreWebView2FindOptions> {
    let environment = environment
        .cast::<ICoreWebView2Environment15>()
        .map_err(|_| unsupported())?;
    // SAFETY: the options object is created on its owning webview's thread.
    let options = unsafe { environment.CreateFindOptions() }.map_err(to_io)?;
    configure(&options, query)?;
    Ok(options)
}

fn read_outcome(find: &ICoreWebView2Find) -> io::Result<FindOutcome> {
    // SAFETY: both outputs are plain integers filled by the Find object.
    unsafe {
        let mut match_count = 0i32;
        let mut active_index = 0i32;
        find.MatchCount(&mut match_count).map_err(to_io)?;
        find.ActiveMatchIndex(&mut active_index).map_err(to_io)?;
        Ok(FindOutcome {
            found: match_count > 0,
            match_count,
            active_index,
        })
    }
}

fn unsupported() -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, "WebView2 Find is unavailable")
}

fn memory_stream() -> io::Result<IStream> {
    // SAFETY: SHCreateMemStream returns an owned in-memory IStream; no external
    // memory is borrowed.
    unsafe { SHCreateMemStream(None) }
        .ok_or_else(|| io::Error::other("failed to create a memory stream"))
}

fn read_memory_stream(stream: &IStream) -> io::Result<Vec<u8>> {
    // SAFETY: the stream is an owned memory stream written by the capture
    // callback; seeking and reading within its reported size is valid.
    unsafe {
        stream.Seek(0, STREAM_SEEK_SET, None).map_err(to_io)?;
        let mut stat = STATSTG::default();
        stream.Stat(&mut stat, STATFLAG_NONAME).map_err(to_io)?;
        let size = stat.cbSize as usize;
        if size == 0 || size > MAX_CAPTURE {
            return Err(io::Error::other("capture size out of range"));
        }
        let mut bytes = vec![0u8; size];
        let mut read = 0u32;
        stream
            .Read(
                bytes.as_mut_ptr().cast(),
                u32::try_from(size).map_err(|_| io::Error::other("capture too large"))?,
                Some(&mut read),
            )
            .ok()
            .map_err(to_io)?;
        bytes.truncate(read as usize);
        Ok(bytes)
    }
}

fn to_io(error: windows::core::Error) -> io::Error {
    io::Error::other(error.to_string())
}

/// Delivers the single result of an operation across the COM callback boundary.
struct Finish<F> {
    slot: Arc<std::sync::Mutex<Option<F>>>,
}

impl<F> Finish<F> {
    fn new(callback: F) -> Self {
        Self {
            slot: Arc::new(std::sync::Mutex::new(Some(callback))),
        }
    }

    fn call<T>(&self, value: T)
    where
        F: FnOnce(T),
    {
        let callback = self.slot.lock().ok().and_then(|mut slot| slot.take());
        if let Some(callback) = callback {
            callback(value);
        }
    }
}

impl<F> Clone for Finish<F> {
    fn clone(&self) -> Self {
        Self {
            slot: Arc::clone(&self.slot),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn setup_failure_and_late_completion_deliver_only_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let finish = Finish::new(move |_: io::Result<FindOutcome>| {
            count.fetch_add(1, Ordering::SeqCst);
        });
        let late = finish.clone();
        finish.call(Err(io::Error::other("setup failed")));
        late.call(Ok(FindOutcome {
            found: true,
            match_count: 3,
            active_index: 2,
        }));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
