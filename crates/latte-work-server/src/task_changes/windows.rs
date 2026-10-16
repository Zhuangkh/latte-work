#[cfg(test)]
#[path = "windows/tests.rs"]
mod tests;
use super::{
    CapturedFile, FILE_LIMIT, FrozenTurn, Identity, Snapshot, WindowsFile, WindowsIdentity,
};
use anyhow::{Context, Result, bail};
use latte_work_platform::fs::project::{
    ManagedRecoveryRoot, ProjectMetadata, ProjectPath, PublishedFile, RecoveryDir, StagedFile,
    project_metadata,
};
use std::{
    collections::HashSet,
    fs::File,
    io::{Read, Seek, Write},
    path::Path,
    time::{Duration, Instant},
};
pub(super) const UNDO_SUPPORTED: bool = true;
pub(super) fn require_undo() -> Result<()> {
    Ok(())
}
pub(super) fn identity(path: &Path) -> Result<Identity> {
    let (volume, file_id) = latte_work_platform::fs::directory_identity(path)?;
    Ok(Identity::Windows {
        windows_v1: WindowsIdentity { volume, file_id },
    })
}
fn persisted(metadata: &ProjectMetadata) -> WindowsFile {
    WindowsFile {
        identity: WindowsIdentity {
            volume: metadata.identity.volume,
            file_id: metadata.identity.file_id,
        },
        security: metadata.security.clone(),
        attributes: metadata.attributes,
    }
}
fn native(metadata: &WindowsFile) -> ProjectMetadata {
    ProjectMetadata {
        identity: latte_work_platform::fs::project::FileIdentity {
            volume: metadata.identity.volume,
            file_id: metadata.identity.file_id,
        },
        security: metadata.security.clone(),
        attributes: metadata.attributes,
        write_time: 0,
        change_time: 0,
    }
}
pub(super) fn capture_file(root: &Path, path: &str) -> Result<CapturedFile> {
    let guard = ProjectPath::open(root, Path::new(path));
    let (file, mut reason) = match &guard {
        Ok(guard) => (guard.open_file(false)?, None),
        // Read-only snapshots remain useful on a filesystem without Undo semantics.
        Err(error) => (
            File::open(crate::files::resolve(root, path)?)?,
            Some(format!("Windows Undo 不支持此路径：{path}（{error}）")),
        ),
    };
    let before = project_metadata(&file);
    let mut bytes = Vec::new();
    (&file)
        .take((FILE_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    let metadata = match before {
        Ok(before) => {
            let after = project_metadata(&file)?;
            if before != after {
                bail!("文件在快照采集时发生变化：{path}");
            }
            if before.security.len() > 64 * 1024 {
                reason = Some(format!("文件权限元数据超出限制：{path}"));
                None
            } else {
                Some(persisted(&before))
            }
        }
        Err(error) => {
            reason = Some(format!("此文件无法完整撤销：{path}（{error}）"));
            None
        }
    };
    Ok(CapturedFile {
        bytes,
        mode: None,
        windows: metadata,
        undo_unavailable: reason,
    })
}
pub(super) fn undo_reason(before: &Snapshot, after: &Snapshot) -> Option<String> {
    if let Some(reason) = before
        .undo_unavailable
        .as_ref()
        .or(after.undo_unavailable.as_ref())
    {
        return Some(if reason == super::WINDOWS_NON_GIT_UNDO {
            "此旧版普通目录快照未完整评估 Windows 撤销条件，无法验证；请手动核对".into()
        } else {
            reason.clone()
        });
    }
    for snapshot in [before, after] {
        let mut identities = HashSet::new();
        for (path, bytes) in &snapshot.files {
            if bytes.is_none() {
                continue;
            }
            let Some(meta) = snapshot.windows_files_v1.get(path) else {
                return Some("此历史快照缺少 Windows 文件权限元数据，不能撤销".into());
            };
            if !identities.insert((meta.identity.volume, meta.identity.file_id)) {
                return Some("快照包含同一文件的多个路径，不能撤销".into());
            }
        }
    }
    for (path, old) in &before.files {
        if old.is_some()
            && after.files.get(path) == Some(old)
            && let (Some(a), Some(b)) = (
                before.windows_files_v1.get(path),
                after.windows_files_v1.get(path),
            )
            && (a.attributes != b.attributes || a.security != b.security)
        {
            return Some(format!("此轮包含仅权限或属性变化，需手动核对：{path}"));
        }
    }
    None
}
pub(super) fn validate_before(before: &Snapshot, path: &str) -> Result<()> {
    if before.files.get(path).and_then(Option::as_ref).is_some()
        && !before.windows_files_v1.contains_key(path)
    {
        bail!("缺少原文件 Windows 权限，无法撤销");
    }
    Ok(())
}
fn check_root(turn: &FrozenTurn, guard: &ProjectPath) -> Result<()> {
    let id = guard.root_identity()?;
    if turn.before.root_identity
        != Some(Identity::Windows {
            windows_v1: WindowsIdentity {
                volume: id.volume,
                file_id: id.file_id,
            },
        })
    {
        bail!("项目目录已改变，无法安全撤销");
    }
    Ok(())
}
fn verify_file(snapshot: &Snapshot, path: &str, file: &File, compare_identity: bool) -> Result<()> {
    let expected = snapshot
        .files
        .get(path)
        .and_then(Option::as_ref)
        .context("文件已意外出现")?;
    let expected_meta = snapshot
        .windows_files_v1
        .get(path)
        .context("缺少 Windows 文件权限元数据")?;
    let metadata = project_metadata(file)?;
    let mut file = file;
    file.rewind()?;
    let mut bytes = Vec::new();
    file.take((FILE_LIMIT + 1) as u64).read_to_end(&mut bytes)?;
    let current = persisted(&metadata);
    if &bytes != expected
        || current.security != expected_meta.security
        || current.attributes != expected_meta.attributes
        || (compare_identity && current.identity != expected_meta.identity)
        || project_metadata(file)? != metadata
    {
        bail!("文件在本轮结束后已改变，未撤销：{path}");
    }
    Ok(())
}
pub(super) fn verify_current(turn: &FrozenTurn, root: &Path, path: &str) -> Result<()> {
    let guard = ProjectPath::open(root, Path::new(path))?;
    check_root(turn, &guard)?;
    if turn
        .after
        .files
        .get(path)
        .and_then(Option::as_ref)
        .is_some()
    {
        let file = guard.open_file(true)?;
        verify_file(&turn.after, path, &file, true)?;
        if turn
            .after
            .windows_files_v1
            .get(path)
            .is_some_and(|m| m.attributes & 1 != 0)
        {
            bail!("目标文件只读，未撤销：{path}");
        }
    } else {
        match guard.path().symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            _ => bail!("文件在本轮结束后已出现，未撤销：{path}"),
        }
    }
    Ok(())
}
enum PreparedTarget {
    Write(StagedFile),
    Delete(ProjectPath),
}
struct PreparedFile {
    target: PreparedTarget,
    source: Option<File>,
    recovery_name: Option<String>,
}
impl PreparedFile {
    fn guard(&self) -> &ProjectPath {
        match &self.target {
            PreparedTarget::Write(staged) => staged.target(),
            PreparedTarget::Delete(guard) => guard,
        }
    }
}
pub(super) struct Prepared {
    files: Vec<PreparedFile>,
    recovery: RecoveryDir,
    pub(super) intent: super::UndoRecovery,
    pub(super) managed: Option<crate::managed_recovery::Registration>,
    managed_root: Option<ManagedRecoveryRoot>,
}
pub(super) fn prepare(turn: &FrozenTurn, root: &Path, git_dir: &Path) -> Result<Prepared> {
    let attempt_id = uuid::Uuid::new_v4().to_string();
    let recovery = RecoveryDir::create(git_dir, &attempt_id)?;
    prepare_files(turn, root, git_dir, recovery, attempt_id, None, None)
}
pub(super) fn prepare_registered(
    turn: &FrozenTurn,
    root: &Path,
    git_dir: Option<&Path>,
    database: &crate::runtime::Database,
) -> Result<Prepared> {
    if let Some(git_dir) = git_dir {
        return prepare(turn, root, git_dir);
    }
    let Some(Identity::Windows {
        windows_v1: expected,
    }) = &turn.before.root_identity
    else {
        bail!("Missing project identity");
    };
    super::undo_phase_checkpoint("preparation")?;
    let (guard, registration) = crate::managed_recovery::prepare_store(
        database,
        root,
        latte_work_platform::fs::project::FileIdentity {
            volume: expected.volume,
            file_id: expected.file_id,
        },
    )?;
    let attempt_id = uuid::Uuid::new_v4().to_string();
    let recovery = guard.create_attempt(&attempt_id)?;
    prepare_files(
        turn,
        root,
        root,
        recovery,
        attempt_id,
        Some(registration),
        Some(guard),
    )
}
fn prepare_files(
    turn: &FrozenTurn,
    root: &Path,
    git_dir: &Path,
    recovery: RecoveryDir,
    attempt_id: String,
    managed: Option<crate::managed_recovery::Registration>,
    managed_root: Option<ManagedRecoveryRoot>,
) -> Result<Prepared> {
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut files = Vec::new();
    let mut entries = Vec::new();
    for entry in &turn.changes.summary.entries {
        if Instant::now() >= deadline {
            bail!("准备撤销超时；未修改目标文件");
        }
        let guard = ProjectPath::open(root, Path::new(&entry.path))?;
        check_root(turn, &guard)?;
        let source = if turn
            .after
            .files
            .get(&entry.path)
            .and_then(Option::as_ref)
            .is_some()
        {
            let source = guard.open_file(true)?;
            verify_file(&turn.after, &entry.path, &source, true)?;
            if project_metadata(&source)?.attributes & 1 != 0 {
                bail!("目标文件只读，未撤销：{}", entry.path);
            }
            recovery.require_same_volume(&source)?;
            Some(source)
        } else {
            require_absent(&guard, &entry.path)?;
            // Missing targets must also be on the recovery volume before any mutation.
            if guard.root_identity()?.volume != recovery.identity()?.volume {
                bail!("Undo recovery and project files must be on the same NTFS volume");
            }
            None
        };
        let recovery_name = source
            .as_ref()
            .map(|_| format!("{}.original", uuid::Uuid::new_v4()));
        entries.push(super::UndoRecoveryEntry {
            path: entry.path.clone(),
            source_identity: source
                .as_ref()
                .map(|file| project_metadata(file).map(|metadata| persisted(&metadata).identity))
                .transpose()?,
            recovery_name: recovery_name.clone(),
            operation: if turn
                .before
                .files
                .get(&entry.path)
                .and_then(Option::as_ref)
                .is_some()
            {
                "restore_beforeimage"
            } else {
                "remove_turn_added"
            }
            .into(),
        });
        let target = if let Some(bytes) =
            turn.before.files.get(&entry.path).and_then(Option::as_ref)
        {
            let metadata = turn
                .before
                .windows_files_v1
                .get(&entry.path)
                .context("缺少原文件权限")?;
            let staged = guard.stage(
                &format!(".latte-undo-{}", uuid::Uuid::new_v4()),
                &native(metadata),
            )?;
            // Security is set before content; only our new temporary object is mutated.
            let mut file = staged.file();
            file.write_all(bytes)?;
            staged.set_attributes(metadata.attributes)?;
            file.sync_all()?;
            let captured = persisted(&project_metadata(file)?);
            if captured.attributes != metadata.attributes || captured.security != metadata.security
            {
                bail!("不能精确恢复原文件权限或属性：{}", entry.path);
            }
            PreparedTarget::Write(staged)
        } else {
            PreparedTarget::Delete(guard)
        };
        files.push(PreparedFile {
            target,
            source,
            recovery_name,
        });
    }
    for (entry, file) in turn.changes.summary.entries.iter().zip(&files) {
        if let Some(source) = &file.source {
            verify_file(&turn.after, &entry.path, source, true)?;
        } else {
            require_absent(file.guard(), &entry.path)?;
        }
    }
    let Some(Identity::Windows {
        windows_v1: project_identity,
    }) = &turn.before.root_identity
    else {
        bail!("缺少项目目录身份");
    };
    let id = |id: latte_work_platform::fs::project::FileIdentity| WindowsIdentity {
        volume: id.volume,
        file_id: id.file_id,
    };
    let intent = super::UndoRecovery {
        attempt_id,
        project_root: root.to_owned(),
        project_identity: project_identity.clone(),
        git_dir: git_dir.to_owned(),
        git_identity: id(recovery.git_identity()?),
        recovery_dir: recovery.path().to_owned(),
        recovery_identity: id(recovery.identity()?),
        entries,
    };
    Ok(Prepared {
        files,
        recovery,
        intent,
        managed,
        managed_root,
    })
}
fn require_absent(guard: &ProjectPath, path: &str) -> Result<()> {
    match guard.path().symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => bail!("文件在本轮结束后已出现，未撤销：{path}"),
    }
}
pub(super) struct Restored {
    _managed_root: Option<ManagedRecoveryRoot>,
    recovery: RecoveryDir,
    originals: Vec<File>,
    published: Vec<(String, PublishedFile)>,
    deleted: Vec<ProjectPath>,
}
pub(super) fn restore(prepared: Prepared, turn: &FrozenTurn, _root: &Path) -> Result<Restored> {
    let location = format!(
        "Undo attempt {}；原文件恢复位置：{}",
        prepared.intent.attempt_id,
        prepared.recovery.path().display()
    );
    apply(prepared, turn).with_context(|| location)
}
fn apply(prepared: Prepared, turn: &FrozenTurn) -> Result<Restored> {
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut restored = Restored {
        _managed_root: prepared.managed_root,
        recovery: prepared.recovery,
        originals: Vec::new(),
        published: Vec::new(),
        deleted: Vec::new(),
    };
    // Validate every held source before the first displacement. No path reopen can
    // exchange these objects; the handles reject ordinary writers and deleters.
    for (entry, file) in turn.changes.summary.entries.iter().zip(&prepared.files) {
        check_root(turn, file.guard())?;
        if let Some(source) = &file.source {
            verify_file(&turn.after, &entry.path, source, true)?;
        } else {
            require_absent(file.guard(), &entry.path)?;
        }
    }
    super::undo_phase_checkpoint("validated")?;
    for (index, (entry, file)) in turn
        .changes
        .summary
        .entries
        .iter()
        .zip(prepared.files)
        .enumerate()
    {
        if Instant::now() >= deadline {
            bail!("撤销超时，状态待核对");
        }
        if let Some(source) = file.source {
            restored.originals.push(restored.recovery.displace(
                source,
                file.recovery_name.as_deref().context("缺少恢复名称")?,
            )?);
            if index == 0 {
                super::undo_phase_checkpoint("displaced")?;
            }
        }
        match file.target {
            PreparedTarget::Write(staged) => {
                check_root(turn, staged.target())?;
                let published = staged.publish()?;
                verify_file(&turn.before, &entry.path, published.file(), false)?;
                restored.published.push((entry.path.clone(), published));
            }
            PreparedTarget::Delete(guard) => {
                check_root(turn, &guard)?;
                require_absent(&guard, &entry.path)?;
                restored.deleted.push(guard);
            }
        }
        super::restore_checkpoint(index + 1)?;
    }
    for (path, published) in &restored.published {
        check_root(turn, published.target())?;
        verify_file(&turn.before, path, published.file(), false)?;
    }
    for deleted in &restored.deleted {
        require_absent(deleted, &deleted.path().display().to_string())?;
    }
    Ok(restored)
}
