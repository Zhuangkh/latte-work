//! Bounded, one-shot WebKit tools without unsafe Rust or remote IPC permissions.
use serde_json::Value;
#[cfg(target_os = "macos")]
use serde_json::json;
use tauri::Webview;

#[cfg(target_os = "macos")]
pub async fn run(
    view: &Webview,
    operation: &str,
    query: &str,
    backwards: bool,
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
                    if meta.len() > 4096 { return Err("浏览器响应过大".to_string()); }
                    return std::fs::read(&path).map_err(|e| e.to_string());
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }).await.map_err(|_| "浏览器操作超时，请重试")??;
        let value: Value = serde_json::from_slice(&response).map_err(|e| e.to_string())?;
        if let Some(error) = value.get("error").and_then(Value::as_str) { return Err(error.to_string()); }
        let image = if operation == "capture" {
            let path = directory.join("capture.png");
            if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 64 * 1024 * 1024 { return Err("截图过大".into()); }
            Some(std::fs::read(path).map_err(|e| e.to_string())?)
        } else { None };
        Ok((value, image))
    }.await;
    // Remove only the private directory created by this invocation. A late
    // native callback cannot recreate it or write outside it.
    let _ = std::fs::remove_dir_all(directory);
    result
}
#[cfg(not(target_os = "macos"))]
pub async fn run(
    _view: &Webview,
    _operation: &str,
    _query: &str,
    _backwards: bool,
) -> Result<(Value, Option<Vec<u8>>), String> {
    Err("此浏览器功能目前仅支持 macOS".into())
}
