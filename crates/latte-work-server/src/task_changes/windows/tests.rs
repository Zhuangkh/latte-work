use super::*;
use crate::task_changes::tests::repository;
use std::os::windows::fs::OpenOptionsExt;
async fn powershell(root: &Path, script: &str) -> String {
    let mut command = tokio::process::Command::new("pwsh.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env("LATTE_UNDO_TEST_ROOT", root);
    let (status, output) =
        latte_work_platform::process::output(&mut command, Duration::from_secs(10), 32 * 1024)
            .await
            .unwrap();
    assert!(status.success(), "{}", String::from_utf8_lossy(&output));
    String::from_utf8(output).unwrap().trim().to_owned()
}
async fn metadata(root: &Path) -> String {
    powershell(root, "$ErrorActionPreference='Stop'; $p=Join-Path $env:LATTE_UNDO_TEST_ROOT 'dirty.txt'; (Get-Acl -LiteralPath $p).GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]'Owner,Group,Access'); [int][IO.File]::GetAttributes($p)").await
}
async fn frozen(root: &Path, before: Snapshot) -> FrozenTurn {
    let after = Snapshot::capture(root).await.unwrap();
    FrozenTurn::freeze("r".into(), before, after, false, false)
        .await
        .unwrap()
}
#[tokio::test]
async fn acl_attributes_and_readonly_before_image_restore_without_changing_parent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    repository(root);
    let parent = powershell(
        root,
        "(Get-Acl -LiteralPath $env:LATTE_UNDO_TEST_ROOT).Sddl",
    )
    .await;
    powershell(root, r#"$ErrorActionPreference='Stop'
$p=Join-Path $env:LATTE_UNDO_TEST_ROOT 'dirty.txt'
$u=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$a=Get-Acl -LiteralPath $p
$a.SetSecurityDescriptorSddlForm("D:P(A;;FA;;;$u)",[Security.AccessControl.AccessControlSections]::Access)
Set-Acl -LiteralPath $p -AclObject $a
[IO.File]::SetAttributes($p,[IO.FileAttributes]'ReadOnly,Hidden,System,Archive')
"#).await;
    let expected = metadata(root).await;
    let before = Snapshot::capture(root).await.unwrap();
    assert!(
        before
            .windows_files_v1
            .values()
            .all(|m| !m.security.contains('\0'))
    );
    powershell(root, r#"$ErrorActionPreference='Stop'
$p=Join-Path $env:LATTE_UNDO_TEST_ROOT 'dirty.txt'
[IO.File]::SetAttributes($p,[IO.FileAttributes]::Archive)
$a=[IO.FileSystemAclExtensions]::GetAccessControl([IO.FileInfo]::new($p),[Security.AccessControl.AccessControlSections]::Access)
$u=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$a.SetSecurityDescriptorSddlForm("D:P(A;;FA;;;$u)(A;;FR;;;WD)",[Security.AccessControl.AccessControlSections]::Access)
[IO.FileSystemAclExtensions]::SetAccessControl([IO.FileInfo]::new($p),$a)
[IO.File]::WriteAllText($p,'changed')
"#).await;
    let turn = frozen(root, before).await;
    turn.restore(root).await.unwrap();
    assert_eq!(
        std::fs::read(root.join("dirty.txt")).unwrap(),
        b"original\n"
    );
    assert_eq!(metadata(root).await, expected);
    assert_eq!(
        powershell(
            root,
            "(Get-Acl -LiteralPath $env:LATTE_UNDO_TEST_ROOT).Sddl"
        )
        .await,
        parent
    );
    powershell(root, "[IO.File]::SetAttributes((Join-Path $env:LATTE_UNDO_TEST_ROOT 'dirty.txt'),[IO.FileAttributes]::Normal)").await;
}
#[tokio::test]
async fn streams_hardlinks_and_metadata_only_changes_are_explicitly_unavailable() {
    for case in ["stream", "hardlink", "metadata"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        repository(root);
        let before = Snapshot::capture(root).await.unwrap();
        match case {
            "stream" => {
                std::fs::write(root.join("dirty.txt:extra"), b"stream").unwrap();
                std::fs::write(root.join("dirty.txt"), b"changed").unwrap();
            }
            "hardlink" => {
                std::fs::hard_link(root.join("dirty.txt"), root.join("alias.txt")).unwrap();
                std::fs::write(root.join("dirty.txt"), b"changed").unwrap();
            }
            _ => {
                powershell(root, "[IO.File]::SetAttributes((Join-Path $env:LATTE_UNDO_TEST_ROOT 'dirty.txt'),[IO.FileAttributes]'Hidden,Archive')").await;
            }
        }
        let turn = frozen(root, before).await;
        assert!(turn.changes.undo_unavailable.is_some(), "{case}");
        assert!(turn.prepare_undo(root).await.is_err(), "{case}");
        if case != "metadata" {
            assert!(!turn.patch("dirty.txt").await.unwrap().0.is_empty());
        }
    }
}
#[tokio::test]
async fn conflicts_occupation_and_bad_metadata_fail_before_target_writes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    repository(root);
    let before = Snapshot::capture(root).await.unwrap();
    std::fs::write(root.join("binary.dat"), [0, 9, 2]).unwrap();
    std::fs::write(root.join("dirty.txt"), "changed\n").unwrap();
    let mut turn = frozen(root, before).await;
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(root.join("dirty.txt"))
        .unwrap();
    assert!(turn.prepare_undo(root).await.is_err());
    drop(held);
    turn.before
        .windows_files_v1
        .get_mut("dirty.txt")
        .unwrap()
        .security = "invalid descriptor".into();
    assert!(turn.prepare_undo(root).await.is_err());
    assert_eq!(std::fs::read(root.join("binary.dat")).unwrap(), [0, 9, 2]);
    assert_eq!(std::fs::read(root.join("dirty.txt")).unwrap(), b"changed\n");
    assert!(!std::fs::read_dir(root).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".latte-undo-")
    }));
}
#[tokio::test]
async fn junctions_git_case_aliases_and_root_replacement_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    std::fs::create_dir(&root).unwrap();
    repository(&root);
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/a.txt"), "before").unwrap();
    let before = Snapshot::capture(&root).await.unwrap();
    std::fs::write(root.join("sub/a.txt"), "after").unwrap();
    let turn = frozen(&root, before).await;
    assert!(ProjectPath::open(&root, Path::new(".GIT/index")).is_err());
    std::fs::rename(root.join("sub"), root.join("saved")).unwrap();
    powershell(&root, "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path (Join-Path $env:LATTE_UNDO_TEST_ROOT 'sub') -Target (Join-Path $env:LATTE_UNDO_TEST_ROOT 'saved') | Out-Null").await;
    assert!(turn.prepare_undo(&root).await.is_err());
    assert_eq!(std::fs::read(root.join("saved/a.txt")).unwrap(), b"after");
    std::fs::remove_dir(root.join("sub")).unwrap();
    std::fs::rename(root.join("saved"), root.join("sub")).unwrap();
    turn.validate_undo(&root).unwrap();
    std::fs::rename(&root, dir.path().join("old-root")).unwrap();
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/a.txt"), "after").unwrap();
    assert!(turn.prepare_undo(&root).await.is_err());
}
#[tokio::test]
async fn changed_parent_inheritance_is_rejected_before_staging_contents() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    repository(root);
    powershell(root, r#"$ErrorActionPreference='Stop'
$p=$env:LATTE_UNDO_TEST_ROOT
$u=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$a=[IO.FileSystemAclExtensions]::GetAccessControl([IO.DirectoryInfo]::new($p),[Security.AccessControl.AccessControlSections]::Access)
$a.SetSecurityDescriptorSddlForm("D:P(A;OICI;FA;;;$u)",[Security.AccessControl.AccessControlSections]::Access)
[IO.FileSystemAclExtensions]::SetAccessControl([IO.DirectoryInfo]::new($p),$a)
"#).await;
    let before = Snapshot::capture(root).await.unwrap();
    let metadata = native(before.windows_files_v1.get("dirty.txt").unwrap());
    powershell(root, r#"$ErrorActionPreference='Stop'
$p=$env:LATTE_UNDO_TEST_ROOT
$u=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$a=[IO.FileSystemAclExtensions]::GetAccessControl([IO.DirectoryInfo]::new($p),[Security.AccessControl.AccessControlSections]::Access)
$a.SetSecurityDescriptorSddlForm("D:P(A;OICI;FA;;;$u)(A;OICI;FR;;;WD)",[Security.AccessControl.AccessControlSections]::Access)
[IO.FileSystemAclExtensions]::SetAccessControl([IO.DirectoryInfo]::new($p),$a)
"#).await;
    let guard = ProjectPath::open(root, Path::new("dirty.txt")).unwrap();
    assert!(guard.stage(".latte-undo-security", &metadata).is_err());
    assert!(!root.join(".latte-undo-security").exists());
    assert_eq!(
        std::fs::read(root.join("dirty.txt")).unwrap(),
        b"original\n"
    );
}
#[tokio::test]
async fn empty_files_roundtrip_and_missing_parent_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    repository(root);
    std::fs::write(root.join("empty.txt"), []).unwrap();
    let before = Snapshot::capture(root).await.unwrap();
    std::fs::remove_file(root.join("empty.txt")).unwrap();
    std::fs::write(root.join("added-empty.txt"), []).unwrap();
    let turn = frozen(root, before).await;
    turn.restore(root).await.unwrap();
    assert_eq!(std::fs::read(root.join("empty.txt")).unwrap(), b"");
    assert!(!root.join("added-empty.txt").exists());
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/file.txt"), "before").unwrap();
    let before = Snapshot::capture(root).await.unwrap();
    std::fs::remove_dir_all(root.join("sub")).unwrap();
    let turn = frozen(root, before).await;
    assert!(turn.prepare_undo(root).await.is_err());
    assert!(!root.join("sub").exists());
}
#[tokio::test]
async fn prepared_undo_pins_directories_but_partial_failure_is_not_a_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    repository(root);
    let before = Snapshot::capture(root).await.unwrap();
    std::fs::write(root.join("binary.dat"), [0, 9, 2]).unwrap();
    std::fs::write(root.join("dirty.txt"), "changed\n").unwrap();
    let turn = frozen(root, before).await;
    let prepared = turn.prepare_undo(root).await.unwrap();
    assert!(std::fs::rename(root, root.with_extension("moved")).is_err());
    assert!(
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(root.join("dirty.txt"))
            .is_err()
    );
    // A later ordinary writer cannot acquire a handle either. Inject a collision
    // in the second recovery slot instead, so the first file is applied once.
    assert!(std::fs::write(root.join("dirty.txt"), "external").is_err());
    let intent = prepared.files.intent.clone();
    let collision = intent
        .recovery_dir
        .join(intent.entries[1].recovery_name.as_ref().unwrap());
    std::fs::write(&collision, "occupied recovery slot").unwrap();
    assert!(prepared.restore().is_err());
    assert_eq!(
        std::fs::read(&collision).unwrap(),
        b"occupied recovery slot"
    );
    let original = intent
        .recovery_dir
        .join(intent.entries[0].recovery_name.as_ref().unwrap());
    assert_eq!(std::fs::read(&original).unwrap(), [0, 9, 2]);
    assert_eq!(std::fs::read(root.join("binary.dat")).unwrap(), [0, 1, 2]);
    assert_eq!(std::fs::read(root.join("dirty.txt")).unwrap(), b"changed\n");
    assert!(
        turn.prepare_undo(root).await.is_err(),
        "partial application cannot be automatically repeated"
    );
}

