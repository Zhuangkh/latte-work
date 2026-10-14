//! Bounded, one-shot page tools. macOS uses its WebKit bridge; Windows calls the
//! reviewed WebView2 boundary; other platforms fail explicitly.
use serde_json::Value;
#[cfg(target_os = "macos")]
use serde_json::json;
use tauri::Webview;

#[cfg(windows)]
pub use latte_work_windows_webview::FindSession;
#[cfg(not(windows))]
#[derive(Clone, Default)]
pub struct FindSession {
    _private: (),
}
#[cfg(not(windows))]
impl FindSession {
    pub fn cancel(&self) {}
}

/// Empty queries are explicit resets, never a new search session.
#[cfg(windows)]
pub async fn stop_find(view: &Webview, session: &FindSession) -> Result<(), String> {
    use std::{io::ErrorKind, time::Duration};
    let request = session.begin();
    let (tx, rx) = tokio::sync::oneshot::channel();
    view.with_webview(move |platform| {
        // An older queued reset must never stop a newer Find session.
        if request.check().is_err() {
            let _ = tx.send(Ok(()));
            return;
        }
        let result = latte_work_windows_webview::stop_find(&platform.controller(), &request);
        match result {
            Err(error) if error.kind() == ErrorKind::Unsupported => {
                // The DOM fallback owns only the page selection, no native session.
                latte_work_windows_webview::execute_script(
                    &platform.controller(),
                    "window.getSelection()?.removeAllRanges()",
                    move |result| {
                        let _ = tx.send(result.map(|_| ()));
                    },
                );
            }
            result => {
                let _ = tx.send(result);
            }
        }
    })
    .map_err(|e| e.to_string())?;
    tokio::time::timeout(Duration::from_secs(5), rx)
        .await
        .map_err(|_| "停止页面查找超时".to_string())?
        .map_err(|_| "浏览器操作已取消".to_string())?
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
pub async fn stop_find(view: &Webview, session: &FindSession) -> Result<(), String> {
    run(view, "find", "", false, true, session)
        .await
        .map(|_| ())
}

#[cfg(not(any(target_os = "macos", windows)))]
pub async fn stop_find(_view: &Webview, _session: &FindSession) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
pub async fn run(
    view: &Webview,
    operation: &str,
    query: &str,
    backwards: bool,
    _restart: bool,
    _session: &FindSession,
) -> Result<(Value, Option<Vec<u8>>), String> {
    use std::{os::unix::fs::DirBuilderExt, time::Duration};
    let nonce = uuid::Uuid::new_v4().to_string();
    let root =
        std::env::temp_dir().join(format!("latte-work-browser-tools-{}", std::process::id()));
    match std::fs::DirBuilder::new().mode(0o700).create(&root) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.to_string()),
    }
    let directory = root.join(&nonce);
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .map_err(|e| e.to_string())?;
    let result = async {
        let (send, receive) = tokio::sync::oneshot::channel();
        view.with_webview(move |platform| { let _ = send.send(platform.inner() as usize); }).map_err(|e| e.to_string())?;
        let pointer = tokio::time::timeout(Duration::from_secs(5), receive).await.map_err(|_| "浏览器响应超时")?.map_err(|e| e.to_string())?;
        let packet = json!({"view":pointer.to_string(), "operation":operation, "query":query, "backwards":backwards});
        std::fs::write(directory.join("request.json"), serde_json::to_vec(&packet).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        view.eval(format!("window.webkit.messageHandlers.latteBrowserTools.postMessage({});", serde_json::to_string(&nonce).map_err(|e| e.to_string())?)).map_err(|e| e.to_string())?;
        let response = tokio::time::timeout(Duration::from_secs(10), async {
            let path = directory.join("result.json");
            loop {
                if let Ok(meta) = std::fs::metadata(&path) {
                    let limit = match operation { "site-info" => 128 * 1024, "site-data" => 8192, _ => 4096 };
                    if meta.len() > limit { return Err("浏览器响应过大".to_string()); }
                    return std::fs::read(&path).map_err(|e| e.to_string());
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }).await.map_err(|_| "浏览器操作超时，请重试")??;
        let value: Value = serde_json::from_slice(&response).map_err(|e| e.to_string())?;
        if let Some(error) = value.get("error").and_then(Value::as_str) { return Err(error.to_string()); }
        let image = if matches!(operation, "capture" | "preview") {
            let preview = operation == "preview";
            let path = directory.join(if preview { "preview.jpg" } else { "capture.png" });
            if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > (if preview { 8 } else { 64 }) * 1024 * 1024 { return Err("截图过大".into()); }
            Some(std::fs::read(path).map_err(|e| e.to_string())?)
        } else { None };
        Ok((value, image))
    }.await;
    // Remove only the private directory created by this invocation. A late
    // native callback cannot recreate it or write outside it.
    let _ = std::fs::remove_dir_all(directory);
    result
}

#[cfg(windows)]
pub async fn run(
    view: &Webview,
    operation: &str,
    query: &str,
    backwards: bool,
    restart: bool,
    session: &FindSession,
) -> Result<(Value, Option<Vec<u8>>), String> {
    use serde_json::json;
    use std::time::Duration;
    match operation {
        "find" => {
            if query.is_empty() {
                stop_find(view, session).await?;
                return Ok((json!({ "found": false }), None));
            }
            let request = session.begin();
            let native_request = request.clone();
            let fallback = format!(
                "window.find({}, false, {}, true)",
                serde_json::to_string(query).map_err(|e| e.to_string())?,
                backwards,
            );
            let query = query.to_owned();
            let (tx, rx) = tokio::sync::oneshot::channel();
            view.with_webview(move |platform| {
                latte_work_windows_webview::find(
                    &platform.controller(),
                    &platform.environment(),
                    &query,
                    backwards,
                    restart,
                    native_request,
                    move |outcome| {
                        let _ = tx.send(outcome);
                    },
                );
            })
            .map_err(|e| e.to_string())?;
            let outcome = match tokio::time::timeout(Duration::from_secs(12), rx).await {
                Ok(result) => result.map_err(|_| "浏览器操作已取消".to_string())?,
                Err(_) => {
                    stop_find(view, session).await?;
                    return Err("浏览器操作超时，请重试".into());
                }
            };
            request.check().map_err(|e| e.to_string())?;
            match outcome {
                Ok(outcome) => Ok((json!({ "found": outcome.found }), None)),
                Err(error) if error.kind() == std::io::ErrorKind::Unsupported => {
                    // Older runtimes: degrade to the DOM search instead of failing.
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let fallback_request = request.clone();
                    view.with_webview(move |platform| {
                        if let Err(error) = fallback_request.check() {
                            let _ = tx.send(Err(error));
                            return;
                        }
                        latte_work_windows_webview::execute_script(
                            &platform.controller(),
                            &fallback,
                            move |result| {
                                let _ = tx.send(result);
                            },
                        );
                    })
                    .map_err(|e| e.to_string())?;
                    let result = match tokio::time::timeout(Duration::from_secs(12), rx).await {
                        Ok(result) => result,
                        Err(_) => {
                            stop_find(view, session).await?;
                            return Err("浏览器操作超时，请重试".into());
                        }
                    }
                    .map_err(|_| "浏览器操作已取消".to_string())?
                    .map_err(|e| e.to_string())?;
                    let found = serde_json::from_str::<bool>(&result).unwrap_or(false);
                    request.check().map_err(|e| e.to_string())?;
                    Ok((json!({ "found": found }), None))
                }
                Err(error) => Err(error.to_string()),
            }
        }
        "capture" => {
            let (tx, rx) = tokio::sync::oneshot::channel();
            view.with_webview(move |platform| {
                latte_work_windows_webview::capture_png(&platform.controller(), move |result| {
                    let _ = tx.send(result);
                });
            })
            .map_err(|e| e.to_string())?;
            let bytes = tokio::time::timeout(Duration::from_secs(15), rx)
                .await
                .map_err(|_| "浏览器操作超时，请重试".to_string())?
                .map_err(|_| "浏览器操作已取消".to_string())?
                .map_err(|e| e.to_string())?;
            Ok((json!({}), Some(bytes)))
        }
        _ => Err("浏览器操作无效".into()),
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub async fn run(
    _view: &Webview,
    _operation: &str,
    _query: &str,
    _backwards: bool,
    _restart: bool,
    _session: &FindSession,
) -> Result<(Value, Option<Vec<u8>>), String> {
    Err("此浏览器功能目前仅支持 macOS 与 Windows".into())
}
