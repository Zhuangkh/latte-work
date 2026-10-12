//! App-owned browser surfaces. Remote pages never receive main-WebView capabilities.
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, Webview, WebviewUrl, webview::WebviewBuilder};

fn caller(view: &Webview) -> Result<(), String> {
    if view.label() != "main" {
        return Err("浏览器操作只能由工作台发起".into());
    }
    Ok(())
}
pub(crate) fn check_caller(view: &Webview) -> Result<(), String> {
    caller(view)
}
pub(crate) fn validate_link(value: &str, dev: Option<&tauri::Url>) -> Result<tauri::Url, String> {
    parse_url(value, dev)
}
fn label(id: &str) -> Result<String, String> {
    let id = uuid::Uuid::parse_str(id).map_err(|_| "浏览器标签无效")?;
    Ok(format!("browser-{id}"))
}
fn allowed(url: &tauri::Url, dev: Option<&tauri::Url>) -> bool {
    url.as_str().len() <= 8192
        && matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && !matches!(url.host_str(), Some("tauri.localhost" | "ipc.localhost"))
        && !dev.is_some_and(|dev| dev.origin() == url.origin())
}
fn parse_url(value: &str, dev: Option<&tauri::Url>) -> Result<tauri::Url, String> {
    if value.len() > 8192 {
        return Err("链接过长".into());
    }
    let url = tauri::Url::parse(value).map_err(|_| "请输入完整的 HTTP 或 HTTPS 链接")?;
    if !allowed(&url, dev) {
        return Err("仅支持打开普通 HTTP 或 HTTPS 网页".into());
    }
    Ok(url)
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserEvent {
    id: String,
    url: String,
    title: Option<String>,
    record: bool,
}
fn notify(view: &Webview, id: String, url: String, title: Option<String>, record: bool) {
    let _ = view.app_handle().emit_to(
        "main",
        "browser-location",
        BrowserEvent {
            id,
            url,
            title,
            record,
        },
    );
}
#[derive(Deserialize)]
pub struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}
impl Bounds {
    fn validate(&self) -> Result<(), String> {
        if [self.x, self.y, self.width, self.height]
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0 || *v > 32768.0)
            || self.width < 1.0
            || self.height < 1.0
        {
            return Err("浏览器区域无效".into());
        }
        Ok(())
    }
}
#[tauri::command]
pub async fn browser_create(
    view: Webview,
    id: String,
    url: String,
    bounds: Bounds,
) -> Result<(), String> {
    caller(&view)?;
    bounds.validate()?;
    let name = label(&id)?;
    let app = view.app_handle();
    if app
        .webviews()
        .keys()
        .filter(|key| key.starts_with("browser-"))
        .count()
        >= 32
    {
        return Err("最多打开 32 个浏览器标签".into());
    }
    let dev = app.config().build.dev_url.clone();
    let initial = if url.is_empty() {
        tauri::Url::parse("about:blank").map_err(|e| e.to_string())?
    } else {
        parse_url(&url, dev.as_ref())?
    };
    let page_id = id.clone();
    let title_id = id.clone();
    let popup_id = id.clone();
    let popup_app = app.clone();
    let popup_dev = dev.clone();
    let navigation_app = app.clone();
    let builder = WebviewBuilder::new(name, WebviewUrl::External(initial))
        // macos_browser.m replaces this nonpersistent store with a shared,
        // isolated browser profile (persistent on macOS 14+).
        .incognito(true)
        .on_navigation(move |url| {
            // A genuine navigation supersedes a data-clear refresh that failed.
            let tools = navigation_app.state::<Tools>();
            if let Ok(mut ignored) = tools.suppressed.lock() {
                let name = format!("browser-{page_id}");
                if ignored.get(&name).is_some_and(|old| old != url.as_str()) {
                    ignored.remove(&name);
                }
            }
            url.as_str() == "about:blank" || allowed(url, dev.as_ref())
        })
        .on_page_load(move |view, payload| {
            let finished = matches!(payload.event(), tauri::webview::PageLoadEvent::Finished);
            let ignored = finished
                && view
                    .app_handle()
                    .state::<Tools>()
                    .suppressed
                    .lock()
                    .map(|mut ignored| ignored.remove(view.label()).is_some())
                    .unwrap_or(true);
            notify(
                &view,
                id.clone(),
                payload.url().to_string(),
                None,
                finished && !ignored,
            );
        })
        .on_document_title_changed(move |view, title| {
            if let Ok(url) = view.url() {
                notify(
                    &view,
                    title_id.clone(),
                    url.to_string(),
                    Some(title.chars().take(256).collect()),
                    false,
                );
            }
        })
        .on_new_window(move |url, _| {
            if allowed(&url, popup_dev.as_ref()) {
                let _ = popup_app.emit_to(
                    "main",
                    "browser-new-tab",
                    BrowserEvent {
                        id: popup_id.clone(),
                        url: url.to_string(),
                        title: None,
                        record: false,
                    },
                );
            }
            tauri::webview::NewWindowResponse::Deny
        })
        .on_download(super::browser_downloads::handle);
    let browser = view
        .window()
        .add_child(
            builder,
            tauri::LogicalPosition::new(bounds.x, bounds.y),
            tauri::LogicalSize::new(bounds.width, bounds.height),
        )
        .map_err(|e| e.to_string())?;
    browser.hide().map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn browser_sync(view: Webview, id: String, bounds: Option<Bounds>) -> Result<(), String> {
    caller(&view)?;
    let browser = view
        .app_handle()
        .get_webview(&label(&id)?)
        .ok_or("浏览器标签已关闭")?;
    if let Some(bounds) = bounds {
        bounds.validate()?;
        browser
            .set_bounds(tauri::Rect {
                position: tauri::LogicalPosition::new(bounds.x, bounds.y).into(),
                size: tauri::LogicalSize::new(bounds.width, bounds.height).into(),
            })
            .map_err(|e| e.to_string())?;
        browser.show().map_err(|e| e.to_string())
    } else {
        browser.hide().map_err(|e| e.to_string())
    }
}
#[tauri::command]
pub async fn browser_action(
    view: Webview,
    id: String,
    action: String,
    url: Option<String>,
    zoom: Option<f64>,
) -> Result<(), String> {
    caller(&view)?;
    let browser = view
        .app_handle()
        .get_webview(&label(&id)?)
        .ok_or("浏览器标签已关闭")?;
    let result = match action.as_str() {
        "navigate" => browser.navigate(parse_url(
            url.as_deref().unwrap_or(""),
            view.app_handle().config().build.dev_url.as_ref(),
        )?),
        "back" => browser.eval("history.back()"),
        "forward" => browser.eval("history.forward()"),
        "reload" => browser.reload(),
        "print" => browser.print(),
        "zoom" => {
            let scale = zoom.ok_or("缺少缩放比例")?;
            if !scale.is_finite() || !(0.25..=3.0).contains(&scale) {
                return Err("缩放比例无效".into());
            }
            browser.set_zoom(scale)
        }
        _ => return Err("浏览器操作无效".into()),
    };
    result.map_err(|e| e.to_string())
}
#[derive(Default)]
pub struct Tools {
    operation: tokio::sync::Mutex<()>,
    suppressed: std::sync::Mutex<std::collections::HashMap<String, String>>,
}
#[tauri::command]
pub async fn browser_find(
    view: Webview,
    id: String,
    query: String,
    backwards: bool,
) -> Result<bool, String> {
    caller(&view)?;
    if query.chars().count() > 512 {
        return Err("查找内容过长".into());
    }
    let state = view.app_handle().state::<Tools>();
    let _guard = state
        .operation
        .try_lock()
        .map_err(|_| "浏览器操作进行中，请稍后再试")?;
    let browser = view
        .app_handle()
        .get_webview(&label(&id)?)
        .ok_or("浏览器标签已关闭")?;
    let (result, _) = super::browser_native::run(&browser, "find", &query, backwards).await?;
    Ok(result
        .get("found")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false))
}
#[tauri::command]
pub async fn browser_capture(view: Webview, id: String) -> Result<Option<String>, String> {
    caller(&view)?;
    let state = view.app_handle().state::<Tools>();
    let _guard = state
        .operation
        .try_lock()
        .map_err(|_| "浏览器操作进行中，请稍后再试")?;
    let browser = view
        .app_handle()
        .get_webview(&label(&id)?)
        .ok_or("浏览器标签已关闭")?;
    let (_, bytes) = super::browser_native::run(&browser, "capture", "", false).await?;
    let bytes = bytes.ok_or("网页截图失败")?;
    if let Some(path) =
        super::browser_downloads::choose_save(view.app_handle(), "网页截图.png", Some("png"))
            .await?
    {
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        Ok(Some(path.to_string_lossy().into_owned()))
    } else {
        Ok(None)
    }
}
#[tauri::command]
pub async fn browser_clear_data(view: Webview, id: String) -> Result<(), String> {
    caller(&view)?;
    let state = view.app_handle().state::<Tools>();
    let _guard = state
        .operation
        .try_lock()
        .map_err(|_| "浏览器操作进行中，请稍后再试")?;
    let browser = view
        .app_handle()
        .get_webview(&label(&id)?)
        .ok_or("浏览器标签已关闭")?;
    super::browser_native::run(&browser, "clear", "", false).await?;
    for (name, browser) in view.app_handle().webviews() {
        if name.starts_with("browser-") {
            state
                .suppressed
                .lock()
                .map_err(|e| e.to_string())?
                .insert(name, browser.url().map_err(|e| e.to_string())?.to_string());
            browser.reload().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
#[tauri::command]
pub async fn browser_close(view: Webview, id: String) -> Result<(), String> {
    caller(&view)?;
    let name = label(&id)?;
    view.app_handle()
        .state::<Tools>()
        .suppressed
        .lock()
        .map_err(|e| e.to_string())?
        .remove(&name);
    if let Some(browser) = view.app_handle().get_webview(&name) {
        browser.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}
#[tauri::command]
pub async fn open_external_url(view: Webview, url: String) -> Result<(), String> {
    caller(&view)?;
    let url = parse_url(&url, view.app_handle().config().build.dev_url.as_ref())?;
    #[cfg(target_os = "macos")]
    let mut command = tokio::process::Command::new("open");
    #[cfg(not(target_os = "macos"))]
    let mut command = tokio::process::Command::new("xdg-open");
    let status = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        command.arg(url.as_str()).kill_on_drop(true).status(),
    )
    .await
    .map_err(|_| "打开系统浏览器超时")?
    .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("无法打开系统默认浏览器".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_web_urls_without_app_access_or_credentials_are_allowed() {
        let dev = tauri::Url::parse("http://127.0.0.1:1420").unwrap();
        for value in [
            "javascript:alert(1)",
            "file:///tmp/a",
            "tauri://localhost",
            "https://tauri.localhost",
            "http://ipc.localhost",
            "http://127.0.0.1:1420/settings",
            "https://user:pass@example.com",
            "data:text/html,x",
        ] {
            assert!(parse_url(value, Some(&dev)).is_err(), "{value}");
        }
        for value in ["https://example.com/a?q=b#c", "http://localhost:8080"] {
            assert!(parse_url(value, Some(&dev)).is_ok());
        }
        assert!(label("../../main").is_err());
    }
}