#[tokio::test]
async fn prepared_sources_and_published_files_are_held_until_result_commit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    repository(root);
    let before = Snapshot::capture(root).await.unwrap();
    std::fs::write(root.join("dirty.txt"), "after").unwrap();
    let turn = frozen(root, before).await;
    let prepared = turn.prepare_undo(root).await.unwrap();
    let intent = prepared.files.intent.clone();
    assert!(std::fs::write(root.join("dirty.txt"), "late write").is_err());
    assert!(std::fs::rename(root.join("dirty.txt"), root.join("external.txt")).is_err());
    assert!(std::fs::remove_file(root.join("dirty.txt")).is_err());
    let stage = std::fs::read_dir(root)
        .unwrap()
        .map(Result::unwrap)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".latte-undo-")
        })
        .unwrap();
    assert!(std::fs::write(stage.path(), "late stage write").is_err());
    let restored = prepared.restore().unwrap();
    assert!(std::fs::write(root.join("dirty.txt"), "before commit").is_err());
    assert!(std::fs::rename(root, root.with_extension("moved")).is_err());
    let original = intent
        .recovery_dir
        .join(intent.entries[0].recovery_name.as_ref().unwrap());
    assert_eq!(std::fs::read(&original).unwrap(), b"after");
    let id = project_metadata(&File::open(&original).unwrap())
        .unwrap()
        .identity;
    let expected = intent.entries[0].source_identity.as_ref().unwrap();
    assert_eq!((id.volume, id.file_id), (expected.volume, expected.file_id));
    assert!(std::fs::remove_file(&original).is_err());
    drop(restored);
    assert_eq!(
        std::fs::read(root.join("dirty.txt")).unwrap(),
        b"original\n"
    );
    assert_eq!(std::fs::read(&original).unwrap(), b"after");
    std::fs::write(root.join("dirty.txt"), "after commit").unwrap();
    let after = Snapshot::capture(root).await.unwrap();
    assert!(
        after
            .files
            .keys()
            .all(|path| !path.contains("latte-work-undo"))
    );
    assert!(
        !crate::files::git(root, &["status", "--porcelain"])
            .await
            .unwrap()
            .0
            .contains("latte-work-undo")
    );
}

