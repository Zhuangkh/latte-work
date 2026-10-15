//! Source inventories come only from explicit reference manifests and observed tools.
use anyhow::{Context, Result, bail};
use latte_work_protocol::{EventKind, FileEntry, SourceKind, TaskSource};
use std::{path::Path, time::Duration};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
const MARKER: &str = "\n文件引用（相对路径基于当前项目根目录；绝对路径位于当前执行主机。仅引用路径，按当前权限读取）：\n";
const COMPACT_MARKER: &str = "\n引用（相对项目；绝对路径在当前主机，按现有权限读取）：\n";
pub fn references(text: &str) -> Vec<TaskSource> {
    let Some((index, marker)) = [MARKER, COMPACT_MARKER]
        .into_iter()
        .filter_map(|marker| text.rfind(marker).map(|index| (index, marker)))
        .max_by_key(|(index, _)| *index)
    else {
        return vec![];
    };
    let Ok(serde_json::Value::Array(refs)) = serde_json::from_str(&text[index + marker.len()..])
    else {
        return vec![];
    };
    refs.iter()
        .take(20)
        .filter_map(|r| {
            let path = r["path"]
                .as_str()
                .filter(|p| !p.is_empty() && p.len() <= 4096 && !p.contains('\0'))?;
            let kind = match r["type"].as_str()? {
                "file" => SourceKind::File,
                "directory" => SourceKind::Directory,
                _ => return None,
            };
            Some(TaskSource {
                id: format!("file:{path}"),
                name: r["name"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 256)
                    .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(path))
                    .into(),
                kind,
                path: Some(path.into()),
                mime_type: r["mime_type"]
                    .as_str()
                    .filter(|s| s.len() <= 128)
                    .map(str::to_owned),
                tools: vec![],
                uses: 0,
            })
        })
        .collect()
}
pub fn tool_source(name: &str) -> Option<TaskSource> {
    if name.is_empty() || name.len() > 256 {
        return None;
    }
    let connector = name
        .strip_prefix("mcp__")
        .and_then(|s| s.split_once("__"))
        .map(|(server, _)| server);
    Some(TaskSource {
        id: format!(
            "{}:{}",
            if connector.is_some() {
                "connector"
            } else {
                "tool"
            },
            connector.unwrap_or(name)
        ),
        name: connector.unwrap_or(name).replace('_', " "),
        kind: if connector.is_some() {
            SourceKind::Connector
        } else {
            SourceKind::Tool
        },
        path: None,
        mime_type: None,
        tools: vec![name.into()],
        uses: 1,
    })
}
pub fn event_sources(event: &EventKind) -> Vec<TaskSource> {
    match event {
        EventKind::User { text, .. } => references(text),
        EventKind::Tool { name, .. } => tool_source(name).into_iter().collect(),
        _ => vec![],
    }
}
pub async fn preview(target: &Path, offset: u64) -> Result<latte_work_protocol::Response> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let metadata = tokio::fs::metadata(target).await?;
        if metadata.is_dir() {
            if offset != 0 {
                bail!("目录没有文件偏移");
            }
            let mut dir = tokio::fs::read_dir(target).await?;
            let mut entries = vec![];
            let mut truncated = false;
            while let Some(entry) = dir.next_entry().await? {
                if entries.len() >= 1000 {
                    truncated = true;
                    break;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if matches!(
                    name.as_str(),
                    ".git" | "node_modules" | "target" | ".DS_Store"
                ) {
                    continue;
                }
                let path = entry.path();
                // Never follow a directory symlink into a different preview scope.
                if path.symlink_metadata()?.file_type().is_symlink() {
                    continue;
                }
                let metadata = entry.metadata().await?;
                if !metadata.is_file() && !metadata.is_dir() {
                    continue;
                }
                entries.push(FileEntry {
                    name,
                    path: path.to_str().context("路径不是 UTF-8")?.into(),
                    directory: metadata.is_dir(),
                });
            }
            entries.sort_by(|a, b| b.directory.cmp(&a.directory).then(a.name.cmp(&b.name)));
            return Ok(latte_work_protocol::Response::SourceDirectory { entries, truncated });
        }
        if !metadata.is_file() || metadata.len() > 64 * 1024 * 1024 {
            bail!("预览仅支持不超过 64 MiB 的普通文件");
        }
        if offset > metadata.len() {
            bail!("预览偏移无效");
        }
        let mut file = tokio::fs::File::open(target).await?;
        let mut header = [0; 512];
        let n = file.read(&mut header).await?;
        let mime_type = mime(&header[..n], target).into();
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        let mut data = vec![];
        file.take(65536).read_to_end(&mut data).await?;
        let next = offset + data.len() as u64;
        Ok(latte_work_protocol::Response::SourcePreview {
            data,
            mime_type,
            size: metadata.len(),
            next,
            has_more: next < metadata.len(),
        })
    })
    .await
    .context("读取来源预览超时")?
}
fn mime(bytes: &[u8], path: &Path) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else if bytes.starts_with(b"%PDF-") {
        "application/pdf"
    } else if bytes.get(4..8) == Some(b"ftyp")
        && !matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("heic" | "heif" | "avif")
        )
    {
        "video/mp4"
    } else if bytes.starts_with(b"\x1a\x45\xdf\xa3")
        && path.extension().and_then(|s| s.to_str()) == Some("webm")
    {
        "video/webm"
    } else if !bytes.contains(&0)
        && match std::str::from_utf8(bytes) {
            Ok(_) => true,
            Err(error) => error.error_len().is_none(),
        }
    {
        "text/plain"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_manifests_and_connector_names_are_the_only_source_inventory() {
        assert!(references("ordinary path /private/file").is_empty());
        let sources = references(&format!(
            "read{MARKER}[{{\"path\":\"/private/image.png\",\"type\":\"file\",\"name\":\"image.png\",\"mime_type\":\"image/png\"}},{{\"path\":\"bad\",\"type\":\"other\"}}]"
        ));
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].mime_type.as_deref(), Some("image/png"));
        let tool = tool_source("mcp__lark_docs__read").unwrap();
        assert_eq!(tool.kind, SourceKind::Connector);
        assert_eq!(tool.id, "connector:lark_docs");
        assert_eq!(tool.tools, vec!["mcp__lark_docs__read"]);
    }
    #[test]
    fn compact_manifests_preserve_paths_and_original_names_without_mime_metadata() {
        let sources = references(&format!(
            "read{COMPACT_MARKER}{}",
            serde_json::json!([
                {"path":"/private/cache/uuid-image.png","type":"file","name":"image.png","kind":"image"},
                {"path":"src/中文 \"file\".rs","type":"file"},
                {"path":"src","type":"directory"}
            ])
        ));
        assert_eq!(sources.len(), 3);
        assert_eq!(sources[0].name, "image.png");
        assert_eq!(
            sources[0].path.as_deref(),
            Some("/private/cache/uuid-image.png")
        );
        assert_eq!(sources[0].mime_type, None);
        assert_eq!(sources[1].name, "中文 \"file\".rs");
        assert_eq!(sources[2].kind, SourceKind::Directory);
        assert!(references(&format!("read{COMPACT_MARKER}[] trailing prose")).is_empty());
    }
    #[tokio::test]
    async fn preview_chunks_and_sniffs_content_without_rendering_html_or_svg() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("text");
        let text = "中文".repeat(22000);
        std::fs::write(&path, &text).unwrap();
        match preview(&path, 0).await.unwrap() {
            latte_work_protocol::Response::SourcePreview {
                data,
                next,
                has_more,
                mime_type,
                size,
            } => {
                assert_eq!(data.len(), 65536);
                assert_eq!(next, 65536);
                assert!(has_more);
                assert_eq!(mime_type, "text/plain");
                assert_eq!(size, text.len() as u64);
            }
            other => panic!("{other:?}"),
        }
        assert!(preview(&path, text.len() as u64 + 1).await.is_err());
        assert_eq!(
            mime(
                b"<svg><script>alert(1)</script></svg>",
                Path::new("image.svg")
            ),
            "text/plain"
        );
        assert_eq!(mime(b"%PDF-1.7", Path::new("binary")), "application/pdf");
        assert_eq!(mime(b"\x89PNG\r\n\x1a\n", Path::new("text")), "image/png");
    }
}
