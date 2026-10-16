use super::*;
use crate::tests::descriptor_semantics;
use std::os::windows::ffi::OsStringExt;

const ATTEMPT: &str = "b644b328-e249-4e75-95c8-372931139ab0";
const ENTRY: &str = "76ab3299-e249-4e75-95c8-372931139ab0.original";

fn project_identity(path: &Path) -> crate::DirectoryIdentity {
    directory_identity(&open_directory(path).unwrap()).unwrap()
}

fn changed_identity(mut identity: crate::DirectoryIdentity) -> crate::DirectoryIdentity {
    identity.file_id[0] ^= 1;
    identity
}

fn writable_security_handle(path: &Path) -> File {
    OpenOptions::new()
        .access_mode(READ_CONTROL | WRITE_DAC | WRITE_OWNER)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .unwrap()
}

#[test]
fn new_root_manifest_and_attempt_are_private_and_reopen_by_registered_identity() {
    let project = tempfile::tempdir().unwrap();
    let expected = project_identity(project.path());
    let root = ManagedRecoveryRoot::create_new(project.path(), expected).unwrap();
    let store = root.identity().unwrap();
    assert_eq!(root.project_identity().unwrap(), expected);
    let user = current_identity().unwrap();
    verify_directory(root.directories.last().unwrap(), &user).unwrap();
    let descriptor = security_text(root.directories.last().unwrap()).unwrap();
    assert_eq!(
        descriptor_semantics(&descriptor),
        descriptor_semantics(&format!("O:{0}D:P(A;OICI;FA;;;{0})", user.sid))
    );
    root.write_owner_manifest_new(b"{\"version\":1}", 1024)
        .unwrap();
    assert_eq!(root.read_owner_manifest(1024).unwrap(), b"{\"version\":1}");
    assert!(root.write_owner_manifest_new(b"replace", 1024).is_err());
    let manifest = File::open(root.path().join(MANIFEST)).unwrap();
    verify_manifest(&manifest).unwrap();
    drop(manifest);
    let attempt = root.create_attempt(ATTEMPT).unwrap();
    verify_directory(attempt.directories.last().unwrap(), &user).unwrap();
    assert_eq!(attempt.git_identity().unwrap(), expected);
    assert!(root.create_attempt(ATTEMPT).is_err());
    drop(attempt);
    drop(root);
    let reopened = ManagedRecoveryRoot::open_verified(project.path(), expected, store).unwrap();
    assert_eq!(
        reopened.read_owner_manifest(1024).unwrap(),
        b"{\"version\":1}"
    );
}

