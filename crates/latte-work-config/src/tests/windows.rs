use super::*;
use std::process::{Command, Stdio};

fn inspect(script: &str, path: &Path, target: &Path) -> String {
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("LATTE_PROVIDER_TEST_PATH", path)
        .env("LATTE_PROVIDER_TEST_TARGET", target)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let mut output = String::new();
            child
                .stdout
                .take()
                .unwrap()
                .take(16_384)
                .read_to_string(&mut output)
                .unwrap();
            let mut error = String::new();
            child
                .stderr
                .take()
                .unwrap()
                .take(16_384)
                .read_to_string(&mut error)
                .unwrap();
            assert!(status.success(), "Provider fixture: {error}");
            return output.trim().to_owned();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Provider fixture timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn sddl(path: &Path) -> String {
    inspect(
        "$ErrorActionPreference='Stop'; [IO.File]::GetAccessControl($env:LATTE_PROVIDER_TEST_PATH).GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::Access -bor [Security.AccessControl.AccessControlSections]::Owner)",
        path,
        path,
    )
}

fn broaden(path: &Path) {
    inspect(
        r#"$ErrorActionPreference='Stop'
$p=$env:LATTE_PROVIDER_TEST_PATH
$directory=[IO.Directory]::Exists($p)
$acl=if ($directory) {[IO.Directory]::GetAccessControl($p)} else {[IO.File]::GetAccessControl($p)}
$rule=[Security.AccessControl.FileSystemAccessRule]::new([Security.Principal.SecurityIdentifier]::new('S-1-1-0'),'Read','Allow')
$acl.AddAccessRule($rule)
if ($directory) {[IO.Directory]::SetAccessControl($p,$acl)} else {[IO.File]::SetAccessControl($p,$acl)}
"#,
        path,
        path,
    );
}

#[test]
fn new_store_reopen_and_replacement_are_private_without_changing_app_parent() {
    let app = tempfile::tempdir().unwrap();
    std::fs::write(app.path().join("unrelated.json"), b"unchanged").unwrap();
    let parent_acl = sddl(app.path());
    let id = {
        let mut store = ProviderStore::open(app.path(), None).unwrap();
        latte_work_platform::fs::open_private_file(&store.path).unwrap();
        save(&mut store, draft("First"))
    };
    let mut store = ProviderStore::open(app.path(), None).unwrap();
    let mut edit = draft("Updated");
    edit.id = Some(id.clone());
    edit.credential = None;
    store.save(edit).unwrap();
    store.bind("local", "claude", Some(id)).unwrap();
    latte_work_platform::fs::open_private_file(&store.path).unwrap();
    assert_eq!(
        store
            .snapshot_for_host("local", "claude")
            .unwrap()
            .unwrap()
            .credential,
        "fixture-private-key"
    );
    assert_eq!(sddl(app.path()), parent_acl);
    assert_eq!(
        std::fs::read(app.path().join("unrelated.json")).unwrap(),
        b"unchanged"
    );
    assert!(!app.path().join("providers.json").exists());
}

#[test]
fn old_app_precedes_server_migration_and_deleted_credentials_do_not_return() {
    let app = tempfile::tempdir().unwrap();
    let server = tempfile::tempdir().unwrap();
    let fixture = tempfile::tempdir().unwrap();
    let id = {
        let mut store = ProviderStore::open(fixture.path(), None).unwrap();
        save(&mut store, draft("Old App"))
    };
    let old_app = app.path().join("providers.json");
    // Same-volume move preserves the exact user-only fixture descriptor.
    std::fs::rename(config_path(fixture.path()), &old_app).unwrap();
    let before = std::fs::read(&old_app).unwrap();
    let acl = sddl(&old_app);
    let server_path = {
        let mut store = ProviderStore::open(server.path(), None).unwrap();
        save(&mut store, draft("Server"));
        store.path.clone()
    };
    {
        let mut store = ProviderStore::open(app.path(), Some(&server_path)).unwrap();
        assert_eq!(store.config.providers[0].metadata.id, id);
        assert_eq!(store.config.providers[0].credential, "fixture-private-key");
        store.delete(&id).unwrap();
    }
    // Even an unsafe retained legacy file is irrelevant once authoritative state exists.
    broaden(&old_app);
    let store = ProviderStore::open(app.path(), Some(&server_path)).unwrap();
    assert!(store.config.providers.is_empty());
    assert_eq!(std::fs::read(&old_app).unwrap(), before);
    assert_ne!(sddl(&old_app), acl);
}

#[test]
fn broad_legacy_acl_fails_before_parsing_without_writing_empty_state() {
    let app = tempfile::tempdir().unwrap();
    let path = app.path().join("providers.json");
    std::fs::write(&path, b"secret-invalid-json").unwrap();
    let acl = sddl(&path);
    for _ in 0..2 {
        let error = ProviderStore::open(app.path(), None).err().unwrap();
        assert!(error.to_string().contains("权限不安全"));
        assert!(!error.to_string().contains("secret"));
        assert!(!config_path(app.path()).exists());
    }
    assert_eq!(sddl(&path), acl);
    assert_eq!(std::fs::read(&path).unwrap(), b"secret-invalid-json");
}

#[test]
fn changed_file_or_directory_acl_rejects_reads_and_updates_without_replacement() {
    for directory_acl in [false, true] {
        let app = tempfile::tempdir().unwrap();
        let mut store = ProviderStore::open(app.path(), None).unwrap();
        save(&mut store, draft("Original"));
        let before = std::fs::read(&store.path).unwrap();
        let target = if directory_acl {
            store.directory.path()
        } else {
            &store.path
        }
        .to_owned();
        broaden(&target);
        let acl = sddl(&target);
        assert!(store.save(draft("Rejected")).is_err());
        assert_eq!(store.config.providers.len(), 1);
        assert_eq!(std::fs::read(&store.path).unwrap(), before);
        assert_eq!(sddl(&target), acl);
        drop(store);
        assert!(ProviderStore::open(app.path(), None).is_err());
        assert_eq!(std::fs::read(config_path(app.path())).unwrap(), before);
    }
}

#[test]
fn junction_app_private_leaf_and_legacy_ancestor_are_rejected() {
    for location in ["app", "private", "legacy"] {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("app");
        std::fs::create_dir(&app).unwrap();
        let fixture = root.path().join("fixture");
        let source = {
            let mut store = ProviderStore::open(&fixture, None).unwrap();
            save(&mut store, draft("Original"));
            store.path.clone()
        };
        let before = std::fs::read(&source).unwrap();
        let link = match location {
            "app" => root.path().join("app-link"),
            "private" => app.join("private-providers"),
            _ => root.path().join("legacy-link"),
        };
        let target = source.parent().unwrap();
        inspect(
            "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:LATTE_PROVIDER_TEST_PATH -Target $env:LATTE_PROVIDER_TEST_TARGET | Out-Null",
            &link,
            target,
        );
        let result = match location {
            "app" => ProviderStore::open(&link, None),
            "private" => ProviderStore::open(&app, None),
            _ => ProviderStore::open(&app, Some(&link.join("providers.json"))),
        };
        assert!(result.is_err(), "accepted {location} junction");
        assert_eq!(std::fs::read(&source).unwrap(), before);
        std::fs::remove_dir(link).unwrap();
    }
}
