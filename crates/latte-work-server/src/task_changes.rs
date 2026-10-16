//! Bounded, durable pre-request worktree baselines. Read-only inspection never
//! changes the user's index, refs or files. Shared-directory attribution is temporal.
use crate::{files, managed_recovery::ManagedRecoveryExclusions, store::now};
use anyhow::{Context, Result, bail};
use latte_work_protocol::{ChangeSummary, TaskChange};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
#[path = "task_changes/unix.rs"]
mod backend;
#[cfg(windows)]
#[path = "task_changes/windows.rs"]
mod backend;

pub const UNDO_SUPPORTED: bool = backend::UNDO_SUPPORTED;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
enum Identity {
    Unix((u64, u64)),
    Windows { windows_v1: WindowsIdentity },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WindowsIdentity {
    pub(crate) volume: u64,
    pub(crate) file_id: [u8; 16],
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WindowsFile {
    identity: WindowsIdentity,
    security: String,
    attributes: u32,
}
struct CapturedFile {
    bytes: Vec<u8>,
    mode: Option<u32>,
    windows: Option<WindowsFile>,
    undo_unavailable: Option<String>,
}
use std::{collections::BTreeMap, path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};
const FILE_LIMIT: usize = 512 * 1024;
const TOTAL_LIMIT: usize = 8 * 1024 * 1024;
const COUNT_LIMIT: usize = 1000;
// Bound directory entries as well as files, including empty directory trees.
const INVENTORY_LIMIT: usize = 5000;
fn no_symlink_ancestors(root: &Path, relative: &Path) -> Result<()> {
    let mut path = root.to_path_buf();
    if latte_work_platform::fs::is_link(&path.symlink_metadata()?) {
        bail!("拒绝采集符号链接目录");
    }
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            bail!("路径必须位于项目目录内");
        }
        path.push(component);
        if latte_work_platform::fs::is_link(&path.symlink_metadata()?) {
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
async fn inventory(
    root: &Path,
    exclusions: &ManagedRecoveryExclusions,
) -> Result<(Vec<String>, bool)> {
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
        let mut visible = Vec::new();
        for path in paths
            .split('\0')
            .filter(|path| !path.is_empty())
            .filter_map(|path| path.strip_prefix(prefix.trim_end_matches('\n')))
        {
            if !exclusions.contains(root, path)? {
                visible.push(path.to_owned());
            }
        }
        return Ok((visible, a || b));
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
            let relative =
                latte_work_platform::paths::relative_text(entry.path().strip_prefix(root)?)?;
            if exclusions.contains(root, &relative)? {
                continue;
            }
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
                paths.push(relative);
            } else {
                truncated = true;
            }
        }
    }
    paths.sort();
    Ok((paths, truncated))
}
#[cfg(windows)]
const WINDOWS_NON_GIT_UNDO: &str =
    "Windows 普通目录支持变更快照和差异；撤销需要同卷 NTFS Git 恢复目录";

