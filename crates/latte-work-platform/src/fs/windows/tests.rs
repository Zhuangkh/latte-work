use crate::fs::{PrivateDir, prepare_state_dir, verify_private_file};
use std::{io::Write, time::Duration};

#[tokio::test]
async fn private_files_are_verified_before_writing_and_after_atomic_replace() {
    let root = tempfile::tempdir().unwrap();
    let state = prepare_state_dir(&root.path().join("状态 data")).unwrap();
    assert!(PrivateDir::prepare(&state).is_err()); // ordinary inherited ACL is not relabelled private
    let private = PrivateDir::prepare(&state.join("secrets")).unwrap();
    for bytes in [b"first".as_slice(), b"second".as_slice()] {
        let mut temp = private.tempfile("credential-", ".json").unwrap();
        verify_private_file(temp.as_file()).unwrap();
        temp.write_all(bytes).unwrap();
        temp.persist(private.path().join("credentials.json"))
            .unwrap();
        private.protect_file("credentials.json").unwrap();
    }
    assert_eq!(
        std::fs::read(private.path().join("credentials.json")).unwrap(),
        b"second"
    );
    let mut command = tokio::process::Command::new("powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-Command", r#"
$ErrorActionPreference='Stop'
$p=$env:LATTE_PRIVATE_TEST_FILE
$acl=[IO.File]::GetAccessControl($p)
$rule=[Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new('S-1-1-0'),'Read','Allow')
$acl.AddAccessRule($rule)
[IO.File]::SetAccessControl($p,$acl)
"#]).env("LATTE_PRIVATE_TEST_FILE", private.path().join("credentials.json"));
    let (status, _) = crate::process::output(&mut command, Duration::from_secs(10), 4096)
        .await
        .unwrap();
    assert!(status.success());
    assert!(private.protect_file("credentials.json").is_err());
    assert!(private.protect_file("credentials.json").is_err()); // no repair on failure
    assert_eq!(
        std::fs::read(private.path().join("credentials.json")).unwrap(),
        b"second"
    );
}

#[tokio::test]
async fn ordinary_state_rejects_junction_ancestors_and_ambiguous_spelling() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("outside");
    std::fs::create_dir(&target).unwrap();
    let link = root.path().join("junction");
    let mut command = tokio::process::Command::new("powershell.exe");
    command.args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:LATTE_PRIVATE_TEST_LINK -Target $env:LATTE_PRIVATE_TEST_TARGET | Out-Null"])
        .env("LATTE_PRIVATE_TEST_LINK", &link).env("LATTE_PRIVATE_TEST_TARGET", &target);
    assert!(
        crate::process::output(&mut command, Duration::from_secs(10), 4096)
            .await
            .unwrap()
            .0
            .success()
    );
    assert!(prepare_state_dir(&link.join("state")).is_err());
    assert!(!target.join("state").exists());
    assert!(prepare_state_dir(&root.path().join("state.")).is_err());
    assert!(!root.path().join("state").exists());
}
