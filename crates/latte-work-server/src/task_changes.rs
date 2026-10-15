//! Bounded, durable pre-request worktree baselines. Read-only inspection never
//! changes the user's index, refs or files. Shared-directory attribution is temporal.
use crate::{files, store::now};
use anyhow::{Context, Result, bail};
use latte_work_protocol::{ChangeSummary, TaskChange};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::{collections::BTreeMap, path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};
const FILE_LIMIT: usize = 512 * 1024;
const TOTAL_LIMIT: usize = 8 * 1024 * 1024;
const COUNT_LIMIT: usize = 1000;
// Bound directory entries as well as files, including empty directory trees.
const INVENTORY_LIMIT: usize = 5000;
fn no_symlink_ancestors(root: &Path, relative: &Path) -> Result<()> {
    let mut path = root.to_path_buf();
    if path.symlink_metadata()?.file_type().is_symlink() {
        bail!("拒绝采集符号链接目录");
    }
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            bail!("路径必须位于项目目录内");
        }
        path.push(component);
        if path.symlink_metadata()?.file_type().is_symlink() {
            bail!("拒绝采集符号链接路径");
        }
    }
    Ok(())
}
fn if_present<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}
async fn inventory_directory(root: &Path, path: &Path) -> Result<Option<tokio::fs::ReadDir>> {
    if_present(
        async {
            no_symlink_ancestors(root, path.strip_prefix(root)?)?;
            Ok(tokio::fs::read_dir(path).await?)
        }
        .await,
    )
}
async fn inventory_entry_type(
    root: &Path,
    entry: &tokio::fs::DirEntry,
) -> Result<Option<std::fs::FileType>> {
    if_present(
        async {
            let kind = entry.file_type().await?;
            if !kind.is_symlink() {
                let path = entry.path();
                no_symlink_ancestors(root, path.strip_prefix(root)?)?;
                files::resolve(
                    root,
                    path.strip_prefix(root)?
                        .to_str()
                        .context("文件路径不是 UTF-8")?,
                )?;
            }
            Ok(kind)
        }
        .await,
    )
}
async fn inventory(root: &Path) -> Result<(Vec<String>, bool)> {
    let (inside, truncated) = match files::git(root, &["rev-parse", "--is-inside-work-tree"]).await
    {
        Ok(result) => result,
        Err(error) if error.to_string() == "无法读取 Git 状态，请确认项目是 Git 仓库" => {
            (String::new(), false)
        }
        Err(error) => return Err(error),
    };
    if inside.trim() == "true" && !truncated {
        let (prefix, a) = files::git(root, &["rev-parse", "--show-prefix"]).await?;
        let (paths, b) = files::git(
            root,
            &[
                "ls-files",
                "--full-name",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
                "--",
                ".",
            ],
        )
        .await?;
        return Ok((
            paths
                .split('\0')
                .filter(|path| !path.is_empty())
                .filter_map(|path| path.strip_prefix(prefix.trim_end_matches('\n')))
                .map(str::to_owned)
                .collect(),
            a || b,
        ));
    }
    let mut directories = vec![root.to_path_buf()];
    let mut paths = Vec::new();
    let mut visited = 0;
    let mut truncated = false;
    'walk: while let Some(directory) = directories.pop() {
        let Some(mut entries) = inventory_directory(root, &directory).await? else {
            continue;
        };
        while let Some(entry) =
            if_present(entries.next_entry().await.map_err(Into::into))?.flatten()
        {
            visited += 1;
            if visited > INVENTORY_LIMIT || paths.len() >= COUNT_LIMIT {
                truncated = true;
                break 'walk;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                truncated = true;
                continue;
            };
            if matches!(name, ".git" | "node_modules" | "target" | ".DS_Store") {
                continue;
            }
            let Some(kind) = inventory_entry_type(root, &entry).await? else {
                continue;
            };
            if kind.is_symlink() {
                truncated = true;
                continue;
            }
            let path = entry.path();
            if kind.is_dir() {
                directories.push(path);
            } else if kind.is_file() {
                paths.push(
                    path.strip_prefix(root)?
                        .to_str()
                        .context("文件路径不是 UTF-8")?
                        .to_owned(),
                );
            } else {
                truncated = true;
            }
        }
    }
    paths.sort();
    Ok((paths, truncated))
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub at: f64,
    #[serde(default)]
    root_identity: Option<(u64, u64)>,
    files: BTreeMap<String, Option<Vec<u8>>>,
    #[serde(default)]
    modes: BTreeMap<String, u32>,
    pub truncated: bool,
    pub unavailable: Option<String>,
}
impl Snapshot {
    pub fn unavailable(message: String) -> Self {
        Self {
            at: now(),
            root_identity: None,
            files: BTreeMap::new(),
            modes: BTreeMap::new(),
            truncated: false,
            unavailable: Some(message),
        }
    }
    pub async fn capture(root: &Path) -> Result<Self> {
        tokio::time::timeout(Duration::from_secs(12), async {
            // Git keeps its tracked/ignored-file semantics. Ordinary directories
            // use a bounded, symlink-free inventory with the same byte limits.
            let (paths, inventory_truncated) = inventory(root).await?;
            let root_metadata = std::fs::metadata(root)?;
            let mut snapshot = Self {
                at: now(),
                root_identity: Some((root_metadata.dev(), root_metadata.ino())),
                files: BTreeMap::new(),
                modes: BTreeMap::new(),
                truncated: inventory_truncated,
                unavailable: None,
            };
            let mut total = 0;
            for path in &paths {
                let path = path.as_str();
                if snapshot.files.len() >= COUNT_LIMIT {
                    snapshot.truncated = true;
                    break;
                }
                if snapshot.files.contains_key(path) {
                    continue;
                }
                let target = root.join(path);
                let metadata = match target.symlink_metadata() {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        snapshot.files.insert(path.into(), None);
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                };
                if metadata.file_type().is_symlink() {
                    snapshot.truncated = true;
                    continue;
                }
                let Some(target) = if_present(files::resolve(root, path))? else {
                    snapshot.files.insert(path.into(), None);
                    continue;
                };
                if !target.is_file() {
                    snapshot.truncated = true;
                    continue;
                }
                let mut bytes = Vec::new();
                let Some(file) =
                    if_present(tokio::fs::File::open(target).await.map_err(Into::into))?
                else {
                    snapshot.files.insert(path.into(), None);
                    continue;
                };
                file.take((FILE_LIMIT + 1) as u64)
                    .read_to_end(&mut bytes)
                    .await?;
                total += bytes.len();
                if total > TOTAL_LIMIT {
                    snapshot.truncated = true;
                    break;
                }
                if bytes.len() > FILE_LIMIT {
                    snapshot.truncated = true;
                    continue;
                }
                snapshot.modes.insert(path.into(), metadata.mode());
                snapshot.files.insert(path.into(), Some(bytes));
            }
            let root_metadata = std::fs::metadata(root)?;
            if snapshot.root_identity != Some((root_metadata.dev(), root_metadata.ino())) {
                bail!("项目目录在采集时已改变，请核对工作区");
            }
            Ok(snapshot)
        })
        .await
        .context("读取任务变更基线超时")?
    }
    pub async fn summary(&self, root: &Path) -> Result<ChangeSummary> {
        if self.unavailable.is_some() {
            return Ok(self.empty_summary());
        }
        let current = Self::capture(root).await?;
        self.compare(&current).await
    }
    pub async fn compare(&self, current: &Self) -> Result<ChangeSummary> {
        if self.root_identity.is_some()
            && current.root_identity.is_some()
            && self.root_identity != current.root_identity
        {
            bail!("项目目录已改变，无法确认本轮变更");
        }
        let mut result = self.empty_summary();
        result.truncated |= current.truncated;
        if self.unavailable.is_some() || current.unavailable.is_some() {
            result.unavailable = self
                .unavailable
                .clone()
                .or_else(|| current.unavailable.clone());
            return Ok(result);
        }
        let temp = tempfile::tempdir()?;
        let before = temp.path().join("before");
        let after = temp.path().join("after");
        std::fs::create_dir(&before)?;
        std::fs::create_dir(&after)?;
        let mut paths: Vec<_> = self
            .files
            .keys()
            .chain(current.files.keys())
            .cloned()
            .collect();
        paths.sort();
        paths.dedup();
        for path in paths {
            let old = self.files.get(&path).and_then(Option::as_ref);
            let new = current.files.get(&path).and_then(Option::as_ref);
            if old == new {
                continue;
            }
            // Missing from a bounded snapshot is unknown, never a fabricated deletion.
            if (old.is_some() && !current.files.contains_key(&path) && current.truncated)
                || (new.is_some() && !self.files.contains_key(&path) && self.truncated)
            {
                result.truncated = true;
                continue;
            }
            let index = result.entries.len();
            if let Some(bytes) = old {
                std::fs::write(before.join(index.to_string()), bytes)?;
            }
            if let Some(bytes) = new {
                std::fs::write(after.join(index.to_string()), bytes)?;
            }
            result.entries.push(TaskChange {
                path,
                status: if old.is_none() {
                    "A"
                } else if new.is_none() {
                    "D"
                } else {
                    "M"
                }
                .into(),
                added: None,
                removed: None,
            });
        }
        if result.entries.is_empty() {
            return Ok(result);
        }
        let (stats, truncated) = no_index(&before, &after, true).await?;
        result.truncated |= truncated;
        let mut records = stats.split('\0');
        while let Some(line) = records.next().filter(|line| !line.is_empty()) {
            let mut fields = line.splitn(3, '\t');
            let (Some(added), Some(removed), Some(path)) =
                (fields.next(), fields.next(), fields.next())
            else {
                result.truncated = true;
                break;
            };
            let path = if path.is_empty() {
                let (Some(old), Some(new)) = (records.next(), records.next()) else {
                    result.truncated = true;
                    break;
                };
                if new == "/dev/null" { old } else { new }
            } else {
                path
            };
            let Some(entry) = path
                .rsplit('/')
                .next()
                .and_then(|p| p.parse::<usize>().ok())
                .and_then(|i| result.entries.get_mut(i))
            else {
                result.truncated = true;
                continue;
            };
            entry.added = added.parse().ok();
            entry.removed = removed.parse().ok();
            match (entry.added, entry.removed) {
                (Some(a), Some(r)) => {
                    result.added += a;
                    result.removed += r;
                }
                _ => result.binary_files += 1,
            }
        }
        Ok(result)
    }
    fn empty_summary(&self) -> ChangeSummary {
        ChangeSummary {
            entries: vec![],
            added: 0,
            removed: 0,
            binary_files: 0,
            truncated: self.truncated,
            baseline_at: Some(self.at),
            unavailable: self.unavailable.clone(),
        }
    }
    pub async fn patch(&self, root: &Path, path: &str) -> Result<(String, bool)> {
        let current = Self::capture(root).await?;
        self.patch_between(&current, path).await
    }
    pub async fn patch_between(&self, current: &Self, path: &str) -> Result<(String, bool)> {
        validate_path(path)?;
        let summary = self.compare(current).await?;
        if let Some(error) = summary.unavailable {
            bail!("{error}");
        }
        if !summary.entries.iter().any(|entry| entry.path == path) {
            bail!("文件不属于本轮已记录的变更");
        }
        let temp = tempfile::tempdir()?;
        let before = temp.path().join("before");
        let after = temp.path().join("after");
        let old = self.files.get(path).and_then(Option::as_ref);
        let new = current.files.get(path).and_then(Option::as_ref);
        if let Some(bytes) = old {
            std::fs::write(&before, bytes)?;
        }
        if let Some(bytes) = new {
            std::fs::write(&after, bytes)?;
        }
        let old_path = if old.is_some() {
            before.as_path()
        } else {
            Path::new("/dev/null")
        };
        let new_path = if new.is_some() {
            after.as_path()
        } else {
            Path::new("/dev/null")
        };
        let (patch, truncated) = no_index(old_path, new_path, false).await?;
        Ok((
            patch
                .replace(&before.to_string_lossy().to_string(), &format!("/{path}"))
                .replace(&after.to_string_lossy().to_string(), &format!("/{path}")),
            truncated,
        ))
    }
}
fn validate_path(path: &str) -> Result<()> {
    if path.is_empty()
        || Path::new(path).is_absolute()
        || Path::new(path)
            .components()
            .any(|p| !matches!(p, std::path::Component::Normal(_)))
    {
        bail!("路径必须位于项目目录内");
    }
    Ok(())
}
/// Retain only changed bytes after completion; historical previews never read live files.
#[derive(Serialize, Deserialize)]
pub struct FrozenTurn {
    pub changes: latte_work_protocol::TurnChanges,
    before: Snapshot,
    after: Snapshot,
}
impl FrozenTurn {
    pub async fn freeze(
        request_id: String,
        mut before: Snapshot,
        mut after: Snapshot,
        interrupted: bool,
        background_pending: bool,
    ) -> Result<Self> {
        let summary = before.compare(&after).await?;
        before
            .files
            .retain(|path, _| summary.entries.iter().any(|e| &e.path == path));
        after
            .files
            .retain(|path, _| summary.entries.iter().any(|e| &e.path == path));
        before
            .modes
            .retain(|path, _| before.files.contains_key(path));
        after.modes.retain(|path, _| after.files.contains_key(path));
        Ok(Self {
            changes: latte_work_protocol::TurnChanges {
                request_id,
                summary,
                interrupted,
                background_pending,
                undo: latte_work_protocol::TurnUndoStatus::Ready,
            },
            before,
            after,
        })
    }
    pub fn validate_undo(&self, root: &Path) -> Result<()> {
        if self.changes.undo != latte_work_protocol::TurnUndoStatus::Ready
            || self.changes.summary.truncated
            || self.changes.summary.unavailable.is_some()
            || self.changes.background_pending
            || self.changes.summary.entries.is_empty()
        {
            bail!("此轮变更无法安全撤销，请查看工作区并手动核对");
        }
        let metadata = std::fs::metadata(root)?;
        if root.symlink_metadata()?.file_type().is_symlink()
            || self.before.root_identity != Some((metadata.dev(), metadata.ino()))
            || self.after.root_identity != self.before.root_identity
        {
            bail!("项目目录已改变，无法安全撤销");
        }
        for entry in &self.changes.summary.entries {
            if self
                .before
                .files
                .get(&entry.path)
                .and_then(Option::as_ref)
                .is_some()
                && !self.before.modes.contains_key(&entry.path)
            {
                bail!("缺少原文件权限，无法撤销");
            }
            self.verify_current(root, &entry.path)?;
        }
        Ok(())
    }
    fn verify_current(&self, root: &Path, path: &str) -> Result<()> {
        use std::io::Read;
        let metadata = std::fs::metadata(root)?;
        if self.before.root_identity != Some((metadata.dev(), metadata.ino()))
            || root.symlink_metadata()?.file_type().is_symlink()
        {
            bail!("项目目录已改变，无法安全撤销");
        }
        validate_path(path)?;
        let mut target = root.to_path_buf();
        let mut components = Path::new(path).components().peekable();
        while let Some(component) = components.next() {
            if component.as_os_str() == ".git" {
                bail!("不能撤销 Git 内部文件");
            }
            target.push(component);
            match target.symlink_metadata() {
                Ok(meta) if meta.file_type().is_symlink() => {
                    bail!("文件路径已变为符号链接：{path}")
                }
                Ok(meta) if components.peek().is_some() && !meta.is_dir() => {
                    bail!("文件所在目录已改变：{path}")
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && components.peek().is_none() => {}
                Err(error) => return Err(error.into()),
                _ => {}
            }
        }
        let expected = self.after.files.get(path).and_then(Option::as_ref);
        match (target.symlink_metadata(), expected) {
            (Err(e), None) if e.kind() == std::io::ErrorKind::NotFound => {}
            (Ok(meta), Some(bytes)) if meta.is_file() && meta.len() <= FILE_LIMIT as u64 => {
                let mut current = Vec::new();
                std::fs::File::open(&target)?
                    .take((FILE_LIMIT + 1) as u64)
                    .read_to_end(&mut current)?;
                if &current != bytes
                    || self
                        .after
                        .modes
                        .get(path)
                        .is_none_or(|mode| *mode != meta.mode())
                {
                    bail!("文件在本轮结束后已改变，未撤销：{path}");
                }
            }
            _ => bail!("文件在本轮结束后已改变，未撤销：{path}"),
        }
        Ok(())
    }
    pub fn restore(&self, root: &Path) -> Result<()> {
        use std::io::Write;
        // Called only after durable Unknown intent, with host admission paused and
        // overlapping idle native processes closed. External edits are checked again.
        for entry in &self.changes.summary.entries {
            self.verify_current(root, &entry.path)?;
            let target = root.join(&entry.path);
            if let Some(bytes) = self.before.files.get(&entry.path).and_then(Option::as_ref) {
                let mode = self
                    .before
                    .modes
                    .get(&entry.path)
                    .context("缺少原文件权限，无法撤销")?;
                let mut temporary =
                    tempfile::NamedTempFile::new_in(target.parent().context("文件目录缺失")?)?;
                temporary.write_all(bytes)?;
                temporary
                    .as_file()
                    .set_permissions(std::fs::Permissions::from_mode(*mode & 0o777))?;
                temporary.as_file().sync_all()?;
                self.verify_current(root, &entry.path)?;
                if self
                    .after
                    .files
                    .get(&entry.path)
                    .and_then(Option::as_ref)
                    .is_some()
                {
                    temporary.persist(&target)?;
                } else {
                    temporary.persist_noclobber(&target)?;
                }
            } else {
                std::fs::remove_file(&target)?;
            }
        }
        Ok(())
    }
    pub async fn patch(&self, path: &str) -> Result<(String, bool)> {
        // The summary was computed before compaction. Retained bytes are sufficient
        // to compare exactly these paths; original partial flags remain on the summary.
        validate_path(path)?;
        if !self.changes.summary.entries.iter().any(|e| e.path == path) {
            bail!("文件不属于本轮已记录的变更");
        }
        let mut before = self.before.clone();
        let mut after = self.after.clone();
        before.truncated = false;
        after.truncated = false;
        before.patch_between(&after, path).await
    }
}
async fn no_index(before: &Path, after: &Path, stats: bool) -> Result<(String, bool)> {
    let mut command = Command::new("git");
    command.args([
        "diff",
        "--no-index",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-renames",
    ]);
    if stats {
        command.args(["--numstat", "-z"]);
    }
    let mut child = command
        .arg("--")
        .arg(before)
        .arg(after)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut bytes = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        child
            .stdout
            .take()
            .context("git stdout missing")?
            .take((FILE_LIMIT + 1) as u64)
            .read_to_end(&mut bytes),
    )
    .await??;
    let truncated = bytes.len() > FILE_LIMIT;
    if truncated {
        child.kill().await?;
    } else {
        let status = tokio::time::timeout(Duration::from_secs(3), child.wait()).await??;
        if !matches!(status.code(), Some(0 | 1)) {
            bail!("无法比较任务文件");
        }
    }
    bytes.truncate(FILE_LIMIT);
    Ok((String::from_utf8_lossy(&bytes).into(), truncated))
}
pub async fn workspace_summary(root: &Path) -> Result<ChangeSummary> {
    let (entries, truncated) = files::changes(root).await?;
    let mut result = ChangeSummary {
        entries: vec![],
        added: 0,
        removed: 0,
        binary_files: 0,
        truncated,
        baseline_at: None,
        unavailable: None,
    };
    // Count the net HEAD-to-worktree diff once, including both staged and unstaged
    // edits. An unborn repository has no HEAD, so compare against its empty index.
    let stats = match files::git(
        root,
        &[
            "diff",
            "HEAD",
            "--numstat",
            "--no-ext-diff",
            "--no-textconv",
            "--",
            ".",
        ],
    )
    .await
    {
        Ok(stats) => vec![stats],
        Err(_) => {
            files::git(root, &["rev-parse", "--is-inside-work-tree"]).await?;
            vec![
                files::git(
                    root,
                    &[
                        "diff",
                        "--cached",
                        "--numstat",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--",
                        ".",
                    ],
                )
                .await?,
                files::git(
                    root,
                    &[
                        "diff",
                        "--numstat",
                        "--no-ext-diff",
                        "--no-textconv",
                        "--",
                        ".",
                    ],
                )
                .await?,
            ]
        }
    };
    for (stats, partial) in stats {
        result.truncated |= partial;
        for line in stats.lines() {
            let mut columns = line.splitn(3, '\t');
            match (
                columns.next().and_then(|v| v.parse::<u32>().ok()),
                columns.next().and_then(|v| v.parse::<u32>().ok()),
            ) {
                (Some(a), Some(r)) => {
                    result.added += a;
                    result.removed += r;
                }
                _ => result.binary_files += 1,
            }
        }
    }
    for (index, entry) in entries.into_iter().enumerate() {
        if index >= 128 {
            result.truncated = true;
            break;
        }
        if entry.section == latte_work_protocol::ChangeSection::Untracked {
            match files::read(root, &entry.path).await {
                Ok((text, partial)) => {
                    result.added += text.lines().count() as u32;
                    result.truncated |= partial;
                }
                Err(_) => result.binary_files += 1,
            }
        }
        if !result.entries.iter().any(|e| e.path == entry.path) {
            result.entries.push(TaskChange {
                path: entry.path,
                status: entry.status,
                added: None,
                removed: None,
            });
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn git(root: &Path, args: &[&str]) {
        assert!(
            std::process::Command::new("git")
                .current_dir(root)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    fn repository(root: &Path) {
        git(root, &["init", "-q"]);
        std::fs::write(root.join("dirty.txt"), "original\n").unwrap();
        std::fs::write(root.join("delete.txt"), "gone\n").unwrap();
        std::fs::write(root.join("binary.dat"), [0, 1, 2]).unwrap();
        git(root, &["add", "."]);
        git(
            root,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "commit",
                "-qm",
                "baseline",
            ],
        );
    }
    #[tokio::test]
    async fn ordinary_directory_freezes_real_edits_additions_and_deletions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/edit.txt"), "before\n").unwrap();
        std::fs::write(root.join("delete.txt"), "remove\n").unwrap();
        let before = Snapshot::capture(root).await.unwrap();
        assert!(before.summary(root).await.unwrap().entries.is_empty());
        std::fs::write(root.join("nested/edit.txt"), "after\nextra\n").unwrap();
        std::fs::remove_file(root.join("delete.txt")).unwrap();
        std::fs::write(root.join("new.txt"), "created\n").unwrap();
        let after = Snapshot::capture(root).await.unwrap();
        let turn = FrozenTurn::freeze("r1".into(), before, after, false, false)
            .await
            .unwrap();
        assert_eq!(turn.changes.summary.entries.len(), 3);
        assert_eq!(
            (turn.changes.summary.added, turn.changes.summary.removed),
            (3, 2)
        );
        assert!(!turn.changes.summary.truncated);
        assert!(turn.changes.summary.unavailable.is_none());
        let turn: FrozenTurn =
            serde_json::from_str(&serde_json::to_string(&turn).unwrap()).unwrap();
        let (patch, _) = turn.patch("nested/edit.txt").await.unwrap();
        assert!(patch.contains("-before") && patch.contains("+extra"));
        std::fs::write(root.join("nested/edit.txt"), "external\n").unwrap();
        assert_eq!(turn.patch("nested/edit.txt").await.unwrap().0, patch);
        assert!(turn.validate_undo(root).is_err());
        std::fs::write(root.join("nested/edit.txt"), "after\nextra\n").unwrap();
        turn.restore(root).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("nested/edit.txt")).unwrap(),
            "before\n"
        );
        assert!(root.join("delete.txt").exists());
        assert!(!root.join("new.txt").exists());
    }
    #[tokio::test]
    async fn ordinary_inventory_is_bounded_and_does_not_follow_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        std::fs::write(external.path().join("private.txt"), "private").unwrap();
        std::os::unix::fs::symlink(external.path(), dir.path().join("outside")).unwrap();
        std::fs::create_dir(dir.path().join("node_modules")).unwrap();
        std::fs::write(dir.path().join("node_modules/skip.txt"), "skip").unwrap();
        std::fs::write(dir.path().join("large.txt"), vec![0; FILE_LIMIT + 1]).unwrap();
        let before = Snapshot::capture(dir.path()).await.unwrap();
        assert!(before.truncated);
        assert!(before.files.is_empty());
        std::fs::remove_file(dir.path().join("large.txt")).unwrap();
        assert!(before.summary(dir.path()).await.unwrap().entries.is_empty());
        for n in 0..=COUNT_LIMIT {
            std::fs::write(dir.path().join(format!("file-{n}.txt")), "bounded").unwrap();
        }
        let (paths, truncated) = inventory(dir.path()).await.unwrap();
        assert!(truncated && paths.len() == COUNT_LIMIT);
        assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));
        let snapshot = Snapshot::capture(dir.path()).await.unwrap();
        assert!(snapshot.truncated);
        assert!(snapshot.files.len() <= COUNT_LIMIT);
        assert!(
            snapshot
                .files
                .keys()
                .all(|p| !p.contains("private") && !p.contains("skip"))
        );
    }
    #[tokio::test]
    async fn ordinary_inventory_skips_entries_and_queued_directories_deleted_after_enumeration() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("gone.txt"), "gone").unwrap();
        let mut entries = tokio::fs::read_dir(dir.path()).await.unwrap();
        let entry = entries.next_entry().await.unwrap().unwrap();
        std::fs::remove_file(entry.path()).unwrap();
        assert!(
            inventory_entry_type(dir.path(), &entry)
                .await
                .unwrap()
                .is_none()
        );
        let queued = dir.path().join("queued");
        std::fs::create_dir(&queued).unwrap();
        std::fs::remove_dir(&queued).unwrap();
        assert!(
            inventory_directory(dir.path(), &queued)
                .await
                .unwrap()
                .is_none()
        );
        std::fs::write(dir.path().join("kept.txt"), "kept").unwrap();
        assert_eq!(
            inventory(dir.path()).await.unwrap(),
            (vec!["kept.txt".into()], false)
        );
        // Disappearance is the only ignored error; permission and path-boundary
        // failures must never be turned into a complete, empty inventory.
        assert!(
            if_present::<()>(Err(std::io::Error::from(
                std::io::ErrorKind::PermissionDenied
            )
            .into()))
            .is_err()
        );
    }
    #[tokio::test]
    async fn task_counts_exclude_existing_dirty_files_survive_serialization_and_do_not_modify_index()
     {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        repository(root);
        std::fs::write(root.join("dirty.txt"), "existing dirty\n").unwrap();
        std::fs::write(root.join("existing.txt"), "preexisting untracked\n").unwrap();
        let baseline = Snapshot::capture(root).await.unwrap();
        assert!(baseline.summary(root).await.unwrap().entries.is_empty());
        let baseline: Snapshot =
            serde_json::from_str(&serde_json::to_string(&baseline).unwrap()).unwrap();
        std::fs::write(root.join("dirty.txt"), "task edit\nsecond line\n").unwrap();
        std::fs::remove_file(root.join("delete.txt")).unwrap();
        std::fs::write(root.join("new.txt"), "new\n").unwrap();
        std::fs::write(root.join("empty.txt"), "").unwrap();
        std::fs::write(root.join("binary.dat"), [0, 4, 2]).unwrap();
        git(root, &["add", "dirty.txt"]);
        let index = std::fs::read(root.join(".git/index")).unwrap();
        let result = baseline.summary(root).await.unwrap();
        assert_eq!(
            (result.added, result.removed, result.binary_files),
            (3, 2, 1)
        );
        assert_eq!(result.entries.len(), 5, "{result:?}");
        assert!(!result.truncated, "{result:?}");
        assert!(!result.entries.iter().any(|e| e.path == "existing.txt"));
        let empty = result
            .entries
            .iter()
            .find(|e| e.path == "empty.txt")
            .unwrap();
        assert_eq!((empty.added, empty.removed), (Some(0), Some(0)));
        let (patch, _) = baseline.patch(root, "dirty.txt").await.unwrap();
        assert!(
            patch.contains("-existing dirty") && patch.contains("+task edit"),
            "{patch}"
        );
        assert!(patch.contains("--- a/dirty.txt") && patch.contains("+++ b/dirty.txt"));
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        assert!(baseline.patch(root, "../private").await.is_err());
        assert!(baseline.patch(root, "existing.txt").await.is_err());
    }
    #[tokio::test]
    async fn workspace_totals_count_net_edits_once_across_staging() {
        let dir = tempfile::tempdir().unwrap();
        repository(dir.path());
        std::fs::write(dir.path().join("dirty.txt"), "staged\n").unwrap();
        git(dir.path(), &["add", "dirty.txt"]);
        std::fs::write(dir.path().join("dirty.txt"), "final\n").unwrap();
        let result = workspace_summary(dir.path()).await.unwrap();
        assert_eq!((result.added, result.removed), (1, 1));
    }
    #[tokio::test]
    async fn bounded_snapshots_never_report_an_oversized_file_as_deleted_and_respect_subprojects() {
        let dir = tempfile::tempdir().unwrap();
        repository(dir.path());
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("a.txt"), "old\n").unwrap();
        let baseline = Snapshot::capture(&sub).await.unwrap();
        std::fs::write(dir.path().join("dirty.txt"), "outside\n").unwrap();
        assert!(baseline.summary(&sub).await.unwrap().entries.is_empty());
        std::fs::write(sub.join("a.txt"), vec![b'a'; FILE_LIMIT + 1]).unwrap();
        let result = baseline.summary(&sub).await.unwrap();
        assert!(result.truncated && result.entries.is_empty());
        assert!(
            Snapshot::capture(Path::new("/missing-fixture-root"))
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn frozen_turns_survive_later_edits_and_restore_only_the_latest_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        repository(root);
        std::fs::write(root.join("dirty.txt"), "existing dirty\n").unwrap();
        let before = Snapshot::capture(root).await.unwrap();
        std::fs::write(root.join("dirty.txt"), "turn one\nsecond line\n").unwrap();
        let after = Snapshot::capture(root).await.unwrap();
        let one = FrozenTurn::freeze("r1".into(), before, after, false, false)
            .await
            .unwrap();
        let patch_one = one.patch("dirty.txt").await.unwrap();
        let before = Snapshot::capture(root).await.unwrap();
        std::fs::write(root.join("dirty.txt"), "turn two\n").unwrap();
        std::fs::remove_file(root.join("delete.txt")).unwrap();
        std::fs::write(root.join("new.txt"), "new\n").unwrap();
        std::fs::write(root.join("binary.dat"), [0, 7, 2]).unwrap();
        let after = Snapshot::capture(root).await.unwrap();
        let two = FrozenTurn::freeze("r2".into(), before, after, false, false)
            .await
            .unwrap();
        assert_eq!(one.patch("dirty.txt").await.unwrap(), patch_one);
        assert!(one.validate_undo(root).is_err());
        let index = std::fs::read(root.join(".git/index")).unwrap();
        let two: FrozenTurn = serde_json::from_str(&serde_json::to_string(&two).unwrap()).unwrap();
        assert_eq!(
            (two.changes.summary.added, two.changes.summary.removed),
            (2, 3)
        );
        assert!(
            two.patch("dirty.txt")
                .await
                .unwrap()
                .0
                .contains("-turn one")
        );
        assert!(
            two.patch("delete.txt")
                .await
                .unwrap()
                .0
                .contains("+++ /dev/null")
        );
        two.validate_undo(root).unwrap();
        std::fs::write(root.join("dirty.txt"), "external writer\n").unwrap();
        assert!(two.validate_undo(root).is_err());
        assert!(
            root.join("new.txt").exists(),
            "preflight has no side effects"
        );
        std::fs::write(root.join("dirty.txt"), "turn two\n").unwrap();
        two.validate_undo(root).unwrap();
        two.restore(root).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("dirty.txt")).unwrap(),
            "turn one\nsecond line\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("delete.txt")).unwrap(),
            "gone\n"
        );
        assert_eq!(std::fs::read(root.join("binary.dat")).unwrap(), [0, 1, 2]);
        assert!(!root.join("new.txt").exists());
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        assert!(two.validate_undo(root).is_err());
        assert_eq!(one.patch("dirty.txt").await.unwrap(), patch_one);
    }
    #[tokio::test]
    async fn undo_refuses_symlink_ancestors_and_partial_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        repository(root);
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/a.txt"), "before\n").unwrap();
        let before = Snapshot::capture(root).await.unwrap();
        std::fs::write(root.join("sub/a.txt"), "after\n").unwrap();
        let after = Snapshot::capture(root).await.unwrap();
        let mut turn = FrozenTurn::freeze("r".into(), before, after, false, false)
            .await
            .unwrap();
        let external = tempfile::tempdir().unwrap();
        std::fs::write(external.path().join("a.txt"), "after\n").unwrap();
        std::fs::rename(root.join("sub"), root.join("saved")).unwrap();
        std::os::unix::fs::symlink(external.path(), root.join("sub")).unwrap();
        assert!(turn.validate_undo(root).is_err());
        assert_eq!(
            std::fs::read_to_string(external.path().join("a.txt")).unwrap(),
            "after\n"
        );
        std::fs::remove_file(root.join("sub")).unwrap();
        std::fs::rename(root.join("saved"), root.join("sub")).unwrap();
        turn.validate_undo(root).unwrap();
        turn.changes.summary.truncated = true;
        assert!(turn.validate_undo(root).is_err());
        turn.changes.summary.truncated = false;
        turn.changes.background_pending = true;
        assert!(turn.validate_undo(root).is_err());
    }
    #[tokio::test]
    async fn undo_refuses_replaced_project_roots_even_with_matching_file_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir(&root).unwrap();
        repository(&root);
        let before = Snapshot::capture(&root).await.unwrap();
        std::fs::write(root.join("dirty.txt"), "after\n").unwrap();
        let after = Snapshot::capture(&root).await.unwrap();
        let turn = FrozenTurn::freeze("r".into(), before, after, false, false)
            .await
            .unwrap();
        let external = dir.path().join("replacement");
        std::fs::create_dir(&external).unwrap();
        std::fs::write(external.join("dirty.txt"), "after\n").unwrap();
        std::fs::rename(&root, dir.path().join("original-project")).unwrap();
        std::os::unix::fs::symlink(&external, &root).unwrap();
        assert!(turn.validate_undo(&root).is_err());
        assert!(turn.restore(&root).is_err());
        assert_eq!(
            std::fs::read_to_string(external.join("dirty.txt")).unwrap(),
            "after\n"
        );
    }
}