#[cfg(windows)]
pub(crate) async fn windows_undo_git_dir(root: &Path) -> Result<Option<std::path::PathBuf>> {
    let (git_dir, truncated) = match files::git(root, &["rev-parse", "--absolute-git-dir"]).await {
        Ok(result) => result,
        Err(error) if error.to_string() == "无法读取 Git 状态，请确认项目是 Git 仓库" =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if truncated {
        bail!("Git 目录路径超出限制");
    }
    let git_dir = git_dir.strip_suffix('\n').unwrap_or(&git_dir);
    let git_dir = git_dir.strip_suffix('\r').unwrap_or(git_dir);
    Ok(Some(std::path::PathBuf::from(git_dir)))
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub at: f64,
    #[serde(default)]
    root_identity: Option<Identity>,
    files: BTreeMap<String, Option<Vec<u8>>>,
    #[serde(default)]
    modes: BTreeMap<String, u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    windows_files_v1: BTreeMap<String, WindowsFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    undo_unavailable: Option<String>,
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
            windows_files_v1: BTreeMap::new(),
            undo_unavailable: None,
            truncated: false,
            unavailable: Some(message),
        }
    }
    #[cfg(test)]
    pub async fn capture(root: &Path) -> Result<Self> {
        Self::capture_excluding(root, &ManagedRecoveryExclusions::empty()).await
    }
    pub async fn capture_excluding(
        root: &Path,
        exclusions: &ManagedRecoveryExclusions,
    ) -> Result<Self> {
        tokio::time::timeout(Duration::from_secs(12), async {
            // Git keeps its tracked/ignored-file semantics. Ordinary directories
            // use a bounded, symlink-free inventory with the same byte limits.
            let (paths, inventory_truncated) = inventory(root, exclusions).await?;
            let mut snapshot = Self {
                at: now(),
                root_identity: Some(backend::identity(root)?),
                files: BTreeMap::new(),
                modes: BTreeMap::new(),
                windows_files_v1: BTreeMap::new(),
                undo_unavailable: None,
                truncated: inventory_truncated,
                unavailable: None,
            };
            let mut total = 0;
            let mut metadata_total = 0;
            for path in &paths {
                let path = path.as_str();
                if snapshot.files.len() >= COUNT_LIMIT {
                    snapshot.truncated = true;
                    break;
                }
                if snapshot.files.contains_key(path) {
                    continue;
                }
                validate_path(path)?;
                let target = root.join(path);
                let metadata = match target.symlink_metadata() {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        snapshot.files.insert(path.into(), None);
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                };
                if latte_work_platform::fs::is_link(&metadata) {
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
                let capture_root = root.to_path_buf();
                let capture_path = path.to_owned();
                let Some(captured) = if_present(
                    tokio::task::spawn_blocking(move || {
                        backend::capture_file(&capture_root, &capture_path)
                    })
                    .await?,
                )?
                else {
                    snapshot.files.insert(path.into(), None);
                    continue;
                };
                let bytes = captured.bytes;
                if let Some(reason) = captured.undo_unavailable {
                    snapshot.undo_unavailable.get_or_insert(reason);
                }
                total += bytes.len();
                if total > TOTAL_LIMIT {
                    snapshot.truncated = true;
                    break;
                }
                if bytes.len() > FILE_LIMIT {
                    snapshot.truncated = true;
                    continue;
                }
                if let Some(mode) = captured.mode {
                    snapshot.modes.insert(path.into(), mode);
                }
                if let Some(metadata) = captured.windows {
                    if metadata_total + metadata.security.len() > TOTAL_LIMIT {
                        snapshot
                            .undo_unavailable
                            .get_or_insert("Windows 文件权限元数据超出限制".into());
                    } else {
                        metadata_total += metadata.security.len();
                        snapshot.windows_files_v1.insert(path.into(), metadata);
                    }
                }
                snapshot.files.insert(path.into(), Some(bytes));
            }
            if snapshot.root_identity != Some(backend::identity(root)?) {
                bail!("项目目录在采集时已改变，请核对工作区");
            }
            Ok(snapshot)
        })
        .await
        .context("读取任务变更基线超时")?
    }
    pub async fn summary(
        &self,
        root: &Path,
        exclusions: &ManagedRecoveryExclusions,
    ) -> Result<ChangeSummary> {
        if self.unavailable.is_some() {
            return Ok(self.empty_summary());
        }
        let current = Self::capture_excluding(root, exclusions).await?;
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
    pub async fn patch(
        &self,
        root: &Path,
        path: &str,
        exclusions: &ManagedRecoveryExclusions,
    ) -> Result<(String, bool)> {
        let current = Self::capture_excluding(root, exclusions).await?;
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
            patch_labels(&patch, path, old.is_some(), new.is_some()),
            truncated,
        ))
    }
}
fn patch_labels(patch: &str, path: &str, old: bool, new: bool) -> String {
    fn label(prefix: &str, path: &str) -> String {
        let value = format!("{prefix}/{path}");
        if value
            .bytes()
            .any(|b| !b.is_ascii_graphic() || matches!(b, b'"' | b'\\'))
        {
            crate::git_review::quote_path(&value)
        } else {
            value
        }
    }
    let a = label("a", path);
    let b = label("b", path);
    let old_label = if old { a.as_str() } else { "/dev/null" };
    let new_label = if new { b.as_str() } else { "/dev/null" };
    let mut in_hunk = false;
    patch
        .split_inclusive('\n')
        .map(|line| {
            if line.starts_with("@@") {
                in_hunk = true;
            }
            if !in_hunk {
                if line.starts_with("diff --git ") {
                    return format!("diff --git {a} {b}\n");
                }
                if line.starts_with("--- ") {
                    return format!("--- {old_label}\n");
                }
                if line.starts_with("+++ ") {
                    return format!("+++ {new_label}\n");
                }
                if line.starts_with("Binary files ") {
                    return format!("Binary files {old_label} and {new_label} differ\n");
                }
            }
            line.to_owned()
        })
        .collect()
}

