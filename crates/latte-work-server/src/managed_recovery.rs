//! Same-user provenance for private, project-local Windows Undo storage.
//! Registry, manifest, native identity and protected ACL must agree before use.
use anyhow::Result;
use std::path::Path;

pub struct ManagedRecoveryExclusions {
    #[cfg(windows)]
    stores: Vec<latte_work_platform::fs::project::ManagedRecoveryRoot>,
}
impl ManagedRecoveryExclusions {
    pub fn load(database: &crate::runtime::Database, root: &Path) -> Result<Self> {
        #[cfg(windows)]
        {
            windows::load(database, root)
        }
        #[cfg(unix)]
        {
            let _ = (database, root);
            Ok(Self {})
        }
    }
    /// Git may force-track paths below directories omitted by ordinary inventory.
    /// Discover their namespace ancestors before any live content/scoped diff read.
    pub async fn load_live(database: &crate::runtime::Database, root: &Path) -> Result<Self> {
        let context = Self::load(database, root)?;
        #[cfg(windows)]
        {
            use anyhow::{Context, bail};
            let (inside, partial) =
                match crate::files::git(root, &["rev-parse", "--is-inside-work-tree"]).await {
                    Ok(result) => result,
                    Err(error)
                        if error.to_string() == "无法读取 Git 状态，请确认项目是 Git 仓库" =>
                    {
                        return Ok(context);
                    }
                    Err(error) => return Err(error),
                };
            if partial {
                bail!("Git recovery exclusion discovery exceeds the read limit");
            }
            if inside.trim() == "true" {
                let (prefix, prefix_partial) =
                    crate::files::git(root, &["rev-parse", "--show-prefix"]).await?;
                let (paths, paths_partial) = crate::files::git(
                    root,
                    &[
                        "ls-files",
                        "--cached",
                        "--others",
                        "--exclude-standard",
                        "--full-name",
                        "-z",
                        "--",
                        ".",
                    ],
                )
                .await?;
                if prefix_partial || paths_partial {
                    bail!("Git recovery exclusion discovery exceeds the read limit");
                }
                for path in paths.split('\0').filter(|path| !path.is_empty()) {
                    let relative = path
                        .strip_prefix(prefix.trim_end_matches('\n'))
                        .context("Git path escaped the project prefix")?;
                    context.contains(root, relative)?;
                }
            }
        }
        Ok(context)
    }
    #[cfg(test)]
    pub fn empty() -> Self {
        Self {
            #[cfg(windows)]
            stores: Vec::new(),
        }
    }
    /// Check existing ancestors by full file identity, including case aliases and
    /// deleted leaves. A lexical prefix must never hide unrelated user contents.
    pub fn contains(&self, root: &Path, relative: &str) -> Result<bool> {
        #[cfg(windows)]
        {
            windows::contains(self, root, relative)
        }
        #[cfg(unix)]
        {
            let _ = (root, relative);
            Ok(false)
        }
    }
    pub fn require_visible(&self, root: &Path, relative: &str) -> Result<()> {
        if self.contains(root, relative)? {
            anyhow::bail!("Managed Undo recovery data is unavailable in live workspace review");
        }
        Ok(())
    }
}
#[cfg(windows)]
pub use windows::prepare_store;
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Registration {
    pub kind: String,
    pub version: u32,
    pub store_uuid: String,
    pub project_path: std::path::PathBuf,
    pub project_identity: crate::task_changes::WindowsIdentity,
    pub namespace_path: std::path::PathBuf,
    pub namespace_identity: crate::task_changes::WindowsIdentity,
}

