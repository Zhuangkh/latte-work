use super::*;
use latte_work_protocol::{ProviderAuth, ProviderProtocol};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
fn config_path(directory: &Path) -> PathBuf {
    #[cfg(windows)]
    let directory = directory.join("private-providers");
    directory.join("providers.json")
}
fn draft(name: &str) -> ProviderDraft {
    ProviderDraft {
        id: None,
        name: name.into(),
        protocol: ProviderProtocol::AnthropicMessages,
        base_url: "https://example.test".into(),
        model: "first".into(),
        models: vec![],
        model_labels: Default::default(),
        auth: ProviderAuth::ApiKey,
        credential: Some("fixture-private-key".into()),
    }
}
fn save(store: &mut ProviderStore, draft: ProviderDraft) -> String {
    let Response::Providers { providers, .. } = store.save(draft).unwrap() else {
        panic!()
    };
    providers.last().unwrap().id.clone()
}
#[test]
fn offline_catalog_and_host_choices_survive_restart_without_credentials_in_responses() {
    let directory = tempfile::tempdir().unwrap();
    let id;
    {
        let mut store = ProviderStore::open(directory.path(), None).unwrap();
        id = save(&mut store, draft("Team"));
        // App configuration is independent even of future/unknown Agent registries.
        store
            .bind("devbox", "future-agent", Some(id.clone()))
            .unwrap();
        store.bind("local", "claude", Some(id.clone())).unwrap();
        assert!(
            store
                .snapshot_for_host("other", "claude")
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .snapshot_for_host("devbox", "claude")
                .unwrap()
                .is_none()
        );
        assert!(
            !serde_json::to_string(&store.list())
                .unwrap()
                .contains("fixture-private-key")
        );
        let before = std::fs::read(config_path(directory.path())).unwrap();
        let mut edit = draft("Changed");
        edit.id = Some(id.clone());
        edit.protocol = ProviderProtocol::OpenaiChat;
        assert!(store.save(edit).is_err());
        assert_eq!(
            before,
            std::fs::read(config_path(directory.path())).unwrap()
        );
    }
    let mut store = ProviderStore::open(directory.path(), None).unwrap();
    let mut edit = draft("Changed");
    edit.id = Some(id.clone());
    edit.model = "second".into();
    edit.credential = None;
    store.save(edit).unwrap();
    let remote = store
        .snapshot_for_host("devbox", "future-agent")
        .unwrap()
        .unwrap();
    assert_eq!(remote.credential, "fixture-private-key");
    assert_eq!(remote.provider.model, "second");
    store.bind("local", "claude", None).unwrap();
    assert!(
        store
            .snapshot_for_host("local", "claude")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .snapshot_for_host("devbox", "future-agent")
            .unwrap()
            .is_some()
    );
    store.forget_host("devbox").unwrap();
    store.delete(&id).unwrap();
    // CRUD supports all protocols with no installed/running Agent.
    for protocol in [
        ProviderProtocol::AnthropicMessages,
        ProviderProtocol::OpenaiChat,
        ProviderProtocol::OpenaiResponses,
    ] {
        let mut value = draft("Unbound");
        value.protocol = protocol;
        save(&mut store, value);
    }
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(config_path(directory.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
#[test]
fn migration_is_read_only_once_and_preserves_local_remote_associations() {
    for schema in [1, 2] {
        let legacy = tempfile::tempdir().unwrap();
        let app = tempfile::tempdir().unwrap();
        let id;
        {
            let mut old = ProviderStore::open(legacy.path(), None).unwrap();
            id = save(&mut old, draft("Legacy"));
            old.bind("local", "claude", Some(id.clone())).unwrap();
            old.bind("devbox", "claude", Some(id.clone())).unwrap();
        }
        let path = config_path(legacy.path());
        if schema == 1 {
            let mut value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            value["schema"] = 1.into();
            value["active_id"] = id.clone().into();
            value["bindings"] = serde_json::json!({});
            value["providers"][0]["metadata"]["revision"] = "".into();
            std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        {
            let store = ProviderStore::open(app.path(), Some(&path)).unwrap();
            for host in ["local", "devbox"] {
                let snapshot = store.snapshot_for_host(host, "claude").unwrap().unwrap();
                assert_eq!(snapshot.provider.id, id);
                assert_eq!(snapshot.credential, "fixture-private-key");
                assert!(!snapshot.provider.revision.is_empty());
            }
        }
        assert_eq!(before, std::fs::read(&path).unwrap());
        std::fs::write(&path, "invalid now").unwrap();
        let store = ProviderStore::open(app.path(), Some(&path)).unwrap();
        assert!(
            store
                .snapshot_for_host("devbox", "claude")
                .unwrap()
                .is_some()
        );
    }
}
#[test]
fn schema_one_incompatible_or_missing_selection_keeps_native_defaults() {
    for protocol in [
        ProviderProtocol::OpenaiChat,
        ProviderProtocol::OpenaiResponses,
    ] {
        for missing in [false, true] {
            let legacy = tempfile::tempdir().unwrap();
            let app = tempfile::tempdir().unwrap();
            let id = {
                let mut old = ProviderStore::open(legacy.path(), None).unwrap();
                let mut provider = draft("Legacy OpenAI");
                provider.protocol = protocol.clone();
                save(&mut old, provider)
            };
            let path = config_path(legacy.path());
            let mut value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            value["schema"] = 1.into();
            value["active_id"] = if missing { "missing" } else { &id }.into();
            let before = serde_json::to_vec(&value).unwrap();
            std::fs::write(&path, &before).unwrap();
            for _ in 0..2 {
                let store = ProviderStore::open(app.path(), Some(&path)).unwrap();
                for host in ["local", "devbox"] {
                    assert!(store.snapshot_for_host(host, "claude").unwrap().is_none());
                }
                assert!(
                    matches!(store.list(), Response::Providers { providers, bindings }
                    if providers.len() == 1 && providers[0].id == id && bindings.is_empty())
                );
                assert_eq!(store.config.providers[0].credential, "fixture-private-key");
            }
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
    }
}
#[test]
fn invalid_or_oversized_migration_cannot_replace_configuration() {
    let legacy = tempfile::tempdir().unwrap();
    let app = tempfile::tempdir().unwrap();
    let path = {
        let store = ProviderStore::open(legacy.path(), None).unwrap();
        store.path.clone()
    };
    for bytes in [
        b"{secret-invalid-json".to_vec(),
        vec![b' '; MAX_CONFIG as usize + 1],
    ] {
        std::fs::write(&path, &bytes).unwrap();
        let error = ProviderStore::open(app.path(), Some(&path))
            .err()
            .unwrap()
            .to_string();
        assert!(!error.contains("secret"));
        assert!(!config_path(app.path()).exists());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
#[test]
fn concurrent_app_transactions_reload_before_writing() {
    let directory = tempfile::tempdir().unwrap();
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let path = directory.path().to_owned();
            std::thread::spawn(move || {
                let mut store = ProviderStore::open(&path, None).unwrap();
                save(&mut store, draft(&format!("Provider {i}")));
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let store = ProviderStore::open(directory.path(), None).unwrap();
    assert!(matches!(store.list(), Response::Providers { providers, .. } if providers.len() == 8));
}
#[cfg(unix)]
#[test]
fn symlinked_app_or_legacy_config_is_rejected() {
    let target = tempfile::tempdir().unwrap();
    let app = tempfile::tempdir().unwrap();
    let private = target.path().join("private");
    std::fs::write(&private, "unchanged").unwrap();
    let link = config_path(app.path());
    std::os::unix::fs::symlink(&private, &link).unwrap();
    assert!(ProviderStore::open(app.path(), None).is_err());
    assert_eq!(std::fs::read_to_string(private).unwrap(), "unchanged");
}

#[test]
fn no_auth_clears_credentials_and_survives_reload() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ProviderStore::open(directory.path(), None).unwrap();
    let id = save(&mut store, draft("Team"));
    let mut edit = draft("Team");
    edit.id = Some(id);
    edit.auth = ProviderAuth::None;
    edit.credential = None;
    let response = store.save(edit).unwrap();
    let Response::Providers { providers, .. } = response else {
        panic!()
    };
    assert!(!providers[0].has_credential);
    assert_eq!(providers[0].auth, ProviderAuth::None);
    let contents = std::fs::read_to_string(config_path(directory.path())).unwrap();
    assert!(!contents.contains("fixture-private-key"));
    drop(store);
    assert!(ProviderStore::open(directory.path(), None).is_ok());
}

#[test]
fn model_labels_survive_reload_and_discovery_cannot_reuse_secret_at_new_url() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ProviderStore::open(directory.path(), None).unwrap();
    let mut value = draft("Named");
    value.model_labels.insert("first".into(), "Friendly".into());
    let id = save(&mut store, value.clone());
    drop(store);
    let store = ProviderStore::open(directory.path(), None).unwrap();
    let Response::Providers { providers, .. } = store.list() else {
        panic!()
    };
    assert_eq!(providers[0].model_labels["first"], "Friendly");
    value.id = Some(id);
    value.credential = None;
    assert!(store.model_endpoint(value.clone()).is_ok());
    value.base_url = "https://other.test".into();
    assert!(store.model_endpoint(value).is_err());
}

#[test]
fn deleting_provider_removes_only_its_bindings_and_persists() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ProviderStore::open(directory.path(), None).unwrap();
    let removed = save(&mut store, draft("Remove"));
    let kept = save(&mut store, draft("Keep"));
    store
        .bind("local", "claude", Some(removed.clone()))
        .unwrap();
    store
        .bind("devbox", "claude", Some(removed.clone()))
        .unwrap();
    store.bind("devbox", "other", Some(kept.clone())).unwrap();
    store.delete(&removed).unwrap();
    drop(store);
    let store = ProviderStore::open(directory.path(), None).unwrap();
    assert!(
        store
            .snapshot_for_host("local", "claude")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .snapshot_for_host("devbox", "claude")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .snapshot_for_host("devbox", "other")
            .unwrap()
            .unwrap()
            .provider
            .id,
        kept
    );
    assert!(
        !std::fs::read_to_string(config_path(directory.path()))
            .unwrap()
            .contains(&removed)
    );
}

#[cfg(windows)]
#[path = "tests/windows.rs"]
mod windows;

#[cfg(unix)]
#[test]
fn unix_existing_permissions_and_atomic_replacements_keep_the_original_contract() {
    let app = tempfile::tempdir().unwrap();
    let path = config_path(app.path());
    {
        let mut store = ProviderStore::open(app.path(), None).unwrap();
        save(&mut store, draft("Original"));
    }
    std::fs::set_permissions(app.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let mut store = ProviderStore::open(app.path(), None).unwrap();
    assert_eq!(
        std::fs::metadata(app.path()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(store.config.providers[0].credential, "fixture-private-key");
    save(&mut store, draft("Replacement"));
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let temp = store.directory.tempfile("permission-", ".json").unwrap();
    assert_eq!(
        temp.as_file().metadata().unwrap().permissions().mode() & 0o777,
        0o600
    );
}
