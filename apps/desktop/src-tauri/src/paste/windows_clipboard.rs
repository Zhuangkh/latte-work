use clipboard_win::Getter;
use std::path::PathBuf;

pub(super) fn file_paths<Guard, Error>(
    acquire: impl FnOnce() -> Result<Guard, Error>,
    format: impl Getter<Vec<PathBuf>>,
) -> Result<Vec<String>, String> {
    // Keep the acquired RAII guard alive until the file-list read has finished.
    let _clipboard = match acquire() {
        Ok(clipboard) => clipboard,
        Err(_) => return Ok(Vec::new()),
    };
    // A text/image clipboard has no CF_HDROP; only real file lists are staged.
    let mut paths = Vec::new();
    if format.read_clipboard(&mut paths).is_err() {
        return Ok(Vec::new());
    }
    if paths.len() > 20 {
        return Err("一次最多粘贴 20 个附件".into());
    }
    paths
        .into_iter()
        .map(|path| {
            path.to_str()
                .map(str::to_owned)
                .ok_or_else(|| "剪贴板文件路径不是有效 UTF-8".to_string())
        })
        .collect()
}
