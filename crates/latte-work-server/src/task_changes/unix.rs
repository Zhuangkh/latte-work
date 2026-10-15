use super::{CapturedFile, FILE_LIMIT, FrozenTurn, Identity, Snapshot};
use anyhow::{Context, Result, bail};
use std::{
    fs::{File, Metadata, Permissions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    time::{Duration, Instant},
};
pub(super) const UNDO_SUPPORTED: bool = true;
pub(super) fn identity(path: &Path) -> Result<Identity> {
    let metadata = path.metadata()?;
    Ok(Identity::Unix((metadata.dev(), metadata.ino())))
}
fn mode(metadata: &Metadata) -> u32 {
    metadata.mode()
}
pub(super) fn require_undo() -> Result<()> {
    Ok(())
}
pub(super) fn capture_file(root: &Path, path: &str) -> Result<CapturedFile> {
    let file = File::open(root.join(path))?;
    let before = file.metadata()?;
    let mut bytes = Vec::new();
    (&file)
        .take((FILE_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    if before.len() != after.len()
        || before.modified()? != after.modified()?
        || mode(&before) != mode(&after)
    {
        bail!("文件在快照采集时发生变化：{path}");
    }
    Ok(CapturedFile {
        bytes,
        mode: Some(mode(&before)),
        windows: None,
        undo_unavailable: None,
    })
}
pub(super) fn undo_reason(before: &Snapshot, after: &Snapshot) -> Option<String> {
    before
        .undo_unavailable
        .clone()
        .or_else(|| after.undo_unavailable.clone())
}
pub(super) fn validate_before(before: &Snapshot, path: &str) -> Result<()> {
    if before.files.get(path).and_then(Option::as_ref).is_some() && !before.modes.contains_key(path)
    {
        bail!("缺少原文件权限，无法撤销");
    }
    Ok(())
}
pub(super) fn verify_current(turn: &FrozenTurn, root: &Path, path: &str) -> Result<()> {
    verify(&turn.after, root, path)
}
fn verify(snapshot: &Snapshot, root: &Path, path: &str) -> Result<()> {
    let target = root.join(path);
    match (
        target.symlink_metadata(),
        snapshot.files.get(path).and_then(Option::as_ref),
    ) {
        (Err(error), None) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        (Ok(meta), Some(bytes))
            if meta.is_file()
                && !meta.file_type().is_symlink()
                && meta.len() <= FILE_LIMIT as u64 =>
        {
            let mut current = Vec::new();
            File::open(&target)?
                .take((FILE_LIMIT + 1) as u64)
                .read_to_end(&mut current)?;
            if &current != bytes || snapshot.modes.get(path).is_none_or(|m| *m != mode(&meta)) {
                bail!("文件在本轮结束后已改变，未撤销：{path}");
            }
            Ok(())
        }
        _ => bail!("文件在本轮结束后已改变，未撤销：{path}"),
    }
}
pub(super) type Prepared = Vec<Option<tempfile::NamedTempFile>>;
pub(super) fn prepare(turn: &FrozenTurn, root: &Path) -> Result<Prepared> {
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut files = Vec::new();
    for entry in &turn.changes.summary.entries {
        if Instant::now() > deadline {
            bail!("准备撤销超时；未修改目标文件");
        }
        turn.verify_current(root, &entry.path)?;
        let target = root.join(&entry.path);
        let staged =
            if let Some(bytes) = turn.before.files.get(&entry.path).and_then(Option::as_ref) {
                let mode = turn
                    .before
                    .modes
                    .get(&entry.path)
                    .context("缺少原文件权限，无法撤销")?;
                let mut file =
                    tempfile::NamedTempFile::new_in(target.parent().context("文件目录缺失")?)?;
                file.write_all(bytes)?;
                file.as_file()
                    .set_permissions(Permissions::from_mode(mode & 0o777))?;
                file.as_file().sync_all()?;
                Some(file)
            } else {
                None
            };
        files.push(staged);
    }
    Ok(files)
}
pub(super) type Restored = ();
pub(super) fn restore(files: Prepared, turn: &FrozenTurn, root: &Path) -> Result<Restored> {
    let deadline = Instant::now() + Duration::from_secs(12);
    for (index, (entry, file)) in turn.changes.summary.entries.iter().zip(files).enumerate() {
        if Instant::now() > deadline {
            bail!("撤销超时，状态待核对");
        }
        turn.verify_current(root, &entry.path)?;
        let target = root.join(&entry.path);
        if let Some(file) = file {
            if turn
                .after
                .files
                .get(&entry.path)
                .and_then(Option::as_ref)
                .is_some()
            {
                file.persist(&target)?;
            } else {
                file.persist_noclobber(&target)?;
            }
        } else {
            std::fs::remove_file(&target)?;
        }
        super::restore_checkpoint(index + 1)?;
    }
    for entry in &turn.changes.summary.entries {
        verify(&turn.before, root, &entry.path)?;
    }
    Ok(())
}