#[tokio::test]
async fn existing_writer_delete_holder_and_stream_alias_cannot_cross_validation() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    repository(root);
    let before = Snapshot::capture(root).await.unwrap();
    std::fs::write(root.join("dirty.txt"), "after").unwrap();
    let turn = frozen(root, before).await;
    for access in [0x4000_0000, 0x0001_0000] {
        // GENERIC_WRITE, DELETE
        let held = std::fs::OpenOptions::new()
            .access_mode(access)
            .share_mode(7)
            .open(root.join("dirty.txt"))
            .unwrap();
        assert!(turn.prepare_undo(root).await.is_err());
        drop(held);
        assert_eq!(std::fs::read(root.join("dirty.txt")).unwrap(), b"after");
    }
    let reader = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(7)
        .open(root.join("dirty.txt"))
        .unwrap();
    let prepared = turn.prepare_undo(root).await.unwrap();
    assert!(
        std::fs::OpenOptions::new()
            .write(true)
            .share_mode(7)
            .open(root.join("dirty.txt::$DATA"))
            .is_err()
    );
    let restored = prepared.restore().unwrap();
    assert_eq!(
        std::fs::read(root.join("dirty.txt")).unwrap(),
        b"original\n"
    );
    assert_eq!(std::io::read_to_string(&reader).unwrap(), "after");
    drop(restored);
}

