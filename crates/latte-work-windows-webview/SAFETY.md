# Windows WebView2 boundary

`latte-work-windows-webview` is the second project exception to the workspace's
`unsafe_code = "forbid"` rule. It exposes the minimum WebView2 native surface the
desktop browser pane needs: page find, surface capture as PNG, and one script
execution used only as the DOM-search fallback.

The crate defaults to `deny(unsafe_code)`; only the private `win32` module is
allowed to use unsafe. `unsafe_op_in_unsafe_fn` and Clippy
`undocumented_unsafe_blocks` remain denied, including inside that module.
Consumers and public APIs are safe Rust. `webview2-com` is pinned to `=0.38.2` to
match the version used by the locked `wry`; a mismatch fails to compile instead
of silently crossing type boundaries.

## Surface

- `find(controller, environment, query, backwards, restart, on_done)` reports one
  `FindOutcome` through `on_done`. `restart` starts a new session; otherwise the
  existing session advances with `FindNext`/`FindPrevious`. Runtimes without
  `ICoreWebView2_28`/`ICoreWebView2Environment15` report `ErrorKind::Unsupported`
  so the caller can use `window.find`.
- `capture_png(controller, on_done)` writes one PNG into an in-memory stream and
  returns it after checking `0 < size <= 64 MiB`.
- `execute_script(controller, script, on_done)` returns the JSON-encoded result.
- Every entry point calls `on_done` exactly once through an `Arc<Mutex<Option<_>>>`
  slot, so a synchronous setup failure and a late callback cannot double-report.

## Threading and ownership

All entry points must be called on the thread that created the webview (Tauri's
`with_webview`), because WebView2 hands results back on that thread's message
loop. The desktop therefore registers the completion callback on the main thread
and awaits a oneshot channel afterwards; it never blocks the main thread and
never moves a COM interface to a worker thread.

Borrowed `ICoreWebView2*` interfaces stay valid because the caller keeps the
webview alive for the duration of the call. The callbacks move only owned data
(`String`, `Vec<u8>`, the option slot) plus cloned COM handles used on the same
thread. The in-memory `IStream` is created by the crate and read only inside the
capture completion callback.

## Limits

- No navigation, JavaScript injection beyond the fixed fallback expression,
  WebView enumeration or IPC capability is exported.
- `find` bounds the query to 512 characters; query text and script input are
  copied into `HSTRING` values owned by the call.
- There is no unbounded wait inside the crate: all asynchronous completion is
  expected within the caller's own timeout (the desktop uses 12–15 seconds).
- Runtime availability of the Find interface is not assumed; the unsupported
  path is exercised by callers and must stay explicit.
