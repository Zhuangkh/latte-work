use super::*;
use std::{
    io::{Read, Write},
    os::windows::io::AsHandle,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

// Independent .NET ACL inspection, only on disposable test directories. No
// PowerShell ACL repair or subprocesses are used by production security code.
fn inspect(script: &str, path: &Path, target: &Path) -> String {
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("LATTE_SECURITY_TEST_PATH", path)
        .env("LATTE_SECURITY_TEST_TARGET", target)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("security fixture timed out");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let mut out = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .take(16_384)
        .read_to_string(&mut out)
        .unwrap();
    let mut err = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .take(16_384)
        .read_to_string(&mut err)
        .unwrap();
    assert!(status.success(), "security fixture: {err}");
    out.trim().to_owned()
}

fn sddl(path: &Path) -> String {
    inspect(
        "$ErrorActionPreference='Stop'; [IO.Directory]::GetAccessControl($env:LATTE_SECURITY_TEST_PATH).GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::Access -bor [Security.AccessControl.AccessControlSections]::Owner)",
        path,
        path,
    )
}

// Resolve SDDL aliases (including RID500's LA) without relaxing any owner,
// control flag, or ACE assertion. This stays independent of the native verifier.
pub(crate) fn descriptor_semantics(descriptor: &str) -> String {
    inspect(
        r#"$ErrorActionPreference='Stop'
$descriptor = [Security.AccessControl.RawSecurityDescriptor]::new($env:LATTE_SECURITY_TEST_TARGET)
$descriptor.Owner.Value
[int]$descriptor.ControlFlags
if ($null -eq $descriptor.DiscretionaryAcl) { 'null'; return }
$descriptor.DiscretionaryAcl.Count
foreach ($ace in $descriptor.DiscretionaryAcl) {
    if ($ace -isnot [Security.AccessControl.CommonAce]) { throw 'Expected a common ACE' }
    [int]$ace.AceType
    [int]$ace.AceFlags
    $ace.AccessMask
    $ace.SecurityIdentifier.Value
    $ace.OpaqueLength
}
"#,
        Path::new(""),
        Path::new(descriptor),
    )
}

#[test]
fn sddl_semantics_resolve_local_administrator_alias_and_preserve_acl_checks() {
    let administrator = inspect(
        "[Security.Principal.SecurityIdentifier]::new('LA').Value",
        Path::new(""),
        Path::new(""),
    );
    assert!(administrator.ends_with("-500"));
    for flags in ["", "OICI"] {
        assert_eq!(
            descriptor_semantics(&format!("O:LAD:P(A;{flags};FA;;;LA)")),
            descriptor_semantics(&format!(
                "O:{administrator}D:P(A;{flags};FA;;;{administrator})"
            )),
        );
    }

    let expected = descriptor_semantics("O:LAD:P(A;OICI;FA;;;LA)");
    for invalid in [
        "O:SYD:P(A;OICI;FA;;;LA)",
        "O:LAD:P(A;OICI;FA;;;SY)",
        "O:LAD:P(A;OICI;FR;;;LA)",
        "O:LAD:(A;OICI;FA;;;LA)",
        "O:LAD:P(A;;FA;;;LA)",
        "O:LAD:P(D;OICI;FA;;;LA)",
        "O:LAD:P(A;OICI;FA;;;LA)(A;;FR;;;WD)",
        "O:LAD:P",
        "O:LAD:NO_ACCESS_CONTROL",
    ] {
        assert_ne!(descriptor_semantics(invalid), expected, "{invalid}");
    }
}

#[test]
fn private_file_rejects_a_different_expected_owner_before_reading() {
    let root = tempfile::tempdir().unwrap();
    let private = ControlDirectory::create_or_validate(&root.path().join("private")).unwrap();
    let path = private.path().join("credential.json");
    create_private_file(&path)
        .unwrap()
        .write_all(b"fixture-secret")
        .unwrap();
    let file = open_private_file(&path).unwrap();
    let mut expected = win32::current_identity().unwrap();
    // Exercise the actual native descriptor reader without taking ownership or
    // requiring an elevated token to install another user's owner SID.
    expected.sid = "S-1-5-18".to_owned();
    assert_eq!(
        win32::verify_private_file(&file, &expected)
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert!(std::fs::write(&path, b"replacement").is_err());
    assert!(std::fs::rename(&path, private.path().join("moved")).is_err());
    drop(file);
    assert_eq!(std::fs::read(&path).unwrap(), b"fixture-secret");
}

#[test]
fn private_file_creation_uses_token_user_and_never_repairs_existing_objects() {
    let root = tempfile::tempdir().unwrap();
    let user = current_user_sid().unwrap();
    let oracle = inspect(
        "[Security.Principal.WindowsIdentity]::GetCurrent().User.Value",
        root.path(),
        root.path(),
    );
    assert_eq!(user, oracle);
    let private = ControlDirectory::create_or_validate(&root.path().join("private")).unwrap();
    let path = private.path().join("credential.json");
    let mut file = create_private_file(&path).unwrap();
    verify_private_file(&file).unwrap();
    file.write_all(b"fixture-secret").unwrap();
    drop(file);
    let descriptor = inspect(
        "[IO.File]::GetAccessControl($env:LATTE_SECURITY_TEST_PATH).GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::Access -bor [Security.AccessControl.AccessControlSections]::Owner)",
        &path,
        &path,
    );
    assert_eq!(
        descriptor_semantics(&descriptor),
        descriptor_semantics(&format!("O:{user}D:P(A;;FA;;;{user})"))
    );
    assert_eq!(
        create_private_file(&path).unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
    let mut reopened = open_private_file(&path).unwrap();
    let mut content = String::new();
    reopened.read_to_string(&mut content).unwrap();
    assert_eq!(content, "fixture-secret");
    // A broad ordinary parent cannot be used to create a purported private file.
    let outside = root.path().join("outside.json");
    assert!(create_private_file(&outside).is_err());
    assert!(!outside.exists());
}

#[test]
fn private_leaf_inherits_nothing_and_pins_until_drop() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("control");
    let before = sddl(root.path());
    let user = current_user_sid().unwrap();
    let guard = ControlDirectory::create_or_validate(&path).unwrap();
    assert!(guard.path().is_dir());
    let expected = descriptor_semantics(&format!("O:{user}D:P(A;OICI;FA;;;{user})"));
    assert_eq!(descriptor_semantics(&sddl(&path)), expected);
    // This regression distinguishes the ordinary parent (previously passed by
    // mistake) from the new leaf whose DACL must contain SE_DACL_PROTECTED.
    assert_eq!(sddl(root.path()), before);
    assert!(std::fs::rename(&path, root.path().join("moved")).is_err());
    assert!(std::fs::remove_dir(&path).is_err());
    drop(guard);
    let reopened = ControlDirectory::create_or_validate(&path).unwrap();
    assert_eq!(descriptor_semantics(&sddl(&path)), expected);
    drop(reopened);
    let reopened = ControlDirectory::open_verified(&path).unwrap();
    drop(reopened);
    std::fs::rename(&path, root.path().join("moved")).unwrap();
}

#[test]
fn existing_broad_acl_is_rejected_without_repair() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("control");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("sentinel"), "unchanged").unwrap();
    let before = sddl(&path);
    for _ in 0..3 {
        assert_eq!(
            ControlDirectory::open_verified(&path).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            ControlDirectory::create_or_validate(&path)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }
    assert_eq!(sddl(&path), before);
    assert_eq!(
        std::fs::read_to_string(path.join("sentinel")).unwrap(),
        "unchanged"
    );
    std::fs::rename(&path, root.path().join("moved")).unwrap(); // error paths released handles
}

#[test]
fn non_exact_acls_are_rejected_without_repair() {
    let root = tempfile::tempdir().unwrap();
    let user = current_user_sid().unwrap();
    let script = r#"$ErrorActionPreference='Stop'
$acl = [Security.AccessControl.DirectorySecurity]::new()
$acl.SetSecurityDescriptorSddlForm($env:LATTE_SECURITY_TEST_TARGET, [Security.AccessControl.AccessControlSections]::Access)
[IO.Directory]::SetAccessControl($env:LATTE_SECURITY_TEST_PATH, $acl)
"#;
    // Empty, extra user, wrong flags, wrong rights, inherit-only, and deny ACEs.
    let descriptors = [
        format!("O:{user}D:P"),
        format!("O:{user}D:NO_ACCESS_CONTROL"),
        format!("O:{user}D:(A;OICI;FA;;;{user})"),
        format!("O:{user}D:P(A;OICI;FA;;;{user})(A;OICI;FR;;;WD)"),
        format!("O:{user}D:P(A;;FA;;;{user})"),
        format!("O:{user}D:P(A;OICI;FR;;;{user})"),
        format!("O:{user}D:P(A;OICIIO;FA;;;{user})"),
        format!("O:{user}D:P(D;;FW;;;WD)(A;OICI;FA;;;{user})"),
    ];
    for (index, descriptor) in descriptors.iter().enumerate() {
        let path = root.path().join(format!("control-{index}"));
        let guard = ControlDirectory::create_or_validate(&path).unwrap();
        drop(guard);
        inspect(script, &path, Path::new(descriptor));
        let before = sddl(&path);
        assert!(
            ControlDirectory::open_verified(&path).is_err(),
            "accepted {descriptor}"
        );
        assert!(
            ControlDirectory::create_or_validate(&path).is_err(),
            "repaired {descriptor}"
        );
        assert_eq!(sddl(&path), before);
        // Fixture-only restore, so even the deny/empty-DACL case can be cleaned.
        inspect(
            script,
            &path,
            Path::new(&format!("O:{user}D:P(A;OICI;FA;;;{user})")),
        );
    }
}

#[test]
fn junction_leaf_and_ancestor_are_rejected_without_touching_target() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    std::fs::create_dir(&target).unwrap();
    let junction = root.path().join("junction");
    inspect(
        "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:LATTE_SECURITY_TEST_PATH -Target $env:LATTE_SECURITY_TEST_TARGET | Out-Null",
        &junction,
        &target,
    );
    let before = sddl(&target);
    for path in [&junction, &junction.join("control")] {
        assert_eq!(
            ControlDirectory::create_or_validate(path)
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            ControlDirectory::open_verified(path).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
    assert_eq!(sddl(&target), before);
    assert!(!target.join("control").exists());
    std::fs::remove_dir(&junction).unwrap();
}

#[test]
fn missing_and_ambiguous_paths_fail_without_creation() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");
    assert_eq!(
        ControlDirectory::open_verified(&missing)
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    assert!(ControlDirectory::create_or_validate(&missing.join("control")).is_err());
    assert!(!missing.exists());
    for path in [
        "relative-control",
        r"C:\",
        r"C:control",
        r"\control",
        r"\\server\share\control",
        r"\\.\pipe\control",
        r"C:\x\..\control",
        r"C:\x\.\control",
        r"C:\x\control.",
        r"C:\x\control ",
        r"C:\x\control:stream",
        r"C:\x\NUL",
        r"C:\x\COM1.txt",
        r"C:\x\LPT¹",
        "C:\\x\\bad\0name",
    ] {
        assert_eq!(
            ControlDirectory::create_or_validate(Path::new(path))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput,
            "{path:?}"
        );
    }
}

#[test]
fn native_unicode_and_extended_length_paths_work() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("a".repeat(150));
    std::fs::create_dir(&parent).unwrap();
    let path = parent.join(format!("控制目录-{}", "b".repeat(150)));
    let guard = ControlDirectory::create_or_validate(&path).unwrap();
    assert!(guard.path().is_dir());
    let second = ControlDirectory::open_verified(guard.path()).unwrap();
    assert_eq!(second.path(), guard.path());
}

#[test]
fn file_handle_cannot_be_used_as_peer_identity() {
    let file = tempfile::tempfile().unwrap();
    let error = verify_pipe_peer(file.as_handle()).unwrap_err();
    assert!(error.to_string().contains("GetNamedPipeInfo"), "{error}");
    let error = verify_pipe_server(file.as_handle()).unwrap_err();
    assert!(error.to_string().contains("GetNamedPipeInfo"), "{error}");
}