#[cfg(windows)]
mod windows {
    use super::*;
    use anyhow::{Context, bail};
    use latte_work_platform::fs::project::{FileIdentity, ManagedRecoveryRoot};
    use std::io::Read;
    const KIND: &str = "latte_work_managed_undo";
    const MANIFEST_LIMIT: usize = 64 * 1024;
    fn native(id: &crate::task_changes::WindowsIdentity) -> FileIdentity {
        FileIdentity {
            volume: id.volume,
            file_id: id.file_id,
        }
    }
    fn persisted(id: FileIdentity) -> crate::task_changes::WindowsIdentity {
        crate::task_changes::WindowsIdentity {
            volume: id.volume,
            file_id: id.file_id,
        }
    }
    fn verify(record: &Registration) -> Result<ManagedRecoveryRoot> {
        if record.kind != KIND
            || record.version != 2
            || uuid::Uuid::parse_str(&record.store_uuid).is_err()
            || record.project_path.canonicalize()? != record.project_path
            || record.namespace_path != record.project_path.join(".latte-work-undo")
        {
            bail!("Invalid managed Undo registration");
        }
        let guard = ManagedRecoveryRoot::open_verified(
            &record.project_path,
            native(&record.project_identity),
            native(&record.namespace_identity),
        )?;
        if guard.path().canonicalize()? != record.namespace_path.canonicalize()? {
            bail!("Managed Undo namespace path changed");
        }
        let manifest: Registration =
            serde_json::from_slice(&guard.read_owner_manifest(MANIFEST_LIMIT)?)?;
        if &manifest != record {
            bail!("Managed Undo manifest does not match the daemon registry");
        }
        Ok(guard)
    }
    pub fn prepare_store(
        database: &crate::runtime::Database,
        root: &Path,
        expected_project: FileIdentity,
    ) -> Result<(ManagedRecoveryRoot, Registration)> {
        let root = root.canonicalize()?;
        let records = crate::runtime::db(database, |store| store.windows_undo_stores())?;
        for record in records {
            if record.project_path == root {
                let guard = verify(&record)?;
                if guard.project_identity()? != expected_project {
                    bail!("Project identity changed");
                }
                return Ok((guard, record));
            }
        }
        // CREATE_NEW refuses every collision, including incomplete/orphaned stores.
        let guard = ManagedRecoveryRoot::create_new(&root, expected_project)?;
        let record = Registration {
            kind: KIND.into(),
            version: 2,
            store_uuid: uuid::Uuid::new_v4().to_string(),
            project_path: root,
            project_identity: persisted(guard.project_identity()?),
            namespace_path: guard.path().to_owned(),
            namespace_identity: persisted(guard.identity()?),
        };
        guard.write_owner_manifest_new(&serde_json::to_vec(&record)?, MANIFEST_LIMIT)?;
        crate::task_changes::undo_phase_checkpoint("registration")?;
        // Guards remain held and the flushed owner manifest precedes COMMIT.
        crate::runtime::db(database, |store| store.register_windows_undo_store(&record))?;
        Ok((guard, record))
    }
    pub fn load(
        database: &crate::runtime::Database,
        root: &Path,
    ) -> Result<ManagedRecoveryExclusions> {
        let root = root.canonicalize()?;
        let records = crate::runtime::db(database, |store| store.windows_undo_stores())?;
        let mut result = ManagedRecoveryExclusions { stores: Vec::new() };
        for record in records {
            // Compare ancestors by identity; do not mistake case aliases for other projects.
            let lexical_overlap = root.ancestors().any(|path| path == record.project_path)
                || record.project_path.ancestors().any(|path| path == root);
            let overlaps = if record.project_path.exists() {
                latte_work_platform::paths::overlap(&root, &record.project_path)?
            } else {
                lexical_overlap
            };
            if overlaps {
                result.stores.push(verify(&record)?);
            }
        }
        if contains(&result, &root, "")? {
            bail!("Project is inside managed Undo recovery data");
        }
        // Detect recognizable orphans before any traversal or Git content read.
        let mut directories = vec![root.clone()];
        let mut visited = 0;
        while let Some(directory) = directories.pop() {
            for entry in directory.read_dir()? {
                let entry = entry?;
                let name = entry.file_name();
                if matches!(
                    name.to_str(),
                    Some(".git" | "node_modules" | "target" | ".DS_Store")
                ) {
                    continue;
                }
                let path = entry.path();
                let relative =
                    latte_work_platform::paths::relative_text(path.strip_prefix(&root)?)?;
                if contains(&result, &root, &relative)? {
                    continue;
                }
                visited += 1;
                if visited > 5000 {
                    bail!("Managed Undo exclusion scan exceeds the capture limit");
                }
                let metadata = entry.path().symlink_metadata()?;
                if latte_work_platform::fs::is_link(&metadata) {
                    continue;
                }
                if metadata.is_dir() {
                    directories.push(path);
                }
            }
        }
        Ok(result)
    }
    fn require_unregistered_ordinary(path: &Path) -> Result<()> {
        if !path
            .file_name()
            .is_some_and(|name| name.eq_ignore_ascii_case(".latte-work-undo"))
        {
            return Ok(());
        }
        let manifest = path.join("owner.json");
        match manifest.symlink_metadata() {
            Ok(metadata) if latte_work_platform::fs::is_link(&metadata) => {
                bail!("Unregistered Undo manifest is a reparse point")
            }
            Ok(metadata) if !metadata.is_file() => {
                bail!("Unregistered Undo manifest is not an ordinary file")
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        let mut bytes = Vec::new();
        std::fs::File::open(manifest)?
            .take((MANIFEST_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MANIFEST_LIMIT || String::from_utf8_lossy(&bytes).contains(KIND) {
            bail!("Unregistered managed Undo recovery data; original daemon registry is required");
        }
        Ok(())
    }
    pub fn contains(
        context: &ManagedRecoveryExclusions,
        root: &Path,
        relative: &str,
    ) -> Result<bool> {
        latte_work_platform::paths::relative(Path::new(relative))?;
        let target = root.join(relative);
        for ancestor in target.ancestors() {
            match ancestor.symlink_metadata() {
                Ok(metadata) if latte_work_platform::fs::is_link(&metadata) => {
                    bail!("Reparse paths cannot be reviewed safely")
                }
                Ok(metadata) if metadata.is_dir() => {
                    let canonical = ancestor.canonicalize()?;
                    if let (Some(actual), Some(requested)) =
                        (canonical.file_name(), ancestor.file_name())
                        && !actual.eq_ignore_ascii_case(requested)
                    {
                        bail!("Short-name aliases are unsupported in managed Undo review");
                    }
                    let (volume, file_id) = latte_work_platform::fs::directory_identity(ancestor)?;
                    let identity = FileIdentity { volume, file_id };
                    for store in &context.stores {
                        if store.identity()? == identity {
                            return Ok(true);
                        }
                    }
                    require_unregistered_ordinary(ancestor)?;
                }
                Ok(_) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => {
                    return Err(error).context("Cannot verify managed Undo path ancestry");
                }
            }
        }
        Ok(false)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::{
        runtime::{Database, db},
        store::Store,
        task_changes::{FrozenTurn, Snapshot},
    };
    use latte_work_platform::fs::project::FileIdentity;
    use std::sync::{Arc, Mutex};
    fn database(state: &Path) -> Database {
        Arc::new(Mutex::new(
            Store::open(&state.join("state.sqlite")).unwrap(),
        ))
    }
    fn expected(root: &Path) -> FileIdentity {
        let (volume, file_id) = latte_work_platform::fs::directory_identity(root).unwrap();
        FileIdentity { volume, file_id }
    }
    async fn turn(root: &Path) -> FrozenTurn {
        std::fs::write(root.join("file.txt"), b"before\r\n").unwrap();
        let before = Snapshot::capture(root).await.unwrap();
        std::fs::write(root.join("file.txt"), b"after\r\n").unwrap();
        FrozenTurn::freeze(
            "managed".into(),
            before,
            Snapshot::capture(root).await.unwrap(),
            false,
            false,
        )
        .await
        .unwrap()
    }
    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[tokio::test]
    async fn ordinary_collision_is_visible_but_never_adopted_or_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let database = database(state.path());
        std::fs::create_dir(root.path().join(".latte-work-undo")).unwrap();
        std::fs::write(
            root.path().join(".latte-work-undo/user.txt"),
            b"user contents",
        )
        .unwrap();
        let turn = turn(root.path()).await;
        let context = ManagedRecoveryExclusions::load(&database, root.path()).unwrap();
        let snapshot = Snapshot::capture_excluding(root.path(), &context)
            .await
            .unwrap();
        assert!(
            serde_json::to_value(snapshot).unwrap()["files"]
                .get(".latte-work-undo/user.txt")
                .is_some()
        );
        assert!(
            turn.prepare_undo_blocking(root.path(), &database, None)
                .is_err()
        );
        assert_eq!(
            std::fs::read(root.path().join("file.txt")).unwrap(),
            b"after\r\n"
        );
        assert_eq!(
            std::fs::read(root.path().join(".latte-work-undo/user.txt")).unwrap(),
            b"user contents"
        );
        assert!(
            db(&database, |store| store.windows_undo_stores())
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn reopened_registry_manifest_and_identities_must_agree_and_orphans_are_unavailable() {
        for failure in [
            "orphan",
            "manifest",
            "registry",
            "namespace_identity",
            "project_identity",
            "acl",
            "copied",
        ] {
            let root = tempfile::tempdir().unwrap();
            let state = tempfile::tempdir().unwrap();
            let database = database(state.path());
            let (guard, mut record) =
                prepare_store(&database, root.path(), expected(root.path())).unwrap();
            let owner = guard.path().join("owner.json");
            drop(guard);
            assert!(ManagedRecoveryExclusions::load(&database, root.path()).is_ok());
            match failure {
                "orphan" => {
                    let other = tempfile::tempdir().unwrap();
                    let alternate = self::database(other.path());
                    assert!(ManagedRecoveryExclusions::load(&alternate, root.path()).is_err());
                    assert!(prepare_store(&alternate, root.path(), expected(root.path())).is_err());
                    continue;
                }
                "manifest" => {
                    record.store_uuid = uuid::Uuid::new_v4().to_string();
                    std::fs::write(&owner, serde_json::to_vec(&record).unwrap()).unwrap();
                }
                "copied" => {
                    let other = root.path().join("copied/.latte-work-undo");
                    std::fs::create_dir_all(&other).unwrap();
                    std::fs::copy(&owner, other.join("owner.json")).unwrap();
                }
                "acl" => {
                    let command = "$ErrorActionPreference='Stop'; $d=[IO.DirectoryInfo]::new($env:LATTE_UNDO_PATH); $a=[IO.FileSystemAclExtensions]::GetAccessControl($d,[Security.AccessControl.AccessControlSections]::Access); $a.SetAccessRuleProtection($false,$true); [IO.FileSystemAclExtensions]::SetAccessControl($d,$a)";
                    assert!(
                        std::process::Command::new("pwsh")
                            .args(["-NoProfile", "-Command", command])
                            .env("LATTE_UNDO_PATH", root.path().join(".latte-work-undo"))
                            .status()
                            .unwrap()
                            .success()
                    );
                }
                _ => {
                    match failure {
                        "registry" => record.store_uuid = uuid::Uuid::new_v4().to_string(),
                        "namespace_identity" => record.namespace_identity.file_id[0] ^= 1,
                        _ => record.project_identity.file_id[0] ^= 1,
                    }
                    // A different daemon DB must not trust even a syntactically valid registration.
                    let other = tempfile::tempdir().unwrap();
                    let alternate = self::database(other.path());
                    db(&alternate, |store| {
                        store.register_windows_undo_store(&record)
                    })
                    .unwrap();
                    assert!(
                        ManagedRecoveryExclusions::load(&alternate, root.path()).is_err(),
                        "{failure}"
                    );
                    continue;
                }
            }
            assert!(
                ManagedRecoveryExclusions::load(&database, root.path()).is_err(),
                "{failure}"
            );
            assert!(owner.exists());
        }
    }
    #[tokio::test]
    async fn registered_nested_store_is_excluded_but_same_name_user_directory_is_ordinary() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let database = database(state.path());
        let child = root.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let (guard, _) = prepare_store(&database, &child, expected(&child)).unwrap();
        let user = root.path().join("other/.latte-work-undo");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(user.join("user.txt"), b"visible").unwrap();
        let exclusions = ManagedRecoveryExclusions::load(&database, root.path()).unwrap();
        assert!(
            exclusions
                .contains(root.path(), "CHILD/.LATTE-WORK-UNDO/owner.json")
                .unwrap()
        );
        assert!(
            !exclusions
                .contains(root.path(), "other/.latte-work-undo/user.txt")
                .unwrap()
        );
        let snapshot = Snapshot::capture_excluding(root.path(), &exclusions)
            .await
            .unwrap();
        let files = serde_json::to_value(snapshot).unwrap()["files"]
            .as_object()
            .unwrap()
            .clone();
        assert!(files.contains_key("other/.latte-work-undo/user.txt"));
        assert!(
            !files
                .keys()
                .any(|key| key.starts_with("child/.latte-work-undo"))
        );
        drop(guard);
    }
    #[tokio::test]
    async fn git_init_force_tracking_and_renames_cannot_publish_recovery_in_live_views() {
        use latte_work_protocol::{ChangeSection, GitReviewScope};
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let database = database(state.path());
        let turn = turn(root.path()).await;
        drop(
            turn.prepare_undo_blocking(root.path(), &database, None)
                .unwrap()
                .restore()
                .unwrap(),
        );
        let record = db(&database, |store| store.windows_undo_stores())
            .unwrap()
            .pop()
            .unwrap();
        let original = std::fs::read_dir(&record.namespace_path)
            .unwrap()
            .map(Result::unwrap)
            .find(|entry| entry.file_type().unwrap().is_dir())
            .unwrap();
        let original = std::fs::read_dir(original.path())
            .unwrap()
            .map(Result::unwrap)
            .find(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "original")
            })
            .unwrap();
        let relative = latte_work_platform::paths::relative_text(
            original.path().strip_prefix(&record.project_path).unwrap(),
        )
        .unwrap();
        git(root.path(), &["init", "-q"]);
        git(root.path(), &["config", "core.autocrlf", "false"]);
        git(root.path(), &["add", "-f", "."]);
        git(
            root.path(),
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "commit",
                "-qm",
                "including managed data",
            ],
        );
        let head = crate::files::git(root.path(), &["rev-parse", "HEAD"])
            .await
            .unwrap()
            .0
            .trim()
            .to_owned();
        let context = ManagedRecoveryExclusions::load(&database, root.path()).unwrap();
        let historic = crate::git_review::diff_excluding(
            root.path(),
            GitReviewScope::Commit,
            None,
            Some(&head),
            &relative,
            false,
            None,
        )
        .await
        .unwrap();
        assert!(historic.0.contains("after"));
        std::fs::write(original.path(), b"private after staged change").unwrap();
        git(root.path(), &["add", "-f", &relative]);
        std::fs::write(original.path(), b"private after unstaged change").unwrap();
        std::fs::write(
            record.namespace_path.join("untracked-secret"),
            b"untracked private",
        )
        .unwrap();
        std::fs::write(root.path().join("file.txt"), b"visible change\r\n").unwrap();
        let snapshot = Snapshot::capture_excluding(root.path(), &context)
            .await
            .unwrap();
        assert!(
            !serde_json::to_value(snapshot).unwrap()["files"]
                .as_object()
                .unwrap()
                .keys()
                .any(|key| key.contains(".latte-work-undo"))
        );
        let changes = crate::files::changes_excluding(root.path(), &context)
            .await
            .unwrap();
        assert!(
            changes
                .0
                .iter()
                .all(|entry| !entry.path.contains("latte-work-undo"))
        );
        let legacy = crate::files::diff_excluding(root.path(), &context)
            .await
            .unwrap();
        assert!(!legacy.0.contains("private"));
        for scope in [
            GitReviewScope::Worktree,
            GitReviewScope::Staged,
            GitReviewScope::Unstaged,
        ] {
            let review =
                crate::git_review::review_excluding(root.path(), scope, None, Some(&context))
                    .await
                    .unwrap();
            assert!(
                review
                    .entries
                    .iter()
                    .all(|entry| !entry.path.contains("latte-work-undo"))
            );
            assert!(
                crate::git_review::diff_excluding(
                    root.path(),
                    scope,
                    None,
                    Some(&head),
                    &relative,
                    false,
                    Some(&context)
                )
                .await
                .is_err()
            );
        }
        assert!(
            crate::files::change_diff_excluding(
                root.path(),
                &context,
                &relative,
                ChangeSection::Staged
            )
            .await
            .is_err()
        );
        assert_eq!(
            historic,
            crate::git_review::diff_excluding(
                root.path(),
                GitReviewScope::Commit,
                None,
                Some(&head),
                &relative,
                false,
                None
            )
            .await
            .unwrap()
        );
        drop(context);
        git(root.path(), &["reset", "--hard", "-q", "HEAD"]);
        git(root.path(), &["mv", &relative, "visible-renamed.txt"]);
        let context = ManagedRecoveryExclusions::load(&database, root.path()).unwrap();
        assert!(
            crate::files::changes_excluding(root.path(), &context)
                .await
                .unwrap()
                .0
                .iter()
                .all(|entry| entry.path != "visible-renamed.txt")
        );
        assert!(
            !crate::files::diff_excluding(root.path(), &context)
                .await
                .unwrap()
                .0
                .contains("after")
        );
        assert!(
            crate::git_review::review_excluding(
                root.path(),
                GitReviewScope::Staged,
                None,
                Some(&context)
            )
            .await
            .unwrap()
            .entries
            .iter()
            .all(|entry| entry.path != "visible-renamed.txt")
        );
        assert!(
            crate::files::change_diff_excluding(
                root.path(),
                &context,
                "visible-renamed.txt",
                ChangeSection::Staged
            )
            .await
            .unwrap()
            .0
            .is_empty()
        );

        std::fs::write(
            root.path().join("visible-renamed.txt"),
            b"after\r\nextra line\r\n",
        )
        .unwrap();
        let unstaged = crate::git_review::review_excluding(
            root.path(),
            GitReviewScope::Unstaged,
            None,
            Some(&context),
        )
        .await
        .unwrap();
        assert!(
            unstaged
                .entries
                .iter()
                .all(|entry| entry.path != "visible-renamed.txt")
        );
        assert!(
            crate::git_review::diff_excluding(
                root.path(),
                GitReviewScope::Unstaged,
                None,
                Some(&head),
                "visible-renamed.txt",
                false,
                Some(&context)
            )
            .await
            .unwrap()
            .0
            .is_empty()
        );
        assert!(
            crate::files::change_diff_excluding(
                root.path(),
                &context,
                "visible-renamed.txt",
                ChangeSection::Untracked
            )
            .await
            .unwrap()
            .0
            .is_empty()
        );
    }
    #[tokio::test]
    async fn git_force_tracked_orphans_below_inventory_skips_fail_before_live_capture_and_review() {
        use latte_work_protocol::GitReviewScope;
        for parent in ["target", "node_modules"] {
            let root = tempfile::tempdir().unwrap();
            let state = tempfile::tempdir().unwrap();
            let database = database(state.path());
            git(root.path(), &["init", "-q"]);
            git(root.path(), &["config", "core.autocrlf", "false"]);
            let child = root.path().join(parent).join("child");
            std::fs::create_dir_all(&child).unwrap();
            let turn = turn(&child).await;
            drop(
                turn.prepare_undo_blocking(&child, &database, None)
                    .unwrap()
                    .restore()
                    .unwrap(),
            );
            git(root.path(), &["add", "-f", "."]);
            let alternate_state = tempfile::tempdir().unwrap();
            let alternate = self::database(alternate_state.path());
            assert!(
                ManagedRecoveryExclusions::load_live(&alternate, root.path())
                    .await
                    .is_err(),
                "{parent}"
            );
            let incomplete = ManagedRecoveryExclusions::load(&alternate, root.path()).unwrap();
            assert!(
                Snapshot::capture_excluding(root.path(), &incomplete)
                    .await
                    .is_err(),
                "{parent}"
            );
            assert!(
                crate::git_review::review_excluding(
                    root.path(),
                    GitReviewScope::Staged,
                    None,
                    Some(&incomplete)
                )
                .await
                .is_err(),
                "{parent}"
            );
            let registered = ManagedRecoveryExclusions::load_live(&database, root.path())
                .await
                .unwrap();
            let snapshot = Snapshot::capture_excluding(root.path(), &registered)
                .await
                .unwrap();
            assert!(
                serde_json::to_value(snapshot).unwrap()["files"]
                    .as_object()
                    .unwrap()
                    .keys()
                    .all(|key| !key.contains("latte-work-undo"))
            );
            let review = crate::git_review::review_excluding(
                root.path(),
                GitReviewScope::Staged,
                None,
                Some(&registered),
            )
            .await
            .unwrap();
            assert!(
                review
                    .entries
                    .iter()
                    .all(|entry| !entry.path.contains("latte-work-undo"))
            );
        }
    }
    #[tokio::test]
    async fn legacy_workspace_diff_preserves_pure_and_edited_visible_renames() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let database = database(state.path());
        git(root.path(), &["init", "-q"]);
        git(root.path(), &["config", "core.autocrlf", "false"]);
        std::fs::write(root.path().join("a.txt"), b"first\nsecond\nthird\n").unwrap();
        git(root.path(), &["add", "."]);
        git(
            root.path(),
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "commit",
                "-qm",
                "seed",
            ],
        );
        git(root.path(), &["mv", "a.txt", "b.txt"]);
        let context = ManagedRecoveryExclusions::load_live(&database, root.path())
            .await
            .unwrap();
        let patch = crate::files::diff_excluding(root.path(), &context)
            .await
            .unwrap()
            .0;
        assert!(
            patch.contains("rename from a.txt") && patch.contains("rename to b.txt"),
            "{patch}"
        );
        std::fs::write(root.path().join("b.txt"), b"first\nsecond\nthird\nextra\n").unwrap();
        git(root.path(), &["add", "b.txt"]);
        let patch = crate::files::diff_excluding(root.path(), &context)
            .await
            .unwrap()
            .0;
        assert!(
            patch.contains("rename from a.txt")
                && patch.contains("rename to b.txt")
                && patch.contains("+extra"),
            "{patch}"
        );
    }
}
