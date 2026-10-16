//! Read-only project inspection, with canonical containment and bounded output.
use crate::managed_recovery::ManagedRecoveryExclusions;
use anyhow::{Context, Result, bail};
use latte_work_protocol::{ChangeSection, FileEntry, GitChange};
use std::{
    path::{Component, Path},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};
const LIMIT: usize = 512 * 1024;
/// Resolve an explicitly chosen host path for a prompt reference only. This does
/// not grant access, read contents, or relax project browsing/read containment.
pub async fn reference(root: &Path, path: &str) -> Result<FileEntry> {
    if path.len() > 4096 || path.contains('\0') || !Path::new(path).is_absolute() {
        bail!("请输入不超过 4096 字节的绝对路径");
    }
    latte_work_platform::paths::absolute(Path::new(path))?;
    tokio::time::timeout(Duration::from_secs(5), async {
        let target = tokio::fs::canonicalize(path)
            .await
            .context("路径不存在或无法访问")?;
        let metadata = tokio::fs::metadata(&target).await?;
        if !metadata.is_file() && !metadata.is_dir() {
            bail!("只能引用普通文件或目录");
        }
        let root = tokio::fs::canonicalize(root).await?;
        let path = if let Ok(relative) = target.strip_prefix(&root) {
            latte_work_platform::paths::relative_text(relative)?
        } else {
            target.to_str().context("路径不是 UTF-8")?.to_owned()
        };
        Ok(FileEntry {
            name: target
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("/")
                .into(),
            path: if path.is_empty() { ".".into() } else { path },
            directory: metadata.is_dir(),
        })
    })
    .await
    .context("读取路径超时，请检查主机或文件系统")?
}
/// Project selection precedes project registration: browse directory names only,
/// under the authenticated host user's filesystem permissions. No file content.
pub fn browse_directories(path: Option<&str>) -> Result<latte_work_protocol::Response> {
    let folder = match path {
        Some(path) => std::path::PathBuf::from(path),
        None => latte_work_platform::dirs::home_dir().context("无法确定主目录，请输入绝对路径")?,
    };
    if !folder.is_absolute() {
        bail!("目录必须是绝对路径");
    }
    latte_work_platform::paths::absolute(&folder)?;
    let folder = folder.canonicalize().context("目录不存在或无法访问")?;
    let path = folder.to_str().context("目录路径不是 UTF-8")?.to_owned();
    let name = folder
        .file_name()
        .unwrap_or_else(|| folder.as_os_str())
        .to_str()
        .context("目录名称不是 UTF-8")?
        .to_owned();
    let parent = folder
        .parent()
        .map(|p| p.to_str().map(str::to_owned).context("目录路径不是 UTF-8"))
        .transpose()?;
    let mut entries = Vec::new();
    let mut bytes = 0;
    let mut truncated = false;
    for (count, entry) in folder.read_dir()?.enumerate() {
        if count >= 5000 || entries.len() >= 1000 || bytes >= LIMIT {
            truncated = true;
            break;
        }
        let entry = entry?;
        let Ok(metadata) = entry.path().metadata() else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(path) = entry.path().to_str().map(str::to_owned) else {
            continue;
        };
        bytes += name.len() + path.len();
        entries.push(FileEntry {
            name,
            path,
            directory: true,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(latte_work_protocol::Response::Directories {
        path,
        name: Some(name),
        parent,
        entries,
        truncated,
    })
}
pub fn resolve(root: &Path, relative: &str) -> Result<std::path::PathBuf> {
    let path = Path::new(relative);
    latte_work_platform::paths::relative(path)?;
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        bail!("路径必须位于项目目录内");
    }
    let root = root.canonicalize()?;
    latte_work_platform::paths::check_project_links(&root, path)?;
    let target = root.join(path).canonicalize()?;
    if !target.starts_with(&root) {
        bail!("拒绝访问指向项目外部的路径");
    }
    Ok(target)
}
pub fn list_excluding(
    root: &Path,
    path: &str,
    exclusions: &ManagedRecoveryExclusions,
) -> Result<Vec<FileEntry>> {
    exclusions.require_visible(root, path)?;
    let folder = resolve(root, path)?;
    let mut entries = Vec::new();
    for entry in folder.read_dir()? {
        let entry = entry?;
        let name = entry
            .file_name()
            .to_str()
            .context("文件名不是 UTF-8")?
            .to_owned();
        if matches!(
            name.as_str(),
            ".git" | "node_modules" | "target" | ".DS_Store"
        ) {
            continue;
        }
        let relative = latte_work_platform::paths::relative_text(&Path::new(path).join(&name))?;
        if exclusions.contains(root, &relative)? {
            continue;
        }
        if entries.len() >= 1000 {
            break;
        }
        let Ok(target) = resolve(root, &relative) else {
            continue;
        };
        entries.push(FileEntry {
            name,
            path: relative,
            directory: target.is_dir(),
        });
    }
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}
#[cfg(test)]
fn list(root: &Path, path: &str) -> Result<Vec<FileEntry>> {
    list_excluding(root, path, &ManagedRecoveryExclusions::empty())
}
pub async fn read(root: &Path, path: &str) -> Result<(String, bool)> {
    let target = resolve(root, path)?;
    if !target.is_file() {
        bail!("不是普通文件");
    }
    let file = tokio::fs::File::open(target).await?;
    let mut bytes = Vec::new();
    file.take((LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .await?;
    let truncated = bytes.len() > LIMIT;
    bytes.truncate(LIMIT);
    if bytes.contains(&0) {
        bail!("首版仅预览文本文件");
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), truncated))
}
pub async fn diff_excluding(
    root: &Path,
    exclusions: &ManagedRecoveryExclusions,
) -> Result<(String, bool)> {
    let (entries, mut truncated) = changes_excluding(root, exclusions).await?;
    let status = entries
        .iter()
        .map(|entry| {
            format!(
                "{} {}\n",
                entry.status,
                crate::git_review::quote_path(&entry.path)
            )
        })
        .collect::<String>();
    let mut sections = Vec::new();
    for section in [ChangeSection::Unstaged, ChangeSection::Staged] {
        let paths: Vec<_> = entries
            .iter()
            .filter(|entry| entry.section == section)
            .flat_map(|entry| {
                std::iter::once(entry.path.as_str()).chain(entry.previous_path.as_deref())
            })
            .collect();
        if paths.is_empty() {
            sections.push(String::new());
            continue;
        }
        // Renames crossing the private boundary are deliberately omitted by changes.
        // Both validated endpoints preserve ordinary rename semantics in this patch.
        let mut args = vec!["diff", "--no-ext-diff", "--no-textconv", "--find-renames"];
        if section == ChangeSection::Staged {
            args.push("--cached");
        }
        args.push("--");
        args.extend(paths);
        let (text, partial) = git(root, &args).await?;
        truncated |= partial;
        sections.push(text);
    }
    let mut text = format!(
        "# 工作区状态\n{status}\n# 未暂存改动\n{}\n# 已暂存改动\n{}",
        sections[0], sections[1]
    );
    if text.len() > LIMIT {
        truncated = true;
        let mut end = LIMIT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    Ok((text, truncated))
}
#[cfg(all(test, unix))]
async fn changes(root: &Path) -> Result<(Vec<GitChange>, bool)> {
    changes_excluding(root, &ManagedRecoveryExclusions::empty()).await
}
#[cfg(all(test, unix))]
async fn diff(root: &Path) -> Result<(String, bool)> {
    diff_excluding(root, &ManagedRecoveryExclusions::empty()).await
}
#[cfg(all(test, unix))]
async fn change_diff(root: &Path, path: &str, section: ChangeSection) -> Result<(String, bool)> {
    change_diff_excluding(root, &ManagedRecoveryExclusions::empty(), path, section).await
}
/// Porcelain -z always reports repository-relative paths, including in subprojects.
/// Read one bounded snapshot, and keep project paths literal throughout Git calls.
pub async fn changes_excluding(
    root: &Path,
    exclusions: &ManagedRecoveryExclusions,
) -> Result<(Vec<GitChange>, bool)> {
    let (prefix, _) = git(root, &["rev-parse", "--show-prefix"]).await?;
    let prefix = prefix.strip_suffix('\n').unwrap_or(&prefix);
    let (status, mut truncated) = git(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
            ".",
        ],
    )
    .await?;
    let complete = status.rfind('\0').map(|end| &status[..=end]).unwrap_or("");
    let mut records = complete.split('\0');
    let mut entries = Vec::new();
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        if record.len() < 4 || record.as_bytes()[2] != b' ' {
            truncated = true;
            break;
        }
        let flags = &record.as_bytes()[..2];
        let renamed = flags.iter().any(|flag| matches!(flag, b'R' | b'C'));
        let previous = if renamed { records.next() } else { None };
        let Some(path) = record[3..].strip_prefix(prefix) else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        let previous_path = previous
            .and_then(|path| path.strip_prefix(prefix))
            .map(str::to_owned);
        if exclusions.contains(root, path)?
            || previous_path
                .as_deref()
                .map(|p| exclusions.contains(root, p))
                .transpose()?
                .unwrap_or(false)
        {
            continue;
        }
        for (flag, section) in if flags == b"??" {
            vec![(b'?', ChangeSection::Untracked)]
        } else {
            vec![
                (flags[0], ChangeSection::Staged),
                (flags[1], ChangeSection::Unstaged),
            ]
        } {
            if flag == b' ' || flag == b'!' {
                continue;
            }
            if entries.len() >= 1000 {
                truncated = true;
                break;
            }
            entries.push(GitChange {
                path: path.into(),
                previous_path: if matches!(flag, b'R' | b'C') {
                    previous_path.clone()
                } else {
                    None
                },
                section,
                status: char::from(flag).to_string(),
            });
        }
        if entries.len() >= 1000 {
            truncated |= records.any(|record| !record.is_empty());
            break;
        }
    }
    Ok((entries, truncated))
}
pub async fn change_diff_excluding(
    root: &Path,
    exclusions: &ManagedRecoveryExclusions,
    path: &str,
    section: ChangeSection,
) -> Result<(String, bool)> {
    exclusions.require_visible(root, path)?;
    if path.is_empty()
        || path.len() > 4096
        || path.contains('\0')
        || Path::new(path).is_absolute()
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("路径必须位于项目目录内");
    }
    let (entries, partial) = changes_excluding(root, exclusions).await?;
    let Some(entry) = entries
        .iter()
        .find(|entry| entry.path == path && entry.section == section)
    else {
        return Ok((String::new(), partial));
    };
    if section == ChangeSection::Untracked {
        let (text, truncated) = read(root, path).await?;
        // Plain contents, not a synthetic patch: binary and symlink checks stay in read().
        return Ok((text, truncated));
    }
    let mut args = vec![
        "diff",
        "--relative",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
    ];
    if section == ChangeSection::Staged {
        args.push("--cached");
    }
    let previous = entry.previous_path.as_deref();
    args.extend(["--", path]);
    if let Some(previous) = previous {
        args.push(previous);
    }
    git(root, &args).await
}