#[test]
fn collisions_and_wrong_identities_do_not_adopt_or_modify_existing_objects() {
    let project = tempfile::tempdir().unwrap();
    let expected = project_identity(project.path());
    assert!(ManagedRecoveryRoot::create_new(project.path(), changed_identity(expected)).is_err());
    assert!(!project.path().join(NAMESPACE).exists());
    let namespace = project.path().join(NAMESPACE);
    std::fs::create_dir(&namespace).unwrap();
    std::fs::write(namespace.join("foreign"), b"untouched").unwrap();
    let before = security_text(&open_directory(&namespace).unwrap()).unwrap();
    assert!(ManagedRecoveryRoot::create_new(project.path(), expected).is_err());
    assert!(
        ManagedRecoveryRoot::open_verified(project.path(), expected, project_identity(&namespace))
            .is_err()
    );
    assert_eq!(
        security_text(&open_directory(&namespace).unwrap()).unwrap(),
        before
    );
    assert_eq!(
        std::fs::read(namespace.join("foreign")).unwrap(),
        b"untouched"
    );

    let other = tempfile::tempdir().unwrap();
    let expected = project_identity(other.path());
    let root = ManagedRecoveryRoot::create_new(other.path(), expected).unwrap();
    let store = root.identity().unwrap();
    drop(root);
    assert!(
        ManagedRecoveryRoot::open_verified(other.path(), changed_identity(expected), store)
            .is_err()
    );
    assert!(
        ManagedRecoveryRoot::open_verified(other.path(), expected, changed_identity(store))
            .is_err()
    );
    assert_eq!(
        std::fs::read_dir(other.path().join(NAMESPACE))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn manifest_bounds_and_foreign_files_fail_without_overwrite() {
    let project = tempfile::tempdir().unwrap();
    let root =
        ManagedRecoveryRoot::create_new(project.path(), project_identity(project.path())).unwrap();
    for bound in [0, MANIFEST_LIMIT + 1, usize::MAX] {
        assert!(root.write_owner_manifest_new(b"payload", bound).is_err());
        assert!(root.read_owner_manifest(bound).is_err());
    }
    assert!(root.write_owner_manifest_new(b"too long", 3).is_err());
    assert!(!root.path().join(MANIFEST).exists());
    root.write_owner_manifest_new(b"12345", 5).unwrap();
    assert!(root.read_owner_manifest(4).is_err());
    assert_eq!(root.read_owner_manifest(5).unwrap(), b"12345");
    let file = writable_security_handle(&root.path().join(MANIFEST));
    let sid = current_identity().unwrap().sid;
    apply_security(&file, &format!("O:{sid}G:{sid}D:(A;;FA;;;{sid})")).unwrap();
    drop(file);
    assert!(
        root.read_owner_manifest(5).is_err(),
        "unprotected manifest must fail"
    );
    assert_eq!(std::fs::read(root.path().join(MANIFEST)).unwrap(), b"12345");
}

#[test]
fn acl_changes_are_refused_without_repair_and_owner_checks_reject_another_sid() {
    let project = tempfile::tempdir().unwrap();
    let expected = project_identity(project.path());
    let root = ManagedRecoveryRoot::create_new(project.path(), expected).unwrap();
    let store = root.identity().unwrap();
    let directory = root.directories.last().unwrap();
    let before = security_text(directory).unwrap();
    let other = Identity {
        sid: "S-1-5-18".into(),
    };
    assert!(verify_directory(directory, &other).is_err());
    assert_eq!(security_text(directory).unwrap(), before);
    let sid = current_identity().unwrap().sid;
    let handle = writable_security_handle(root.path());
    apply_security(
        &handle,
        &format!("O:{sid}G:{sid}D:P(A;OICI;FA;;;{sid})(A;OICI;FR;;;WD)"),
    )
    .unwrap();
    let changed = security_text(&handle).unwrap();
    assert!(root.create_attempt(ATTEMPT).is_err());
    assert!(root.write_owner_manifest_new(b"payload", 10).is_err());
    assert!(ManagedRecoveryRoot::open_verified(project.path(), expected, store).is_err());
    assert_eq!(security_text(&handle).unwrap(), changed);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn retained_attempt_pins_namespace_and_project_and_preserves_displaced_security() {
    let parent = tempfile::tempdir().unwrap();
    let project = parent.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let target = project.join("source.txt");
    std::fs::write(&target, b"afterimage").unwrap();
    let guard = ProjectPath::open(&project, Path::new("source.txt")).unwrap();
    let source = guard.open_file(true).unwrap();
    let original = project_metadata(&source).unwrap();
    let root = ManagedRecoveryRoot::create_new(&project, guard.root_identity().unwrap()).unwrap();
    let attempt = root.create_attempt(ATTEMPT).unwrap();
    drop(guard);
    drop(root);
    assert!(std::fs::rename(&project, parent.path().join("replaced")).is_err());
    assert!(std::fs::rename(project.join(NAMESPACE), project.join("other")).is_err());
    assert!(std::fs::rename(attempt.path(), project.join("moved-attempt")).is_err());
    let displaced = attempt.displace(source, ENTRY).unwrap();
    assert_eq!(
        project_metadata(&displaced).unwrap().security,
        original.security
    );
    assert_eq!(directory_identity(&displaced).unwrap(), original.identity);
    assert_eq!(
        std::fs::read(attempt.path().join(ENTRY)).unwrap(),
        b"afterimage"
    );
    assert!(!target.exists());
}

#[test]
fn unsafe_attempt_names_and_existing_private_namespace_never_get_adopted() {
    let project = tempfile::tempdir().unwrap();
    let expected = project_identity(project.path());
    let root = ManagedRecoveryRoot::create_new(project.path(), expected).unwrap();
    for name in ["", "..", "../escape", "name:stream", "not-a-uuid"] {
        assert!(root.create_attempt(name).is_err());
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    drop(root);
    assert!(ManagedRecoveryRoot::create_new(project.path(), expected).is_err());
    assert_eq!(
        std::fs::read_dir(project.path().join(NAMESPACE))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn native_short_alias_is_rejected_when_the_volume_provides_one() {
    let parent = tempfile::tempdir().unwrap();
    let project = parent.path().join("long-project-directory-for-short-alias");
    std::fs::create_dir(&project).unwrap();
    let path = wide(project.as_os_str()).unwrap();
    let mut short = vec![0u16; 32768];
    // SAFETY: terminated input and bounded writable output remain live for this call.
    let length = unsafe { GetShortPathNameW(path.as_ptr(), short.as_mut_ptr(), short.len() as u32) }
        as usize;
    assert!(length > 0 && length < short.len());
    let short = PathBuf::from(std::ffi::OsString::from_wide(&short[..length]));
    if short.file_name() == project.file_name() {
        // NTFS commonly disables new 8.3 names; the spelling policy is exercised
        // only when the filesystem actually supplies an alternate spelling.
        return;
    }
    let expected = project_identity(&project);
    assert!(ManagedRecoveryRoot::create_new(&short, expected).is_err());
    assert!(!project.join(NAMESPACE).exists());
}

fn junction(path: &Path, target: &Path) {
    use std::{
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:LATTE_UNDO_LINK -Target $env:LATTE_UNDO_TARGET | Out-Null"])
        .env("LATTE_UNDO_LINK", path)
        .env("LATTE_UNDO_TARGET", target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "junction fixture creation failed");
            return;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("junction fixture timed out");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn project_and_namespace_junctions_are_rejected_without_target_writes() {
    let parent = tempfile::tempdir().unwrap();
    let project = parent.path().join("project");
    let outside = parent.path().join("outside");
    std::fs::create_dir(&project).unwrap();
    std::fs::create_dir(&outside).unwrap();
    let alias = parent.path().join("alias");
    junction(&alias, &project);
    let expected = project_identity(&project);
    assert!(ManagedRecoveryRoot::create_new(&alias, expected).is_err());
    assert!(!project.join(NAMESPACE).exists());
    std::fs::remove_dir(&alias).unwrap();
    junction(&project.join(NAMESPACE), &outside);
    let before = security_text(&open_directory(&outside).unwrap()).unwrap();
    assert!(ManagedRecoveryRoot::create_new(&project, expected).is_err());
    assert!(
        ManagedRecoveryRoot::open_verified(&project, expected, project_identity(&outside)).is_err()
    );
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    assert_eq!(
        security_text(&open_directory(&outside).unwrap()).unwrap(),
        before
    );
    std::fs::remove_dir(project.join(NAMESPACE)).unwrap();
}

#[test]
fn manifest_hard_links_and_reparse_leaves_are_rejected_before_read() {
    let parent = tempfile::tempdir().unwrap();
    let project = parent.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let root = ManagedRecoveryRoot::create_new(&project, project_identity(&project)).unwrap();
    root.write_owner_manifest_new(b"private manifest", 100)
        .unwrap();
    let alias = parent.path().join("manifest-alias");
    std::fs::hard_link(root.path().join(MANIFEST), &alias).unwrap();
    assert!(root.read_owner_manifest(100).is_err());
    assert_eq!(std::fs::read(&alias).unwrap(), b"private manifest");
    std::fs::remove_file(&alias).unwrap();
    assert_eq!(root.read_owner_manifest(100).unwrap(), b"private manifest");
    std::fs::remove_file(root.path().join(MANIFEST)).unwrap();
    let outside = parent.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("foreign"), b"untouched").unwrap();
    junction(&root.path().join(MANIFEST), &outside);
    assert!(root.read_owner_manifest(100).is_err());
    assert!(root.write_owner_manifest_new(b"replace", 100).is_err());
    assert_eq!(
        std::fs::read(outside.join("foreign")).unwrap(),
        b"untouched"
    );
    std::fs::remove_dir(root.path().join(MANIFEST)).unwrap();
}