#[tokio::test]
async fn recovery_reparse_and_creation_denial_leave_all_sources_untouched() {
    for reparse in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        repository(root);
        let before = Snapshot::capture(root).await.unwrap();
        std::fs::write(root.join("binary.dat"), [0, 9, 2]).unwrap();
        std::fs::write(root.join("dirty.txt"), "after").unwrap();
        let turn = frozen(root, before).await;
        let recovery = root.join(".git/latte-work-undo");
        if reparse {
            std::fs::create_dir(root.join("foreign")).unwrap();
            powershell(root, "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path (Join-Path $env:LATTE_UNDO_TEST_ROOT '.git/latte-work-undo') -Target (Join-Path $env:LATTE_UNDO_TEST_ROOT 'foreign') | Out-Null").await;
        } else {
            std::fs::create_dir(&recovery).unwrap();
            powershell(root, r#"$ErrorActionPreference='Stop'
$p=Join-Path $env:LATTE_UNDO_TEST_ROOT '.git/latte-work-undo'
$u=[Security.Principal.WindowsIdentity]::GetCurrent().User
$a=Get-Acl -LiteralPath $p
$r=[Security.AccessControl.FileSystemAccessRule]::new($u,[Security.AccessControl.FileSystemRights]::CreateDirectories,[Security.AccessControl.AccessControlType]::Deny)
$a.AddAccessRule($r)
Set-Acl -LiteralPath $p -AclObject $a
"#).await;
        }
        let result = turn.prepare_undo(root).await;
        if !reparse {
            powershell(root, r#"$ErrorActionPreference='Stop'
$p=Join-Path $env:LATTE_UNDO_TEST_ROOT '.git/latte-work-undo'
$u=[Security.Principal.WindowsIdentity]::GetCurrent().User
$a=Get-Acl -LiteralPath $p
$r=[Security.AccessControl.FileSystemAccessRule]::new($u,[Security.AccessControl.FileSystemRights]::CreateDirectories,[Security.AccessControl.AccessControlType]::Deny)
$a.RemoveAccessRuleAll($r)
Set-Acl -LiteralPath $p -AclObject $a
"#).await;
        }
        assert!(result.is_err(), "reparse={reparse}");
        assert_eq!(std::fs::read(root.join("binary.dat")).unwrap(), [0, 9, 2]);
        assert_eq!(std::fs::read(root.join("dirty.txt")).unwrap(), b"after");
        if reparse {
            assert!(
                std::fs::read_dir(root.join("foreign"))
                    .unwrap()
                    .next()
                    .is_none()
            );
            std::fs::remove_dir(recovery).unwrap();
        }
    }
}

#[tokio::test]
async fn ordinary_project_conflicts_and_legacy_masked_refusals_never_write_targets() {
    for case in [
        "bytes",
        "identity",
        "attributes",
        "acl",
        "missing_parent",
        "missing_metadata",
        "legacy_masked",
        "other_refusal",
        "truncated",
        "unchanged_unsupported",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let state = tempfile::tempdir().unwrap();
        let database = std::sync::Arc::new(std::sync::Mutex::new(
            crate::store::Store::open(&state.path().join("state.sqlite")).unwrap(),
        ));
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/file.txt"), b"before\r\n").unwrap();
        if case == "unchanged_unsupported" {
            std::fs::write(root.join("unchanged.txt"), b"same").unwrap();
            std::fs::write(root.join("unchanged.txt:stream"), b"unsupported").unwrap();
        }
        let before = Snapshot::capture(root).await.unwrap();
        std::fs::write(root.join("sub/file.txt"), b"after\r\n").unwrap();
        let mut turn = frozen(root, before).await;
        match case {
            "bytes" => std::fs::write(root.join("sub/file.txt"), b"external").unwrap(),
            "identity" => {
                std::fs::rename(root.join("sub/file.txt"), root.join("saved.txt")).unwrap();
                std::fs::write(root.join("sub/file.txt"), b"after\r\n").unwrap();
            }
            "attributes" => {
                powershell(root, "[IO.File]::SetAttributes((Join-Path $env:LATTE_UNDO_TEST_ROOT 'sub/file.txt'),[IO.FileAttributes]'Hidden,Archive')").await;
            }
            "acl" => {
                powershell(root, r#"$ErrorActionPreference='Stop'; $p=Join-Path $env:LATTE_UNDO_TEST_ROOT 'sub/file.txt'; $a=[IO.FileSystemAclExtensions]::GetAccessControl([IO.FileInfo]::new($p),[Security.AccessControl.AccessControlSections]::Access); $u=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value; $a.SetSecurityDescriptorSddlForm("D:P(A;;FA;;;$u)",[Security.AccessControl.AccessControlSections]::Access); [IO.FileSystemAclExtensions]::SetAccessControl([IO.FileInfo]::new($p),$a)"#).await;
            }
            "missing_parent" => {
                std::fs::remove_file(root.join("sub/file.txt")).unwrap();
                std::fs::remove_dir(root.join("sub")).unwrap();
            }
            "missing_metadata" => {
                turn.before.windows_files_v1.clear();
            }
            "legacy_masked" => {
                turn.before.undo_unavailable = Some(super::super::WINDOWS_NON_GIT_UNDO.into());
                turn.changes.undo_unavailable = Some(super::super::WINDOWS_NON_GIT_UNDO.into());
            }
            "other_refusal" => {
                turn.changes.undo_unavailable = Some("specific historical refusal".into());
            }
            "truncated" => {
                turn.changes.summary.truncated = true;
            }
            _ => assert!(turn.changes.undo_unavailable.is_some()),
        }
        let target = std::fs::read(root.join("sub/file.txt")).ok();
        assert!(
            turn.prepare_undo_blocking(root, &database, None).is_err(),
            "{case}"
        );
        assert_eq!(
            std::fs::read(root.join("sub/file.txt")).ok(),
            target,
            "{case}"
        );
        assert!(!root.join(".latte-work-undo").exists(), "{case}");
    }
}
