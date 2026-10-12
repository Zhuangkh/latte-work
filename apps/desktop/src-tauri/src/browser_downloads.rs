//! Native downloads retain the original WebKit request (including cookies/POST).
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Mutex};
use tauri::{AppHandle, Emitter, Manager, Webview, webview::DownloadEvent};
use tauri_plugin_dialog::DialogExt;

const LIMIT: usize = 100;
const MAX_BYTES: u64 = 128 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Download {
    id: String,
    name: String,
    url: String,
    status: String,
    path: Option<PathBuf>,
    error: Option<String>,
    #[serde(skip)]
    temporary: Option<PathBuf>,
}
#[derive(Default)]
pub struct Downloads {
    items: Mutex<Option<Vec<Download>>>,
    save: tokio::sync::Mutex<()>,
}
fn file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("browser-downloads.json"))
}
fn load(app: &AppHandle) -> Result<Vec<Download>, String> {
    let path = file(app)?;
    if !path.exists() {
        return Ok(vec![]);
    }
    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 1024 * 1024 {
        return Err("下载记录过大".into());
    }
    let items = restore(&std::fs::read(path).map_err(|e| e.to_string())?)?;
    let cache = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("browser-downloads");
    for item in &items {
        if item.status == "interrupted" {
            // This is an app-generated UUID .part file, never a user save path.
            let _ = std::fs::remove_file(cache.join(format!("{}.part", item.id)));
        }
    }
    Ok(items)
}
fn restore(bytes: &[u8]) -> Result<Vec<Download>, String> {
    let mut items: Vec<Download> = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if items.len() > LIMIT {
        return Err("下载记录过多".into());
    }
    for item in &mut items {
        if uuid::Uuid::parse_str(&item.id).is_err()
            || item.name.chars().count() > 180
            || item.url.len() > 8192
            || !matches!(
                item.status.as_str(),
                "downloading" | "saving" | "completed" | "failed" | "interrupted" | "cancelled"
            )
            || item.path.as_ref().is_some_and(|path| !path.is_absolute())
        {
            return Err("下载记录无效".into());
        }
        if item.status == "downloading" || item.status == "saving" {
            item.status = "interrupted".into();
            item.error = Some("应用退出时下载尚未完成".into());
        }
    }
    Ok(items)
}
fn change<T>(
    app: &AppHandle,
    edit: impl FnOnce(&mut Vec<Download>) -> Result<T, String>,
) -> Result<T, String> {
    let state = app.state::<Downloads>();
    let mut guard = state.items.lock().map_err(|e| e.to_string())?;
    if guard.is_none() {
        *guard = Some(load(app)?);
    }
    let items = guard.as_mut().ok_or("下载记录不可用")?;
    let mut next = items.clone();
    let result = edit(&mut next)?;
    let path = file(app)?;
    std::fs::create_dir_all(path.parent().ok_or("下载记录路径无效")?).map_err(|e| e.to_string())?;
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, serde_json::to_vec(&next).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    std::fs::rename(temp, path).map_err(|e| e.to_string())?;
    *items = next;
    let _ = app.emit_to("main", "browser-downloads", &*items);
    Ok(result)
}
pub fn safe_name(value: &str) -> String {
    let name: String = value
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\' | ':'))
        .take(180)
        .collect();
    if name.trim_matches('.').trim().is_empty() {
        "下载文件".into()
    } else {
        name
    }
}
fn fail(app: &AppHandle, error: String) {
    let _ = app.emit_to("main", "browser-download-error", error);
}
pub fn handle(view: Webview, event: DownloadEvent<'_>) -> bool {
    let app = view.app_handle().clone();
    match event {
        DownloadEvent::Requested { url, destination } => {
            let result = change(&app, |items| {
                if items
                    .iter()
                    .filter(|item| matches!(item.status.as_str(), "downloading" | "saving"))
                    .count()
                    >= 4
                {
                    return Err("最多同时下载 4 个文件".into());
                }
                if items
                    .iter()
                    .any(|item| item.status == "downloading" && item.url == url.as_str())
                {
                    return Err("此文件正在下载".into());
                }
                let valid_url = url.as_str().strip_prefix("blob:").unwrap_or(url.as_str());
                super::browser::validate_link(valid_url, app.config().build.dev_url.as_ref())?;
                let id = uuid::Uuid::new_v4().to_string();
                let name = safe_name(
                    destination
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("下载文件"),
                );
                let directory = app
                    .path()
                    .app_cache_dir()
                    .map_err(|e| e.to_string())?
                    .join("browser-downloads");
                std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
                *destination = directory.join(format!("{id}.part"));
                while items.len() >= LIMIT {
                    let index = items
                        .iter()
                        .position(|item| !matches!(item.status.as_str(), "downloading" | "saving"))
                        .ok_or("下载记录已满")?;
                    items.remove(index);
                }
                items.push(Download {
                    id,
                    name,
                    url: url.to_string(),
                    status: "downloading".into(),
                    path: None,
                    error: None,
                    temporary: Some(destination.clone()),
                });
                Ok(())
            });
            if let Err(error) = result {
                fail(&app, error);
                return false;
            }
            true
        }
        DownloadEvent::Finished { url, path, success } => {
            let result = change(&app, |items| {
                let item = items
                    .iter_mut()
                    .find(|item| {
                        item.status == "downloading"
                            && path.as_ref().map_or(item.url == url.as_str(), |path| {
                                item.temporary.as_ref() == Some(path)
                            })
                    })
                    .ok_or("下载记录未找到")?;
                if !success {
                    item.status = "failed".into();
                    item.error = Some("下载失败".into());
                } else {
                    item.status = "saving".into();
                }
                Ok(item.clone())
            });
            match result {
                Ok(item) => {
                    tauri::async_runtime::spawn(save_finished(app, item, success));
                }
                Err(error) => fail(&app, error),
            }
            true
        }
        _ => false,
    }
}
async fn save_finished(app: AppHandle, item: Download, success: bool) {
    let state = app.state::<Downloads>();
    let _save = state.save.lock().await;
    let result = async {
        let source = item.temporary.as_ref().ok_or("临时下载文件未找到")?;
        if !success {
            return Err("下载失败".to_string());
        }
        let size = std::fs::metadata(source).map_err(|e| e.to_string())?.len();
        if size > MAX_BYTES {
            return Err("文件超过 128 MiB 下载上限，请使用系统浏览器".into());
        }
        let destination = choose_save(&app, &item.name, None).await?;
        if let Some(path) = destination {
            let source = source.clone();
            let saved = path.clone();
            tauri::async_runtime::spawn_blocking(move || std::fs::copy(source, saved))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }
    .await;
    if let Some(source) = &item.temporary {
        let _ = std::fs::remove_file(source);
    }
    let updated = change(&app, |items| {
        let current = items
            .iter_mut()
            .find(|entry| entry.id == item.id)
            .ok_or("下载记录未找到")?;
        current.temporary = None;
        match result {
            Ok(Some(path)) => {
                current.path = Some(path);
                current.status = "completed".into();
            }
            Ok(None) => {
                current.status = "cancelled".into();
            }
            Err(error) => {
                current.status = "failed".into();
                current.error = Some(error);
            }
        }
        Ok(())
    });
    if let Err(error) = updated {
        fail(&app, error);
    }
}
pub async fn choose_save(
    app: &AppHandle,
    name: &str,
    extension: Option<&str>,
) -> Result<Option<PathBuf>, String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let mut dialog = app
        .dialog()
        .file()
        .set_title("保存文件")
        .set_file_name(safe_name(name));
    if let Some(extension) = extension {
        dialog = dialog.add_filter(extension.to_uppercase(), &[extension]);
    }
    dialog.save_file(move |file| {
        let _ = send.send(
            file.map(|file| file.into_path().map_err(|e| e.to_string()))
                .transpose(),
        );
    });
    receive.await.map_err(|e| e.to_string())?
}
#[tauri::command]
pub fn browser_downloads(view: Webview) -> Result<Vec<Download>, String> {
    super::browser::check_caller(&view)?;
    change(view.app_handle(), |items| Ok(items.clone()))
}
#[tauri::command]
pub fn browser_clear_downloads(view: Webview) -> Result<(), String> {
    super::browser::check_caller(&view)?;
    change(view.app_handle(), |items| {
        items.retain(|item| matches!(item.status.as_str(), "downloading" | "saving"));
        Ok(())
    })
}
#[tauri::command]
pub async fn browser_reveal_download(view: Webview, id: String) -> Result<(), String> {
    super::browser::check_caller(&view)?;
    let path = change(view.app_handle(), |items| {
        items
            .iter()
            .find(|item| item.id == id && item.status == "completed")
            .and_then(|item| item.path.clone())
            .ok_or("下载文件未找到".to_string())
    })?;
    if !path.is_file() {
        return Err("下载文件已移动或删除".into());
    }
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = tokio::process::Command::new("open");
        command.arg("-R");
        command
    };
    #[cfg(not(target_os = "macos"))]
    let mut command = tokio::process::Command::new("xdg-open");
    let status = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        command.arg(path).kill_on_drop(true).status(),
    )
    .await
    .map_err(|_| "打开文件位置超时")?
    .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("无法显示下载文件".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn download_names_cannot_escape_the_picker_directory() {
        assert_eq!(safe_name("../a\\b\n:file.pdf"), "..abfile.pdf");
        assert_eq!(safe_name(".."), "下载文件");
        assert_eq!(safe_name(&"x".repeat(1000)).len(), 180);
    }
    #[test]
    fn restarting_never_infers_download_success_or_replays_it() {
        let id = uuid::Uuid::new_v4().to_string();
        let bytes = serde_json::to_vec(&vec![Download {
            id,
            name: "a.pdf".into(),
            url: "https://example.com/a.pdf".into(),
            status: "saving".into(),
            path: None,
            error: None,
            temporary: Some(PathBuf::from("/tmp/not-persisted")),
        }])
        .unwrap();
        let restored = restore(&bytes).unwrap();
        assert_eq!(restored[0].status, "interrupted");
        assert!(restored[0].path.is_none());
        assert!(restored[0].temporary.is_none());
        let malicious = br#"[{"id":"invalid","name":"a","url":"x","status":"completed","path":"relative","error":null}]"#;
        assert!(restore(malicious).is_err());
    }
}