/// Index/worktree renames can be absent from one scoped diff. Keep both
/// endpoints vetoed when the combined porcelain record crosses private storage.
pub(crate) async fn private_rename_paths(
    root: &Path,
    exclusions: &ManagedRecoveryExclusions,
) -> Result<std::collections::HashSet<String>> {
    let (prefix, prefix_truncated) = git(root, &["rev-parse", "--show-prefix"]).await?;
    let (status, truncated) = git(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--",
            ".",
        ],
    )
    .await?;
    if truncated || prefix_truncated {
        bail!("Git rename boundary information exceeds the read limit");
    }
    let prefix = prefix.trim_end_matches('\n');
    let mut records = status.split('\0');
    let mut veto = std::collections::HashSet::new();
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        if record.len() < 4 || record.as_bytes()[2] != b' ' {
            bail!("Invalid Git rename boundary record");
        }
        if !record.as_bytes()[..2]
            .iter()
            .any(|flag| matches!(flag, b'R' | b'C'))
        {
            continue;
        }
        let previous = records
            .next()
            .context("Incomplete Git rename boundary record")?;
        let path = record[3..].strip_prefix(prefix);
        let previous = previous.strip_prefix(prefix);
        if path
            .map(|path| exclusions.contains(root, path))
            .transpose()?
            .unwrap_or(false)
            || previous
                .map(|path| exclusions.contains(root, path))
                .transpose()?
                .unwrap_or(false)
        {
            veto.extend(path.map(str::to_owned));
            veto.extend(previous.map(str::to_owned));
        }
    }
    Ok(veto)
}
pub(crate) async fn git(root: &Path, args: &[&str]) -> Result<(String, bool)> {
    let mut child = Command::new("git")
        .args([
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.quotePath=false",
        ])
        .args(args)
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut output = child
        .stdout
        .take()
        .context("git stdout missing")?
        .take((LIMIT + 1) as u64);
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), output.read_to_end(&mut bytes)).await??;
    let truncated = bytes.len() > LIMIT;
    if truncated {
        child.kill().await?;
    } else if !tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await??
        .success()
    {
        bail!("无法读取 Git 状态，请确认项目是 Git 仓库");
    }
    bytes.truncate(LIMIT);
    if truncated
        && let Err(error) = std::str::from_utf8(&bytes)
        && error.error_len().is_none()
    {
        bytes.truncate(error.valid_up_to());
    }
    let text = if args.iter().any(|arg| {
        matches!(
            *arg,
            "-z" | "--show-prefix" | "--show-toplevel" | "--absolute-git-dir"
        )
    }) {
        String::from_utf8(bytes).context("Git 文件路径不是 UTF-8")?
    } else {
        // Diff contents are display-only; metadata used as paths must remain exact.
        String::from_utf8_lossy(&bytes).into_owned()
    };
    Ok((text, truncated))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn native_references_use_protocol_relative_separators() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("子目录")).unwrap();
        let file = root.path().join("子目录").join("file.txt");
        std::fs::write(&file, "text").unwrap();
        let entry = reference(root.path(), file.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(entry.path, "子目录/file.txt");
        assert_eq!(read(root.path(), &entry.path).await.unwrap().0, "text");
        assert_eq!(
            list(root.path(), "子目录").unwrap()[0].path,
            "子目录/file.txt"
        );
    }
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_external_references_preserve_project_boundaries() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let folder = outside.path().join("中文 docs");
        std::fs::create_dir(&folder).unwrap();
        let file = folder.join("中文 file.txt");
        std::fs::write(&file, "reference only").unwrap();

        for (target, directory) in [(&file, false), (&folder, true)] {
            let canonical = target.canonicalize().unwrap();
            let extended = canonical.to_str().unwrap();
            let drive = extended.strip_prefix(r"\\?\").unwrap();
            for path in [drive, extended] {
                let entry = reference(root.path(), path).await.unwrap();
                assert_eq!(entry.path, extended);
                assert_eq!(entry.directory, directory);
                assert_eq!(entry.name, target.file_name().unwrap().to_str().unwrap());
                assert!(read(root.path(), &entry.path).await.is_err());
                assert!(list(root.path(), &entry.path).is_err());
            }
        }
        // An explicit reference must not make project-relative traversal valid.
        assert!(
            read(root.path(), "../中文 docs/中文 file.txt")
                .await
                .is_err()
        );
        assert!(list(root.path(), "..").is_err());
    }
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_external_references_reject_invalid_native_paths() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file.txt");
        std::fs::write(&file, "text").unwrap();
        let ads = format!("{}:stream", file.to_str().unwrap());
        let nul = format!("{}\0suffix", file.to_str().unwrap());
        let missing = root.path().join("missing.txt");
        for path in [
            "relative.txt",
            "../escape",
            r"C:relative.txt",
            r"\root-relative.txt",
            r"\\server\share\file.txt",
            r"\\?\UNC\server\share\file.txt",
            r"\\.\pipe\file",
            &ads,
            &nul,
            missing.to_str().unwrap(),
        ] {
            assert!(reference(root.path(), path).await.is_err(), "{path:?}");
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn unix_backslash_names_remain_literal() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("a\\b.txt");
        std::fs::write(&file, "literal").unwrap();
        let entry = reference(root.path(), file.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(entry.path, "a\\b.txt");
        assert_eq!(read(root.path(), &entry.path).await.unwrap().0, "literal");
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn explicit_references_resolve_without_expanding_project_access() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("中文 file.txt");
        std::fs::write(&file, "reference only").unwrap();
        let reference_file = reference(root.path(), file.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(
            reference_file.path,
            file.canonicalize().unwrap().to_str().unwrap()
        );
        assert!(!reference_file.directory);
        assert!(
            reference(root.path(), outside.path().to_str().unwrap())
                .await
                .unwrap()
                .directory
        );
        std::fs::write(root.path().join("inside"), "text").unwrap();
        assert_eq!(
            reference(root.path(), root.path().join("inside").to_str().unwrap())
                .await
                .unwrap()
                .path,
            "inside"
        );
        assert_eq!(
            reference(root.path(), root.path().to_str().unwrap())
                .await
                .unwrap()
                .path,
            "."
        );
        std::os::unix::fs::symlink(&file, root.path().join("link")).unwrap();
        assert_eq!(
            reference(root.path(), root.path().join("link").to_str().unwrap())
                .await
                .unwrap()
                .path,
            reference_file.path
        );
        assert!(resolve(root.path(), "link").is_err());
        assert!(read(root.path(), &reference_file.path).await.is_err());
        assert!(list(root.path(), outside.path().to_str().unwrap()).is_err());
        for invalid in [
            "relative",
            "../escape",
            "/missing/latte-reference",
            "/nul\0path",
        ] {
            assert!(reference(root.path(), invalid).await.is_err());
        }
        assert!(
            reference(root.path(), &format!("/{}", "x".repeat(4096)))
                .await
                .is_err()
        );
        let socket = outside.path().join("socket");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert!(
            reference(root.path(), socket.to_str().unwrap())
                .await
                .is_err()
        );
    }
    #[test]
    fn directory_browser_names_current_folder_and_root_natively() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("中文 project");
        std::fs::create_dir(&folder).unwrap();
        let root = folder.ancestors().last().unwrap();
        for (path, expected) in [
            (&folder, "中文 project"),
            (&root.to_owned(), root.to_str().unwrap()),
        ] {
            let response = browse_directories(path.to_str()).unwrap();
            let latte_work_protocol::Response::Directories { name, .. } = response else {
                panic!("expected directory listing");
            };
            // Canonical Windows roots use the verbatim prefix.
            let expected = if path == root {
                path.canonicalize().unwrap().to_str().unwrap().to_owned()
            } else {
                expected.to_owned()
            };
            assert_eq!(name.as_deref(), Some(expected.as_str()));
        }
    }
    #[cfg(unix)]
    #[test]
    fn directory_browser_preserves_unix_backslashes_in_names() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join(r"foo\bar");
        std::fs::create_dir(&folder).unwrap();
        let response = browse_directories(folder.to_str()).unwrap();
        assert!(
            matches!(response, latte_work_protocol::Response::Directories { name: Some(name), .. } if name == r"foo\bar")
        );
    }
    #[test]
    fn directory_picker_bounds_large_listings_and_rejects_files() {
        let root = tempfile::tempdir().unwrap();
        for i in 0..1001 {
            std::fs::create_dir(root.path().join(i.to_string())).unwrap();
        }
        let response = browse_directories(root.path().to_str()).unwrap();
        assert!(
            matches!(response, latte_work_protocol::Response::Directories { entries, truncated: true, .. } if entries.len() == 1000)
        );
        let file = root.path().join("file");
        std::fs::write(&file, "not a folder").unwrap();
        assert!(browse_directories(file.to_str()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn traversal_and_symlink_escape_are_denied() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        assert!(resolve(root.path(), "../x").is_err());
        assert!(resolve(root.path(), "/etc/passwd").is_err());
        assert!(resolve(root.path(), "escape").is_err());
        std::fs::write(root.path().join("ok"), "text").unwrap();
        assert!(resolve(root.path(), "ok").is_ok());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn changes_are_project_scoped_and_preserve_unusual_paths() {
        let root = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(root.path())
                .status()
                .unwrap();
            assert!(status.success());
        };
        run(&["init", "-q"]);
        run(&["config", "user.name", "Fixture"]);
        run(&["config", "user.email", "fixture@example.test"]);
        let sub = root.path().join("project");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("existing.txt"), "original\n").unwrap();
        std::fs::write(sub.join("old.txt"), "rename me\n").unwrap();
        std::fs::write(root.path().join("outside.txt"), "outside\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "initial"]);
        std::fs::write(sub.join("existing.txt"), "staged\n").unwrap();
        run(&["add", "project/existing.txt"]);
        std::fs::write(sub.join("existing.txt"), "working\n").unwrap();
        run(&["mv", "project/old.txt", "project/renamed.txt"]);
        let unusual = "中文 space\nfile.txt";
        std::fs::write(sub.join(unusual), "untracked\n").unwrap();
        std::fs::write(root.path().join("outside.txt"), "changed\n").unwrap();
        std::fs::write(root.path().join("outside-new.txt"), "new\n").unwrap();
        let (entries, truncated) = changes(&sub).await.unwrap();
        assert!(!truncated);
        assert_eq!(entries.len(), 4);
        assert!(entries.iter().all(|entry| !entry.path.contains("outside") && !entry.path.starts_with("project/")));
        assert!(
            entries
                .iter()
                .any(|entry| entry.path == unusual && entry.section == ChangeSection::Untracked)
        );
        assert!(entries.iter().any(|entry| entry.path == "renamed.txt"
            && entry.previous_path.as_deref() == Some("old.txt")));
        assert!(
            change_diff(&sub, "existing.txt", ChangeSection::Staged)
                .await
                .unwrap()
                .0
                .contains("+staged")
        );
        assert!(
            change_diff(&sub, "existing.txt", ChangeSection::Unstaged)
                .await
                .unwrap()
                .0
                .contains("+working")
        );
        assert_eq!(
            change_diff(&sub, unusual, ChangeSection::Untracked)
                .await
                .unwrap()
                .0,
            "untracked\n"
        );
        assert!(!diff(&sub).await.unwrap().0.contains("outside"));
        for path in ["../outside.txt", "/etc/passwd", ":(top)*", ""] {
            // Magic syntax is a literal filename, never a pathspec escape.
            let result = change_diff(&sub, path, ChangeSection::Unstaged).await;
            if path.starts_with(':') {
                assert_eq!(result.unwrap().0, "");
            } else {
                assert!(result.is_err());
            }
        }
        std::os::unix::fs::symlink(root.path().join("outside.txt"), sub.join("link")).unwrap();
        assert!(
            change_diff(&sub, "link", ChangeSection::Untracked)
                .await
                .is_err()
        );
        std::fs::write(sub.join("binary"), [0, 1, 2]).unwrap();
        assert!(
            change_diff(&sub, "binary", ChangeSection::Untracked)
                .await
                .is_err()
        );
        std::fs::write(sub.join("large"), vec![b'a'; LIMIT + 10]).unwrap();
        assert!(
            change_diff(&sub, "large", ChangeSection::Untracked)
                .await
                .unwrap()
                .1
        );
    }
    #[tokio::test]
    async fn change_diff_missing_from_bounded_listing_preserves_partial_state() {
        let root = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(root.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init", "-q"]);
        run(&["config", "core.autocrlf", "false"]);
        let paths: Vec<_> = (0..1002)
            .map(|index| format!("file-{index:04}.txt"))
            .collect();
        for path in &paths {
            std::fs::write(root.path().join(path), b"actual contents\n").unwrap();
        }
        let exclusions = ManagedRecoveryExclusions::empty();
        for section in [ChangeSection::Untracked, ChangeSection::Staged] {
            if section == ChangeSection::Staged {
                run(&["add", "."]);
            }
            let (entries, partial) = changes_excluding(root.path(), &exclusions).await.unwrap();
            assert!(partial);
            assert_eq!(entries.len(), 1000);
            let missing = paths
                .iter()
                .find(|path| !entries.iter().any(|entry| &entry.path == *path))
                .unwrap();
            assert!(root.path().join(missing).is_file());
            let result = change_diff_excluding(root.path(), &exclusions, missing, section)
                .await
                .unwrap();
            assert!(result.0.is_empty());
            assert!(
                result.1,
                "An omitted existing path must remain explicitly partial"
            );
        }
    }
}
