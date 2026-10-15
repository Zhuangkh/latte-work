use super::*;
fn repository(path: &Path) {
    std::fs::create_dir(path).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(path)
            .status()
            .unwrap()
            .success()
    );
}
async fn edit_pair(client: &mut Client, root: &Path) -> String {
    std::fs::write(root.join("a.txt"), "before a\n").unwrap();
    std::fs::write(root.join("z.txt"), "before z\n").unwrap();
    let session = create_session(client, root).await;
    let response = ask(
        client,
        json!({"method":"send","session_id":session,"request_id":"pair","text":"edit-pair"}),
    )
    .await;
    assert_eq!(response["kind"], "accepted", "{response}");
    finished(client, &session, "completed").await;
    session
}
fn git(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = std::process::Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
    output.stdout
}
fn git_dir(root: &Path) -> PathBuf {
    PathBuf::from(
        String::from_utf8(git(root, &["rev-parse", "--absolute-git-dir"]))
            .unwrap()
            .trim(),
    )
}
fn pair_index(root: &Path) -> Vec<u8> {
    git(root, &["add", "a.txt", "z.txt"]);
    let index = std::fs::read(git_dir(root).join("index")).unwrap();
    assert!(index.len() > 12 && index.starts_with(b"DIRC"));
    index
}
fn assert_write_denied(error: std::io::Error) {
    // Rust currently classifies Windows ERROR_SHARING_VIOLATION (32) as
    // Uncategorized; replacing an occupied destination can instead deny access.
    assert!(
        error.kind() == std::io::ErrorKind::PermissionDenied || error.raw_os_error() == Some(32),
        "{error}"
    );
}
fn frozen(host: &Host, session: &str, request: &str) -> Value {
    let database = rusqlite::Connection::open(host.state().join("state.sqlite")).unwrap();
    let data: String = database
        .query_row(
            "SELECT data FROM turn_changes WHERE session_id=?1 AND request_id=?2",
            rusqlite::params![session, request],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_str(&data).unwrap()
}
fn identity(path: &Path) -> Value {
    let (volume, file_id) = latte_work_platform::fs::directory_identity(path).unwrap();
    json!({"volume":volume,"file_id":file_id})
}
fn assert_recovery(host: &Host, session: &str, request: &str, project: &Path) -> Value {
    let turn = frozen(host, session, request);
    let recovery = &turn["windows_undo_recovery_v1"];
    let attempt = recovery["attempt_id"].as_str().unwrap();
    uuid::Uuid::parse_str(attempt).unwrap();
    let actual_git = git_dir(project).canonicalize().unwrap();
    let recovery_path = Path::new(recovery["recovery_dir"].as_str().unwrap());
    assert_eq!(
        recovery_path,
        actual_git.join("latte-work-undo").join(attempt)
    );
    assert_eq!(
        Path::new(recovery["project_root"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        project.canonicalize().unwrap()
    );
    assert_eq!(
        Path::new(recovery["git_dir"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        actual_git
    );
    assert_eq!(recovery["project_identity"], identity(project));
    assert_eq!(recovery["git_identity"], identity(&actual_git));
    assert_eq!(recovery["recovery_identity"], identity(recovery_path));
    assert_eq!(
        recovery["project_identity"]["volume"],
        recovery["recovery_identity"]["volume"]
    );
    assert_eq!(
        recovery["entries"].as_array().unwrap().len(),
        turn["changes"]["summary"]["entries"]
            .as_array()
            .unwrap()
            .len()
    );
    recovery.clone()
}
fn assert_original(recovery: &Value, relative: &str, bytes: &[u8]) {
    let entry = recovery["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == relative)
        .unwrap();
    let name = entry["recovery_name"].as_str().unwrap();
    uuid::Uuid::parse_str(name.strip_suffix(".original").unwrap()).unwrap();
    let original = Path::new(recovery["recovery_dir"].as_str().unwrap()).join(name);
    assert_eq!(std::fs::read(&original).unwrap(), bytes);
    let file = std::fs::File::open(original).unwrap();
    let metadata = latte_work_platform::fs::project::project_metadata(&file).unwrap();
    assert_eq!(
        entry["source_identity"],
        json!({"volume":metadata.identity.volume,"file_id":metadata.identity.file_id})
    );
}
async fn barrier_host(variable: &str, barrier_name: &str) -> (Host, PathBuf) {
    let mut host = Host::start().await;
    let mut client = host.client().await;
    host.stop(&mut client).await;
    drop(client);
    let barrier = host.root.path().join(barrier_name);
    std::fs::create_dir(&barrier).unwrap();
    host.process = Host::command(host.root.path(), &host.fixture)
        .env(variable, &barrier)
        .spawn()
        .unwrap();
    host.ready().await;
    (host, barrier)
}
async fn barrier_ready(barrier: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !barrier.join("ready").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
fn pending_undo(
    mut client: Client,
    session: &str,
    request: &str,
) -> tokio::task::JoinHandle<(Client, anyhow::Result<Response>)> {
    let pending = serde_json::from_value(
        json!({"method":"undo_turn_changes","session_id":session,"request_id":request}),
    )
    .unwrap();
    tokio::spawn(async move {
        let result = client.request(pending).await;
        (client, result)
    })
}
#[tokio::test]
async fn windows_undo_validated_guards_reject_competing_writes_and_renames() {
    let (mut host, barrier) = barrier_host("LATTE_WORK_TEST_UNDO_VALIDATED", "validated").await;
    let project = host.root.path().join("project");
    repository(&project);
    let mut client = host.client().await;
    let session = edit_pair(&mut client, &project).await;
    let undo = pending_undo(client, &session, "pair");
    barrier_ready(&barrier).await;
    assert_eq!(
        frozen(&host, &session, "pair")["changes"]["undo"],
        "unknown"
    );
    for name in ["a.txt", "z.txt"] {
        let target = project.join(name);
        let write = std::fs::write(&target, b"external overwrite\0\xff").unwrap_err();
        assert_write_denied(write);
        let rename =
            std::fs::rename(&target, project.join(format!("external-{name}"))).unwrap_err();
        assert_write_denied(rename);
    }
    std::fs::write(project.join("foreign.txt"), b"foreign replacement\0\xff").unwrap();
    let replacement =
        std::fs::rename(project.join("foreign.txt"), project.join("a.txt")).unwrap_err();
    assert_write_denied(replacement);
    assert_eq!(
        std::fs::read(project.join("foreign.txt")).unwrap(),
        b"foreign replacement\0\xff"
    );
    assert_eq!(std::fs::read(project.join("a.txt")).unwrap(), b"after a\n");
    assert_eq!(std::fs::read(project.join("z.txt")).unwrap(), b"after z\n");
    let recovery = assert_recovery(&host, &session, "pair", &project);
    std::fs::write(barrier.join("release"), b"").unwrap();
    let (mut client, result) = undo.await.unwrap();
    let result = serde_json::to_value(result.unwrap()).unwrap();
    assert_eq!(result["changes"]["undo"], "reverted", "{result}");
    assert_eq!(std::fs::read(project.join("a.txt")).unwrap(), b"before a\n");
    assert_eq!(std::fs::read(project.join("z.txt")).unwrap(), b"before z\n");
    assert_original(&recovery, "a.txt", b"after a\n");
    assert_original(&recovery, "z.txt", b"after z\n");
    host.stop(&mut client).await;
}
#[tokio::test]
async fn windows_undo_foreign_creation_after_displacement_survives_and_records_original() {
    let (mut host, barrier) = barrier_host("LATTE_WORK_TEST_UNDO_DISPLACED", "displaced").await;
    let project = host.root.path().join("project");
    repository(&project);
    let mut client = host.client().await;
    let session = edit_pair(&mut client, &project).await;
    let index = pair_index(&project);
    let undo = pending_undo(client, &session, "pair");
    barrier_ready(&barrier).await;
    assert!(!project.join("a.txt").exists());
    assert_eq!(
        frozen(&host, &session, "pair")["changes"]["undo"],
        "unknown"
    );
    let recovery = assert_recovery(&host, &session, "pair", &project);
    assert_original(&recovery, "a.txt", b"after a\n");
    let foreign = b"foreign gap data\0\xff\r\n";
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(project.join("a.txt"))
        .unwrap();
    std::io::Write::write_all(&mut file, foreign).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let foreign_identity = latte_work_platform::fs::project::project_metadata(
        &std::fs::File::open(project.join("a.txt")).unwrap(),
    )
    .unwrap()
    .identity;
    std::fs::write(barrier.join("release"), b"").unwrap();
    let (mut client, result) = undo.await.unwrap();
    assert!(matches!(result.unwrap(), Response::Error { .. }));
    let summary = ask(
        &mut client,
        json!({"method":"turn_change_summary","session_id":session,"request_id":"pair"}),
    )
    .await;
    assert_eq!(summary["changes"]["undo"], "unknown", "{summary}");
    assert_eq!(std::fs::read(project.join("a.txt")).unwrap(), foreign);
    assert_eq!(
        latte_work_platform::fs::project::project_metadata(
            &std::fs::File::open(project.join("a.txt")).unwrap()
        )
        .unwrap()
        .identity,
        foreign_identity
    );
    assert_eq!(std::fs::read(project.join("z.txt")).unwrap(), b"after z\n");
    assert_eq!(
        std::fs::read(git_dir(&project).join("index")).unwrap(),
        index
    );
    assert_original(&recovery, "a.txt", b"after a\n");
    assert_eq!(assert_recovery(&host, &session, "pair", &project), recovery);
    host.stop(&mut client).await;
    drop(client);
    host.process = Host::spawn(host.root.path(), &host.fixture);
    host.ready().await;
    let mut client = host.client().await;
    assert_eq!(
        ask(
            &mut client,
            json!({"method":"undo_turn_changes","session_id":session,"request_id":"pair"})
        )
        .await["kind"],
        "error"
    );
    assert_eq!(std::fs::read(project.join("a.txt")).unwrap(), foreign);
    assert_eq!(
        std::fs::read(git_dir(&project).join("index")).unwrap(),
        index
    );
    assert_original(&recovery, "a.txt", b"after a\n");
    assert_eq!(
        std::fs::read_to_string(host.root.path().join("prompts.log")).unwrap(),
        "edit-pair\n"
    );
    host.stop(&mut client).await;
}
#[tokio::test]
async fn windows_undo_crash_after_displacement_keeps_durable_unknown_and_original() {
    let (mut host, barrier) =
        barrier_host("LATTE_WORK_TEST_UNDO_DISPLACED", "crash-displaced").await;
    let project = host.root.path().join("project");
    repository(&project);
    let mut client = host.client().await;
    let session = edit_pair(&mut client, &project).await;
    let index = pair_index(&project);
    let undo = pending_undo(client, &session, "pair");
    barrier_ready(&barrier).await;
    assert_eq!(
        frozen(&host, &session, "pair")["changes"]["undo"],
        "unknown"
    );
    let recovery = assert_recovery(&host, &session, "pair", &project);
    assert_original(&recovery, "a.txt", b"after a\n");
    assert!(!project.join("a.txt").exists());
    host.process.kill().await.unwrap();
    let (client, result) = undo.await.unwrap();
    assert!(result.is_err());
    drop(client);
    host.process = Host::spawn(host.root.path(), &host.fixture);
    host.ready().await;
    let mut client = host.client().await;
    let summary = ask(
        &mut client,
        json!({"method":"turn_change_summary","session_id":session,"request_id":"pair"}),
    )
    .await;
    assert_eq!(summary["changes"]["undo"], "unknown", "{summary}");
    assert_eq!(
        ask(
            &mut client,
            json!({"method":"undo_turn_changes","session_id":session,"request_id":"pair"})
        )
        .await["kind"],
        "error"
    );
    assert!(!project.join("a.txt").exists());
    assert_eq!(std::fs::read(project.join("z.txt")).unwrap(), b"after z\n");
    assert_eq!(
        std::fs::read(git_dir(&project).join("index")).unwrap(),
        index
    );
    assert_eq!(assert_recovery(&host, &session, "pair", &project), recovery);
    assert_original(&recovery, "a.txt", b"after a\n");
    assert_eq!(
        std::fs::read_to_string(host.root.path().join("prompts.log")).unwrap(),
        "edit-pair\n"
    );
    host.stop(&mut client).await;
}
#[tokio::test]
async fn windows_undo_success_preserves_full_index_and_hides_recovery_in_regular_and_linked_worktrees()
 {
    for linked in [false, true] {
        let mut host = Host::start().await;
        let main = host.root.path().join("main");
        repository(&main);
        std::fs::write(main.join("seed.txt"), b"committed seed\n").unwrap();
        git(&main, &["add", "seed.txt"]);
        git(
            &main,
            &[
                "-c",
                "user.name=Native fixture",
                "-c",
                "user.email=native@example.invalid",
                "commit",
                "-qm",
                "seed",
            ],
        );
        let project = if linked {
            let project = host.root.path().join("linked project");
            git(
                &main,
                &[
                    "worktree",
                    "add",
                    "-qb",
                    "undo-test",
                    project.to_str().unwrap(),
                ],
            );
            assert!(project.join(".git").is_file());
            project
        } else {
            main.clone()
        };
        let mut client = host.client().await;
        let session = edit_pair(&mut client, &project).await;
        // Include staged content and a real index flag, rather than an absent or
        // empty index. Undo must preserve the complete index, not only ls-files.
        std::fs::write(project.join("staged.txt"), b"independently staged\n").unwrap();
        git(&project, &["add", "a.txt", "z.txt", "staged.txt"]);
        git(&project, &["update-index", "--assume-unchanged", "a.txt"]);
        let index_path = git_dir(&project).join("index");
        let index = std::fs::read(&index_path).unwrap();
        assert!(index.len() > 12 && index.starts_with(b"DIRC"));
        assert!(
            String::from_utf8(git(&project, &["ls-files", "-v", "a.txt"]))
                .unwrap()
                .starts_with("h ")
        );
        let undo = ask(
            &mut client,
            json!({"method":"undo_turn_changes","session_id":session,"request_id":"pair"}),
        )
        .await;
        assert_eq!(undo["changes"]["undo"], "reverted", "{undo}");
        assert_eq!(std::fs::read(project.join("a.txt")).unwrap(), b"before a\n");
        assert_eq!(std::fs::read(project.join("z.txt")).unwrap(), b"before z\n");
        assert_eq!(std::fs::read(&index_path).unwrap(), index);
        let recovery = assert_recovery(&host, &session, "pair", &project);
        assert_original(&recovery, "a.txt", b"after a\n");
        assert_original(&recovery, "z.txt", b"after z\n");
        if linked {
            assert!(
                git_dir(&project)
                    .canonicalize()
                    .unwrap()
                    .starts_with(main.join(".git").join("worktrees").canonicalize().unwrap())
            );
        }
        let status = String::from_utf8(git(
            &project,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        ))
        .unwrap();
        assert!(!status.contains("latte-work-undo"), "{status}");
        assert!(!status.contains(".latte-undo-"), "{status}");
        let send = ask(&mut client, json!({"method":"send","session_id":session,"request_id":"next-snapshot","text":"wait"})).await;
        assert_eq!(send["kind"], "accepted", "{send}");
        finished(&mut client, &session, "running").await;
        // FrozenTurn retains changed bytes only. Inspect the actual pending
        // snapshot before cancellation compacts its unchanged file inventory.
        let baseline: Value = {
            let database = rusqlite::Connection::open(host.state().join("state.sqlite")).unwrap();
            let data: String = database.query_row(
                "SELECT baseline FROM turn_changes WHERE session_id=?1 AND request_id='next-snapshot'",
                [&session], |row| row.get(0)).unwrap();
            serde_json::from_str(&data).unwrap()
        };
        let files = baseline["files"].as_object().unwrap();
        assert_eq!(files.len(), 4, "{baseline}");
        for name in ["a.txt", "z.txt", "seed.txt", "staged.txt"] {
            assert!(files.contains_key(name), "{baseline}");
        }
        ask(&mut client, json!({"method":"cancel","session_id":session})).await;
        finished(&mut client, &session, "stopped").await;
        let next = frozen(&host, &session, "next-snapshot");
        assert!(
            next["changes"]["summary"]["entries"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert_eq!(std::fs::read(&index_path).unwrap(), index);
        assert_original(&recovery, "a.txt", b"after a\n");
        assert_original(&recovery, "z.txt", b"after z\n");
        host.stop(&mut client).await;
    }
}
#[tokio::test]
async fn windows_undo_restores_missing_beforeimage_and_retains_removed_turn_added_file() {
    let mut host = Host::start().await;
    let project = host.root.path().join("project");
    repository(&project);
    std::fs::write(project.join("a.txt"), b"before existing\n").unwrap();
    std::fs::write(project.join("m.txt"), b"before missing\n").unwrap();
    git(&project, &["add", "a.txt", "m.txt"]);
    let index = std::fs::read(git_dir(&project).join("index")).unwrap();
    let mut client = host.client().await;
    let session = create_session(&mut client, &project).await;
    assert_eq!(
        ask(
            &mut client,
            json!({"method":"send","session_id":session,"request_id":"mixed","text":"wait"})
        )
        .await["kind"],
        "accepted"
    );
    finished(&mut client, &session, "running").await;
    // The waiting native fixture gives this turn a deterministic mutation window.
    std::fs::write(project.join("a.txt"), b"after existing\n").unwrap();
    std::fs::remove_file(project.join("m.txt")).unwrap();
    std::fs::write(project.join("z.txt"), b"turn-added\0\xff").unwrap();
    assert_eq!(
        ask(&mut client, json!({"method":"cancel","session_id":session})).await["kind"],
        "ok"
    );
    finished(&mut client, &session, "stopped").await;
    let summary = ask(
        &mut client,
        json!({"method":"turn_change_summary","session_id":session,"request_id":"mixed"}),
    )
    .await;
    assert_eq!(
        summary["changes"]["summary"]["entries"]
            .as_array()
            .unwrap()
            .len(),
        3,
        "{summary}"
    );
    let result = ask(
        &mut client,
        json!({"method":"undo_turn_changes","session_id":session,"request_id":"mixed"}),
    )
    .await;
    assert_eq!(result["changes"]["undo"], "reverted", "{result}");
    assert_eq!(
        std::fs::read(project.join("a.txt")).unwrap(),
        b"before existing\n"
    );
    assert_eq!(
        std::fs::read(project.join("m.txt")).unwrap(),
        b"before missing\n"
    );
    assert!(!project.join("z.txt").exists());
    assert_eq!(
        std::fs::read(git_dir(&project).join("index")).unwrap(),
        index
    );
    let recovery = assert_recovery(&host, &session, "mixed", &project);
    assert_original(&recovery, "a.txt", b"after existing\n");
    assert_original(&recovery, "z.txt", b"turn-added\0\xff");
    let entries = recovery["entries"].as_array().unwrap();
    for name in ["a.txt", "m.txt"] {
        assert_eq!(
            entries.iter().find(|entry| entry["path"] == name).unwrap()["operation"],
            "restore_beforeimage"
        );
    }
    let missing = entries
        .iter()
        .find(|entry| entry["path"] == "m.txt")
        .unwrap();
    assert!(missing["source_identity"].is_null());
    assert!(missing["recovery_name"].is_null());
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry["path"] == "z.txt")
            .unwrap()["operation"],
        "remove_turn_added"
    );
    host.stop(&mut client).await;
}
#[tokio::test]
async fn windows_undo_missing_target_created_after_validation_is_never_replaced() {
    let (mut host, barrier) =
        barrier_host("LATTE_WORK_TEST_UNDO_VALIDATED", "missing-validated").await;
    let project = host.root.path().join("project");
    repository(&project);
    std::fs::write(project.join("missing.txt"), b"before missing\n").unwrap();
    git(&project, &["add", "missing.txt"]);
    let index = std::fs::read(git_dir(&project).join("index")).unwrap();
    let mut client = host.client().await;
    let session = create_session(&mut client, &project).await;
    assert_eq!(
        ask(
            &mut client,
            json!({"method":"send","session_id":session,"request_id":"missing","text":"wait"})
        )
        .await["kind"],
        "accepted"
    );
    finished(&mut client, &session, "running").await;
    std::fs::remove_file(project.join("missing.txt")).unwrap();
    ask(&mut client, json!({"method":"cancel","session_id":session})).await;
    finished(&mut client, &session, "stopped").await;
    let undo = pending_undo(client, &session, "missing");
    barrier_ready(&barrier).await;
    assert_eq!(
        frozen(&host, &session, "missing")["changes"]["undo"],
        "unknown"
    );
    let recovery = assert_recovery(&host, &session, "missing", &project);
    let entry = &recovery["entries"][0];
    assert_eq!(entry["path"], "missing.txt");
    assert_eq!(entry["operation"], "restore_beforeimage");
    assert!(entry["source_identity"].is_null());
    assert!(entry["recovery_name"].is_null());
    let foreign = b"foreign formerly missing\0\xff";
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(project.join("missing.txt"))
        .unwrap();
    std::io::Write::write_all(&mut file, foreign).unwrap();
    file.sync_all().unwrap();
    drop(file);
    std::fs::write(barrier.join("release"), b"").unwrap();
    let (mut client, result) = undo.await.unwrap();
    assert!(matches!(result.unwrap(), Response::Error { .. }));
    assert_eq!(std::fs::read(project.join("missing.txt")).unwrap(), foreign);
    assert_eq!(
        std::fs::read(git_dir(&project).join("index")).unwrap(),
        index
    );
    assert_eq!(
        frozen(&host, &session, "missing")["changes"]["undo"],
        "unknown"
    );
    assert_eq!(
        assert_recovery(&host, &session, "missing", &project),
        recovery
    );
    host.stop(&mut client).await;
}
#[tokio::test]
async fn windows_undo_partial_failure_and_crash_persist_unknown_without_replay() {
    for crash in [false, true] {
        let mut host = Host::start().await;
        let mut client = host.client().await;
        host.stop(&mut client).await;
        drop(client);
        let barrier = host.root.path().join("undo-barrier");
        std::fs::create_dir(&barrier).unwrap();
        if !crash {
            std::fs::write(barrier.join("fail"), b"").unwrap();
        }
        host.process = Host::command(host.root.path(), &host.fixture)
            .env("LATTE_WORK_TEST_UNDO_BARRIER", &barrier)
            .spawn()
            .unwrap();
        host.ready().await;
        let mut client = host.client().await;
        let project = host.root.path().join("project");
        repository(&project);
        let session = edit_pair(&mut client, &project).await;
        let index = pair_index(&project);
        let request =
            json!({"method":"undo_turn_changes","session_id":session,"request_id":"pair"});
        let pending: Request = serde_json::from_value(request.clone()).unwrap();
        let undo = tokio::spawn(async move {
            let result = client.request(pending).await;
            (client, result)
        });
        tokio::time::timeout(Duration::from_secs(10), async {
            while !barrier.join("ready").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        if crash {
            host.process.kill().await.unwrap();
        }
        let (mut client, result) = undo.await.unwrap();
        if crash {
            assert!(result.is_err());
        } else {
            assert!(matches!(result.unwrap(), Response::Error { .. }));
            host.stop(&mut client).await;
        }
        drop(client);
        assert_eq!(
            std::fs::read_to_string(project.join("a.txt")).unwrap(),
            "before a\n"
        );
        assert_eq!(
            std::fs::read_to_string(project.join("z.txt")).unwrap(),
            "after z\n"
        );
        let recovery = assert_recovery(&host, &session, "pair", &project);
        assert_original(&recovery, "a.txt", b"after a\n");
        assert_eq!(
            std::fs::read(git_dir(&project).join("index")).unwrap(),
            index
        );
        // Restart without the fixture fault. Recovery must not continue restoration.
        host.process = Host::spawn(host.root.path(), &host.fixture);
        host.ready().await;
        let mut client = host.client().await;
        let stored = ask(
            &mut client,
            json!({"method":"turn_change_summary","session_id":session,"request_id":"pair"}),
        )
        .await;
        assert_eq!(stored["changes"]["undo"], "unknown", "{stored}");
        assert_eq!(ask(&mut client, request).await["kind"], "error");
        assert_eq!(assert_recovery(&host, &session, "pair", &project), recovery);
        assert_original(&recovery, "a.txt", b"after a\n");
        assert_eq!(
            std::fs::read(git_dir(&project).join("index")).unwrap(),
            index
        );
        assert_eq!(
            std::fs::read_to_string(project.join("a.txt")).unwrap(),
            "before a\n"
        );
        assert_eq!(
            std::fs::read_to_string(project.join("z.txt")).unwrap(),
            "after z\n"
        );
        assert_eq!(
            std::fs::read_to_string(host.root.path().join("prompts.log")).unwrap(),
            "edit-pair\n"
        );
        host.stop(&mut client).await;
    }
}
#[tokio::test]
async fn windows_legacy_turn_is_readable_but_history_and_summary_explain_undo_refusal() {
    let mut host = Host::start().await;
    let mut client = host.client().await;
    let project = host.root.path().join("project");
    repository(&project);
    let session = edit_pair(&mut client, &project).await;
    host.stop(&mut client).await;
    drop(client);
    {
        let database = rusqlite::Connection::open(host.state().join("state.sqlite")).unwrap();
        database.execute("UPDATE turn_changes SET data=json_remove(data,'$.before.windows_files_v1','$.after.windows_files_v1') WHERE session_id=?1", [&session]).unwrap();
    }
    host.process = Host::spawn(host.root.path(), &host.fixture);
    host.ready().await;
    let mut client = host.client().await;
    let summary = ask(
        &mut client,
        json!({"method":"turn_change_summary","session_id":session,"request_id":"pair"}),
    )
    .await;
    assert!(
        summary["changes"]["undo_unavailable"]
            .as_str()
            .unwrap()
            .contains("Windows")
    );
    let history = ask(
        &mut client,
        json!({"method":"poll","session_id":session,"after":0}),
    )
    .await;
    assert!(
        history["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["event"]["kind"] == "turn_changes"
                && e["event"]["changes"]["undo_unavailable"].is_string())
    );
    let patch = ask(&mut client, json!({"method":"turn_change_diff","session_id":session,"request_id":"pair","path":"a.txt"})).await;
    assert!(patch["text"].as_str().unwrap().contains("+after a"));
    let refused = ask(
        &mut client,
        json!({"method":"undo_turn_changes","session_id":session,"request_id":"pair"}),
    )
    .await;
    assert_eq!(refused["kind"], "error");
    assert_eq!(
        std::fs::read_to_string(project.join("a.txt")).unwrap(),
        "after a\n"
    );
    host.stop(&mut client).await;
}
