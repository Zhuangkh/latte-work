//! Explicit, bounded local preview copies. The host has no GUI dependency.
static JOBS: std::sync::Mutex<Vec<tauri::async_runtime::JoinHandle<()>>> =
    std::sync::Mutex::new(Vec::new());
pub async fn close() {
    let jobs = JOBS
        .lock()
        .map(|mut jobs| std::mem::take(&mut *jobs))
        .unwrap_or_default();
    for job in &jobs {
        job.abort();
    }
    for job in jobs {
        let _ = job.await;
    }
}
struct Copy {
    folder: std::path::PathBuf,
    path: std::path::PathBuf,
    thumbnail: std::path::PathBuf,
}
impl Drop for Copy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(&self.thumbnail);
        let _ = std::fs::remove_dir(&self.folder);
    }
}
fn validate(name: &str, data: &[u8], limit: usize) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 256
        || name.contains(['/', '\\', '\0'])
        || matches!(name, "." | "..")
        || name.chars().any(char::is_control)
        || data.len() > limit
    {
        return Err("预览文件名无效或内容超过预览上限".into());
    }
    Ok(())
}
/// Keep the cache bounded even after forced exits that skip `Copy::drop`.
fn prune(root: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut folders: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let modified = entry.metadata().ok()?.modified().ok()?;
            path.is_dir().then_some((modified, path))
        })
        .collect();
    if folders.len() < 20 {
        return;
    }
    folders.sort_by_key(|(modified, _)| *modified);
    for (_, path) in folders.iter().take(folders.len() - 19) {
        let _ = std::fs::remove_dir_all(path);
    }
}
fn local_copy(app: &tauri::AppHandle, name: String, data: Vec<u8>) -> Result<Copy, String> {
    use tauri::Manager;
    let root = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("source-previews");
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    if latte_work_platform::fs::is_link(&root.symlink_metadata().map_err(|e| e.to_string())?) {
        return Err("预览缓存不能是符号链接".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    prune(&root);
    let folder = root.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&folder).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let copy = Copy {
        path: folder.join(&name),
        thumbnail: folder.join(format!("{name}.png")),
        folder,
    };
    std::fs::write(&copy.path, data).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&copy.path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(copy)
}
#[cfg(target_os = "macos")]
fn quicklook() -> tokio::process::Command {
    let mut command = tokio::process::Command::new("/usr/bin/qlmanage");
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    command
}
#[tauri::command]
pub async fn preview_pdf(
    app: tauri::AppHandle,
    name: String,
    data: Vec<u8>,
) -> Result<Vec<u8>, String> {
    validate(&name, &data, 16 * 1024 * 1024)?;
    if !data.starts_with(b"%PDF-") {
        return Err("来源不是 PDF 文件".into());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, name, data);
        Err("PDF 首页预览暂不支持此平台，可使用系统预览".into())
    }
    #[cfg(target_os = "macos")]
    {
        use tokio::io::AsyncReadExt;
        static RENDERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
        let _permit = RENDERS
            .try_acquire()
            .map_err(|_| "正在生成其他 PDF 预览，请稍后刷新".to_string())?;
        let copy = local_copy(&app, name, data)?;
        let status = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            quicklook()
                .args(["-t", "-s", "1400", "-o"])
                .arg(&copy.folder)
                .arg(&copy.path)
                .status(),
        )
        .await
        .map_err(|_| "PDF 预览生成超时".to_string())?
        .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("无法生成 PDF 预览，请使用系统预览".into());
        }
        let mut png = vec![];
        tokio::fs::File::open(&copy.thumbnail)
            .await
            .map_err(|e| e.to_string())?
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut png)
            .await
            .map_err(|e| e.to_string())?;
        if png.len() > 8 * 1024 * 1024 || !png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err("PDF 预览图无效或超过上限".into());
        }
        Ok(png)
    }
}
#[tauri::command]
pub async fn system_preview(
    app: tauri::AppHandle,
    name: String,
    data: Vec<u8>,
) -> Result<(), String> {
    validate(&name, &data, 32 * 1024 * 1024)?;
    use tauri::Manager;
    if app
        .state::<crate::Connections>()
        .quitting
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err("应用正在退出".into());
    }
    let copy = local_copy(&app, name, data)?;
    #[cfg(target_os = "macos")]
    {
        static PREVIEWS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
        let permit = PREVIEWS
            .try_acquire()
            .map_err(|_| "已有两个来源在系统预览中，请先关闭其中一个".to_string())?;
        let mut child = quicklook()
            .arg("-p")
            .arg(&copy.path)
            .spawn()
            .map_err(|e| e.to_string())?;
        let mut jobs = JOBS.lock().map_err(|_| "预览状态不可用".to_string())?;
        jobs.retain(|job| !job.inner().is_finished());
        jobs.push(tauri::async_runtime::spawn(async move {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3600), child.wait()).await;
            drop(child);
            drop(copy);
            drop(permit);
        }));
    }
    #[cfg(not(target_os = "macos"))]
    {
        use tauri_plugin_opener::OpenerExt;
        let path = copy.path.to_str().ok_or("预览路径不是有效 UTF-8")?;
        app.opener()
            .open_path(path, None::<&str>)
            .map_err(|e| format!("无法使用系统默认应用打开：{e}"))?;
        let mut jobs = JOBS.lock().map_err(|_| "预览状态不可用".to_string())?;
        jobs.retain(|job| !job.inner().is_finished());
        jobs.push(tauri::async_runtime::spawn(async move {
            // The external app owns the file; keep the copy bounded and delete it on quit.
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            drop(copy);
        }));
    }
    Ok(())
}
