//! Native clipboard file URLs and bounded staging/SSH transfer. No agent execution.
use crate::{Connections, local_client};
use latte_work_client::Client;
use latte_work_protocol::{FileEntry, Request, Response};
use std::{path::Path, sync::Arc};
use tauri::State;
use tokio::{io::AsyncReadExt, sync::Mutex};
const MAX_FILE: u64 = 64 * 1024 * 1024;
#[cfg(target_os = "windows")]
mod windows_clipboard;

#[tauri::command]
pub fn clipboard_file_paths() -> Result<Vec<String>, String> {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSPasteboard;
        use objc2_foundation::{NSString, NSURL};
        let pasteboard = NSPasteboard::generalPasteboard();
        let mut paths = Vec::new();
        if let Some(items) = pasteboard.pasteboardItems() {
            if items.len() > 20 {
                return Err("一次最多粘贴 20 个附件".into());
            }
            for item in items.iter() {
                if let Some(value) = item.stringForType(&NSString::from_str("public.file-url")) {
                    let url = NSURL::URLWithString(&value).ok_or("剪贴板文件 URL 无效")?;
                    if !url.isFileURL() {
                        return Err("剪贴板内容不是本机文件".into());
                    }
                    // Finder may publish /.file/id=... file-reference URLs.
                    // Foundation resolves them to usable filesystem paths.
                    let path = url
                        .filePathURL()
                        .and_then(|url| url.path())
                        .ok_or("无法解析剪贴板文件路径")?;
                    paths.push(path.to_string());
                }
            }
        }
        Ok(paths)
    }
    #[cfg(target_os = "windows")]
    {
        windows_clipboard::file_paths(
            || clipboard_win::Clipboard::new_attempts(10),
            clipboard_win::formats::FileList,
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Ok(Vec::new())
    }
}
async fn call(client: &Arc<Mutex<Client>>, request: Request) -> Result<Response, String> {
    let response = client
        .lock()
        .await
        .request(request)
        .await
        .map_err(|e| format!("{e:#}"))?;
    match response {
        Response::Error { message, .. } => Err(message),
        other => Ok(other),
    }
}
async fn target(state: &Connections, host_id: &str) -> Result<Arc<Mutex<Client>>, String> {
    state
        .clients
        .lock()
        .await
        .get(host_id)
        .cloned()
        .ok_or("项目主机未连接".into())
}
async fn upload(
    client: &Arc<Mutex<Client>>,
    path: &Path,
    name: String,
) -> Result<FileEntry, String> {
    let meta = tokio::fs::metadata(path).await.map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return Err("仅支持不超过 64 MiB 的普通文件；目录请使用＋引用".into());
    }
    let mut file = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::fs::File::open(path),
    )
    .await
    .map_err(|_| "打开附件超时")?
    .map_err(|e| e.to_string())?;
    let id = match call(
        client,
        Request::BeginAttachment {
            name,
            size: meta.len(),
        },
    )
    .await?
    {
        Response::AttachmentUpload { id } => id,
        _ => return Err("主机不支持粘贴附件，请更新 Server".into()),
    };
    let result = async {
        let mut offset = 0;
        let mut buffer = vec![0u8; 65536];
        loop {
            let n =
                tokio::time::timeout(std::time::Duration::from_secs(10), file.read(&mut buffer))
                    .await
                    .map_err(|_| "读取附件超时")?
                    .map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            if offset + n as u64 > meta.len() {
                return Err("文件在粘贴期间发生变化，请重试".into());
            }
            call(
                client,
                Request::AttachmentChunk {
                    id: id.clone(),
                    offset,
                    data: buffer[..n].to_vec(),
                },
            )
            .await?;
            offset += n as u64;
        }
        if offset != meta.len() {
            return Err("文件在粘贴期间发生变化，请重试".into());
        }
        match call(client, Request::FinishAttachment { id: id.clone() }).await? {
            Response::FileReference { entry } => Ok(entry),
            _ => Err("附件保存响应无效".into()),
        }
    }
    .await;
    if result.is_err() {
        let _ = call(client, Request::AbortAttachment { id }).await;
    }
    result
}
/// The WebView streams at most 64 KiB per call into the local app's host cache.
#[tauri::command]
pub async fn paste_stage(
    app: tauri::AppHandle,
    state: State<'_, Connections>,
    request: Request,
) -> Result<Response, String> {
    if !matches!(
        request,
        Request::BeginAttachment { .. }
            | Request::AttachmentChunk { .. }
            | Request::AbortAttachment { .. }
    ) {
        return Err("无效的附件暂存请求".into());
    }
    call(&local_client(&app, &state).await?, request).await
}
#[tauri::command]
pub async fn finish_paste_upload(
    app: tauri::AppHandle,
    state: State<'_, Connections>,
    host_id: String,
    id: String,
) -> Result<FileEntry, String> {
    let local = local_client(&app, &state).await?;
    let entry = match call(&local, Request::FinishAttachment { id }).await? {
        Response::FileReference { entry } => entry,
        _ => return Err("附件保存响应无效".into()),
    };
    if host_id == "local" {
        return Ok(entry);
    }
    upload(
        &target(&state, &host_id).await?,
        Path::new(&entry.path),
        entry.name.clone(),
    )
    .await
}
#[tauri::command]
pub async fn import_clipboard_file(
    app: tauri::AppHandle,
    state: State<'_, Connections>,
    host_id: String,
    path: String,
    project_id: String,
) -> Result<FileEntry, String> {
    // A path supplied by the UI must still be present in the native pasteboard.
    if !clipboard_file_paths()?.contains(&path) {
        return Err("剪贴板已变化，请重新粘贴".into());
    }
    let source = tokio::fs::canonicalize(&path)
        .await
        .map_err(|e| e.to_string())?;
    let local = local_client(&app, &state).await?;
    if source.is_dir() {
        if host_id != "local" {
            return Err("不能直接粘贴本机目录到远程；请先传输目录，再用＋引用远程路径".into());
        }
        return match call(
            &local,
            Request::ResolveReference {
                project_id,
                path: source.to_str().ok_or("路径不是 UTF-8")?.into(),
            },
        )
        .await?
        {
            Response::FileReference { entry } => Ok(entry),
            _ => Err("目录引用响应无效".into()),
        };
    }
    let name = Path::new(&path)
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("文件名无效")?
        .to_owned();
    let staged = upload(&local, &source, name).await?;
    if host_id == "local" {
        return Ok(staged);
    }
    upload(
        &target(&state, &host_id).await?,
        Path::new(&staged.path),
        staged.name.clone(),
    )
    .await
}
