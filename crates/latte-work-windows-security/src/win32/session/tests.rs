use super::*;
use std::io::{Read, Seek, Write};

fn inspect(script: &str, path: &Path, target: &Path) -> String {
    use std::{
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("LATTE_SESSION_TEST_PATH", path)
        .env("LATTE_SESSION_TEST_TARGET", target)
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
            panic!("native session fixture timed out");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let (mut stdout, mut stderr) = (String::new(), String::new());
    child
        .stdout
        .take()
        .unwrap()
        .take(16_384)
        .read_to_string(&mut stdout)
        .unwrap();
    child
        .stderr
        .take()
        .unwrap()
        .take(16_384)
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(status.success(), "native session fixture: {stderr}");
    stdout.trim().to_owned()
}

fn sddl(path: &Path) -> String {
    inspect(
        "$ErrorActionPreference='Stop'; [IO.File]::GetAccessControl($env:LATTE_SESSION_TEST_PATH).GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::Access -bor [Security.AccessControl.AccessControlSections]::Owner)",
        path,
        path,
    )
}

fn write_user_owned_fixture(path: &Path, contents: &[u8]) {
    let user = current_identity().unwrap();
    // Default TokenOwner can differ from TokenUser under elevation. Establish
    // the session reader's owner precondition explicitly on new fixtures only.
    let mut file = create_private_file(path, &user).unwrap_or_else(|error| {
        panic!(
            "cannot create session fixture owned by TokenUser {}: {error}",
            user.sid
        )
    });
    file.write_all(contents).unwrap();
    drop(file);
    let owner = inspect(
        "$ErrorActionPreference='Stop'; [IO.File]::GetAccessControl($env:LATTE_SESSION_TEST_PATH).GetOwner([Security.Principal.SecurityIdentifier]).Value",
        path,
        path,
    );
    assert_eq!(
        owner, user.sid,
        "session fixture must be owned by TokenUser"
    );
}

#[test]
fn existing_files_preserve_contents_and_acl_with_append_only_access() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("session.jsonl");
    write_user_owned_fixture(&path, b"original\n");
    let before = sddl(&path);
    let mut producer = OpenOptions::new().append(true).open(&path).unwrap();
    let mut file = NativeSessionFile::open(&path, true).unwrap();
    let identity = file.identity().unwrap();
    file.file_mut().rewind().unwrap();
    file.file_mut().write_all(b"title\n").unwrap();
    producer.write_all(b"producer\n").unwrap();
    assert!(std::fs::rename(&path, root.path().join("moved")).is_err());
    assert!(std::fs::rename(root.path(), root.path().with_extension("moved")).is_err());
    assert!(
        file.file().set_len(0).is_err(),
        "append must not allow truncation"
    );
    file.file_mut().rewind().unwrap();
    let mut contents = String::new();
    file.file_mut().read_to_string(&mut contents).unwrap();
    assert_eq!(contents, "original\ntitle\nproducer\n");
    assert_eq!(file.identity().unwrap(), identity);
    assert_eq!(sddl(&path), before);
    drop(file);
    drop(producer);
    let mut read = NativeSessionFile::open(&path, false).unwrap();
    assert_eq!(read.identity().unwrap(), identity);
    assert!(read.file_mut().write_all(b"forbidden").is_err());
    assert!(NativeSessionFile::open(&root.path().join("missing.jsonl"), true).is_err());
    assert!(!root.path().join("missing.jsonl").exists());
}

#[test]
fn native_owner_reader_rejects_a_different_owner_without_acl_repair() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("session.jsonl");
    write_user_owned_fixture(&path, b"original\n");
    let before = sddl(&path);
    let file = NativeSessionFile::open(&path, false).unwrap();
    let mut other = current_identity().unwrap();
    other.sid = "S-1-5-18".to_owned();
    assert_eq!(
        verify_owner(file.file(), &other).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(sddl(&path), before);
    assert_eq!(std::fs::read(&path).unwrap(), b"original\n");
}

#[test]
fn junctions_and_ambiguous_paths_fail_without_reading_or_modifying_targets() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    std::fs::create_dir(&target).unwrap();
    let path = target.join("session.jsonl");
    write_user_owned_fixture(&path, b"outside\n");
    drop(NativeSessionFile::open(&path, false).unwrap());
    let junction = root.path().join("junction");
    inspect(
        "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:LATTE_SESSION_TEST_PATH -Target $env:LATTE_SESSION_TEST_TARGET | Out-Null",
        &junction,
        &target,
    );
    for path in [
        &junction,
        &junction.join("session.jsonl"),
        &path.with_extension("jsonl:stream"),
    ] {
        for append in [false, true] {
            assert!(NativeSessionFile::open(path, append).is_err());
        }
    }
    assert!(NativeSessionFile::open(&target, false).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"outside\n");
    std::fs::remove_dir(junction).unwrap();
}