fn validate_path(path: &str) -> Result<()> {
    latte_work_platform::paths::relative(Path::new(path))?;
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
#[derive(Clone, Serialize, Deserialize)]
pub struct FrozenTurn {
    pub changes: latte_work_protocol::TurnChanges,
    before: Snapshot,
    after: Snapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    windows_undo_recovery_v1: Option<UndoRecovery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    windows_undo_recovery_v2: Option<ManagedUndoRecovery>,
}
#[derive(Clone, Serialize, Deserialize)]
struct UndoRecovery {
    attempt_id: String,
    project_root: std::path::PathBuf,
    project_identity: WindowsIdentity,
    git_dir: std::path::PathBuf,
    git_identity: WindowsIdentity,
    recovery_dir: std::path::PathBuf,
    recovery_identity: WindowsIdentity,
    entries: Vec<UndoRecoveryEntry>,
}
#[derive(Clone, Serialize, Deserialize)]
struct ManagedUndoRecovery {
    kind: String,
    version: u32,
    attempt_id: String,
    store: crate::managed_recovery::Registration,
    recovery_dir: std::path::PathBuf,
    recovery_identity: WindowsIdentity,
    entries: Vec<UndoRecoveryEntry>,
}
#[derive(Clone, Serialize, Deserialize)]
struct UndoRecoveryEntry {
    path: String,
    source_identity: Option<WindowsIdentity>,
    recovery_name: Option<String>,
    operation: String,
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
        let undo_unavailable = backend::undo_reason(&before, &after);
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
        before
            .windows_files_v1
            .retain(|path, _| before.files.contains_key(path));
        after
            .windows_files_v1
            .retain(|path, _| after.files.contains_key(path));
        Ok(Self {
            changes: latte_work_protocol::TurnChanges {
                request_id,
                summary,
                interrupted,
                background_pending,
                undo: latte_work_protocol::TurnUndoStatus::Ready,
                undo_unavailable,
            },
            before,
            after,
            windows_undo_recovery_v1: None,
            windows_undo_recovery_v2: None,
        })
    }
    pub fn validate_undo(&self, root: &Path) -> Result<()> {
        backend::require_undo()?;
        if self.windows_undo_recovery_v1.is_some() && self.windows_undo_recovery_v2.is_some() {
            bail!("Conflicting Windows Undo recovery intents");
        }
        if let Some(reason) = self
            .changes
            .undo_unavailable
            .clone()
            .or_else(|| backend::undo_reason(&self.before, &self.after))
        {
            bail!("{reason}");
        }
        if self.changes.undo != latte_work_protocol::TurnUndoStatus::Ready
            || self.changes.summary.truncated
            || self.changes.summary.unavailable.is_some()
            || self.changes.background_pending
            || self.changes.summary.entries.is_empty()
            || self.changes.summary.entries.len() > COUNT_LIMIT
        {
            bail!("此轮变更无法安全撤销，请查看工作区并手动核对");
        }
        if latte_work_platform::fs::is_link(&root.symlink_metadata()?)
            || self.before.root_identity != Some(backend::identity(root)?)
            || self.after.root_identity != self.before.root_identity
        {
            bail!("项目目录已改变，无法安全撤销");
        }
        for entry in &self.changes.summary.entries {
            backend::validate_before(&self.before, &entry.path)?;
            self.verify_current(root, &entry.path)?;
        }
        Ok(())
    }
    fn verify_current(&self, root: &Path, path: &str) -> Result<()> {
        if self.before.root_identity != Some(backend::identity(root)?)
            || latte_work_platform::fs::is_link(&root.symlink_metadata()?)
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
                Ok(meta) if latte_work_platform::fs::is_link(&meta) => {
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
        backend::verify_current(self, root, path)
    }
    pub fn refresh_undo_availability(&mut self) {
        if self.windows_undo_recovery_v1.is_some() && self.windows_undo_recovery_v2.is_some() {
            self.changes.undo_unavailable =
                Some("Conflicting Windows Undo recovery intents".into());
            return;
        }
        #[cfg(windows)]
        if self.changes.undo_unavailable.as_deref() == Some(WINDOWS_NON_GIT_UNDO) {
            self.changes.undo_unavailable =
                Some("此旧版普通目录快照未完整评估 Windows 撤销条件，无法验证；请手动核对".into());
        }
        if self.changes.undo_unavailable.is_none() {
            self.changes.undo_unavailable = backend::undo_reason(&self.before, &self.after);
        }
    }
    /// The caller transfers admission to its blocking worker before invoking this.
    pub fn prepare_undo_blocking(
        &self,
        root: &Path,
        database: &crate::runtime::Database,
        #[cfg(windows)] git_dir: Option<&Path>,
    ) -> Result<PreparedUndo> {
        let exclusions = ManagedRecoveryExclusions::load(database, root)?;
        for entry in &self.changes.summary.entries {
            exclusions.require_visible(root, &entry.path)?;
        }
        self.validate_undo(root)?;
        #[cfg(windows)]
        let files = backend::prepare_registered(self, root, git_dir, database)?;
        #[cfg(unix)]
        let files = {
            let _ = database;
            backend::prepare(self, root)?
        };
        Ok(PreparedUndo {
            turn: self.clone(),
            root: root.to_owned(),
            files,
            exclusions,
        })
    }
    #[cfg(test)]
    pub async fn prepare_undo(&self, root: &Path) -> Result<PreparedUndo> {
        #[cfg(windows)]
        let git_dir = windows_undo_git_dir(root).await?.context(
            "Test requires Git recovery; use registered preparation for ordinary projects",
        )?;
        self.validate_undo(root)?;
        #[cfg(windows)]
        let files = backend::prepare(self, root, &git_dir)?;
        #[cfg(unix)]
        let files = backend::prepare(self, root)?;
        Ok(PreparedUndo {
            turn: self.clone(),
            root: root.to_owned(),
            files,
            exclusions: ManagedRecoveryExclusions::empty(),
        })
    }
    #[cfg(test)]
    pub async fn restore(&self, root: &Path) -> Result<()> {
        self.prepare_undo(root).await?.restore().map(drop)
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
fn restore_checkpoint(_completed: usize) -> Result<()> {
    #[cfg(feature = "test-fixture")]
    if _completed == 1
        && let Some(path) = std::env::var_os("LATTE_WORK_TEST_UNDO_BARRIER")
    {
        let path = std::path::PathBuf::from(path);
        std::fs::write(path.join("ready"), b"first file published")?;
        if path.join("fail").exists() {
            bail!("fixture: restoration failed after first file");
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !path.join("release").exists() {
            if std::time::Instant::now() >= deadline {
                bail!("fixture: restoration barrier timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(())
}
#[cfg(windows)]
pub(crate) fn undo_phase_checkpoint(_phase: &str) -> Result<()> {
    #[cfg(feature = "test-fixture")]
    if let Some(path) = std::env::var_os(format!(
        "LATTE_WORK_TEST_UNDO_{}",
        _phase.to_ascii_uppercase()
    )) {
        let path = std::path::PathBuf::from(path);
        std::fs::write(path.join("ready"), _phase)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !path.join("release").exists() {
            if std::time::Instant::now() >= deadline {
                bail!("fixture: {_phase} barrier timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    Ok(())
}
pub struct PreparedUndo {
    turn: FrozenTurn,
    root: std::path::PathBuf,
    files: backend::Prepared,
    exclusions: ManagedRecoveryExclusions,
}
impl PreparedUndo {
    pub fn record_intent(&self, turn: &mut FrozenTurn) {
        #[cfg(windows)]
        {
            if let Some(store) = &self.files.managed {
                let intent = &self.files.intent;
                turn.windows_undo_recovery_v2 = Some(ManagedUndoRecovery {
                    kind: "managed_project".into(),
                    version: 2,
                    attempt_id: intent.attempt_id.clone(),
                    store: store.clone(),
                    recovery_dir: intent.recovery_dir.clone(),
                    recovery_identity: intent.recovery_identity.clone(),
                    entries: intent.entries.clone(),
                });
            } else {
                turn.windows_undo_recovery_v1 = Some(self.files.intent.clone());
            }
        }
        #[cfg(unix)]
        let _ = turn;
    }
    /// Caller persists Unknown before applying; preparation only created staging files.
    pub fn restore(self) -> Result<RestoredUndo> {
        Ok(RestoredUndo {
            _guards: backend::restore(self.files, &self.turn, &self.root)?,
            _exclusions: self.exclusions,
        })
    }
}
/// Owned file/directory guards are released only after the result transaction commits.
pub struct RestoredUndo {
    _guards: backend::Restored,
    _exclusions: ManagedRecoveryExclusions,
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
    let empty_config = tempfile::NamedTempFile::new()?;
    let mut child = command
        .arg("--")
        .arg(before)
        .arg(after)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", empty_config.path())
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
pub async fn workspace_summary(
    root: &Path,
    exclusions: &ManagedRecoveryExclusions,
) -> Result<ChangeSummary> {
    let (entries, truncated) = files::changes_excluding(root, exclusions).await?;
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
    #[test]
    fn snapshot_identity_preserves_legacy_unix_and_full_windows_ids() {
        let legacy = serde_json::json!({"at":0.0,"root_identity":[1,2],"files":{},"modes":{},"truncated":false,"unavailable":null});
        let snapshot: Snapshot = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(serde_json::to_value(snapshot).unwrap(), legacy);
        let native = serde_json::json!({"at":0.0,"root_identity":{"windows_v1":{"volume":7,"file_id":[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,255]}},"files":{},"modes":{},"truncated":false,"unavailable":null});
        let snapshot: Snapshot = serde_json::from_value(native.clone()).unwrap();
        assert_eq!(serde_json::to_value(snapshot).unwrap(), native);
    }

    #[test]
    fn patch_header_rewrite_preserves_content_and_quotes_unusual_paths() {
        let patch = "diff --git a/C:/tmp/before b/C:/tmp/after\n--- a/C:/tmp/before\n+++ b/C:/tmp/after\n@@ -1 +1 @@\n--- content\n+++ content\n";
        let rewritten = patch_labels(patch, "a\nb.txt", true, true);
        assert!(rewritten.contains("--- \"a/a\\012b.txt\"\n"));
        assert!(rewritten.ends_with("@@ -1 +1 @@\n--- content\n+++ content\n"));
        assert!(!rewritten.contains("C:/tmp"));
    }

    #[tokio::test]
    async fn added_and_deleted_file_previews_use_git_null_path_on_each_host() {
        let dir = tempfile::tempdir().unwrap();
        repository(dir.path());
        let before = Snapshot::capture(dir.path()).await.unwrap();
        std::fs::remove_file(dir.path().join("delete.txt")).unwrap();
        std::fs::write(dir.path().join("new.txt"), "new content\n").unwrap();
        let after = Snapshot::capture(dir.path()).await.unwrap();
        assert!(
            before
                .patch_between(&after, "delete.txt")
                .await
                .unwrap()
                .0
                .contains("+++ /dev/null")
        );
        assert!(
            before
                .patch_between(&after, "new.txt")
                .await
                .unwrap()
                .0
                .contains("--- /dev/null")
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_legacy_snapshot_is_refused_before_any_restore() {
        let dir = tempfile::tempdir().unwrap();
        repository(dir.path());
        let before = Snapshot::capture(dir.path()).await.unwrap();
        std::fs::write(dir.path().join("dirty.txt"), "changed\n").unwrap();
        let after = Snapshot::capture(dir.path()).await.unwrap();
        let mut frozen = FrozenTurn::freeze("r".into(), before, after, false, false)
            .await
            .unwrap();
        frozen.validate_undo(dir.path()).unwrap();
        frozen.before.windows_files_v1.clear();
        frozen.after.windows_files_v1.clear();
        assert!(
            frozen
                .validate_undo(dir.path())
                .unwrap_err()
                .to_string()
                .contains("Windows")
        );
        assert!(frozen.restore(dir.path()).await.is_err());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("dirty.txt")).unwrap(),
            "changed\n"
        );
    }

    pub(super) fn git(root: &Path, args: &[&str]) {
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
    pub(super) fn repository(root: &Path) {
        git(root, &["init", "-q"]);
        git(root, &["config", "core.autocrlf", "false"]);
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
        assert!(
            before
                .summary(root, &ManagedRecoveryExclusions::empty())
                .await
                .unwrap()
                .entries
                .is_empty()
        );
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
        #[cfg(unix)]
        {
            turn.restore(root).await.unwrap();
            assert_eq!(
                std::fs::read_to_string(root.join("nested/edit.txt")).unwrap(),
                "before\n"
            );
            assert!(root.join("delete.txt").exists());
            assert!(!root.join("new.txt").exists());
        }
        #[cfg(windows)]
        {
            assert!(turn.changes.undo_unavailable.is_none());
            let state = tempfile::tempdir().unwrap();
            let database = std::sync::Arc::new(std::sync::Mutex::new(
                crate::store::Store::open(&state.path().join("store.sqlite")).unwrap(),
            ));
            let prepared = turn.prepare_undo_blocking(root, &database, None).unwrap();
            let mut persisted = turn.clone();
            prepared.record_intent(&mut persisted);
            assert!(persisted.windows_undo_recovery_v2.is_some());
            assert!(persisted.windows_undo_recovery_v1.is_none());
            let restored = prepared.restore().unwrap();
            assert_eq!(
                std::fs::read(root.join("nested/edit.txt")).unwrap(),
                b"before\n"
            );
            assert_eq!(std::fs::read(root.join("delete.txt")).unwrap(), b"remove\n");
            assert!(!root.join("new.txt").exists());
            assert_eq!(turn.patch("nested/edit.txt").await.unwrap().0, patch);
            drop(restored);
            let context = ManagedRecoveryExclusions::load(&database, root).unwrap();
            let after_undo = Snapshot::capture_excluding(root, &context).await.unwrap();
            assert!(
                after_undo
                    .files
                    .keys()
                    .all(|path| !path.starts_with(".latte-work-undo"))
            );
        }
    }
    #[cfg(unix)]
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
        assert!(
            before
                .summary(dir.path(), &ManagedRecoveryExclusions::empty())
                .await
                .unwrap()
                .entries
                .is_empty()
        );
        for n in 0..=COUNT_LIMIT {
            std::fs::write(dir.path().join(format!("file-{n}.txt")), "bounded").unwrap();
        }
        let (paths, truncated) = inventory(dir.path(), &ManagedRecoveryExclusions::empty())
            .await
            .unwrap();
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
            inventory(dir.path(), &ManagedRecoveryExclusions::empty())
                .await
                .unwrap(),
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
        assert!(
            baseline
                .summary(root, &ManagedRecoveryExclusions::empty())
                .await
                .unwrap()
                .entries
                .is_empty()
        );
        let baseline: Snapshot =
            serde_json::from_str(&serde_json::to_string(&baseline).unwrap()).unwrap();
        std::fs::write(root.join("dirty.txt"), "task edit\nsecond line\n").unwrap();
        std::fs::remove_file(root.join("delete.txt")).unwrap();
        std::fs::write(root.join("new.txt"), "new\n").unwrap();
        std::fs::write(root.join("empty.txt"), "").unwrap();
        std::fs::write(root.join("binary.dat"), [0, 4, 2]).unwrap();
        git(root, &["add", "dirty.txt"]);
        let index = std::fs::read(root.join(".git/index")).unwrap();
        let result = baseline
            .summary(root, &ManagedRecoveryExclusions::empty())
            .await
            .unwrap();
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
        let (patch, _) = baseline
            .patch(root, "dirty.txt", &ManagedRecoveryExclusions::empty())
            .await
            .unwrap();
        assert!(
            patch.contains("-existing dirty") && patch.contains("+task edit"),
            "{patch}"
        );
        assert!(
            patch.contains("--- a/dirty.txt") && patch.contains("+++ b/dirty.txt"),
            "{patch}"
        );
        assert_eq!(std::fs::read(root.join(".git/index")).unwrap(), index);
        assert!(
            baseline
                .patch(root, "../private", &ManagedRecoveryExclusions::empty())
                .await
                .is_err()
        );
        assert!(
            baseline
                .patch(root, "existing.txt", &ManagedRecoveryExclusions::empty())
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn workspace_totals_count_net_edits_once_across_staging() {
        let dir = tempfile::tempdir().unwrap();
        repository(dir.path());
        std::fs::write(dir.path().join("dirty.txt"), "staged\n").unwrap();
        git(dir.path(), &["add", "dirty.txt"]);
        std::fs::write(dir.path().join("dirty.txt"), "final\n").unwrap();
        let result = workspace_summary(dir.path(), &ManagedRecoveryExclusions::empty())
            .await
            .unwrap();
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
        assert!(
            baseline
                .summary(&sub, &ManagedRecoveryExclusions::empty())
                .await
                .unwrap()
                .entries
                .is_empty()
        );
        std::fs::write(sub.join("a.txt"), vec![b'a'; FILE_LIMIT + 1]).unwrap();
        let result = baseline
            .summary(&sub, &ManagedRecoveryExclusions::empty())
            .await
            .unwrap();
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
        two.restore(root).await.unwrap();
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
    #[cfg(unix)]
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
    #[cfg(unix)]
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
        assert!(turn.restore(&root).await.is_err());
        assert_eq!(
            std::fs::read_to_string(external.join("dirty.txt")).unwrap(),
            "after\n"
        );
    }
}
