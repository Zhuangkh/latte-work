//! Final Server binary with a native deterministic Agent; works without a shell script.
#![cfg(feature = "test-fixture")]
use latte_work_client::Client;
use latte_work_platform::ipc::{self, Endpoint};
use latte_work_protocol::{Request, Response, VERSION};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::{Child, Command};
const SERVER: &str = env!("CARGO_BIN_EXE_latte-work-server");
#[path = "native/owned_resources.rs"]
mod owned_resources;
#[path = "native/ssh.rs"]
mod ssh;
#[cfg(windows)]
#[path = "native/terminal.rs"]
mod terminal;
#[cfg(windows)]
#[path = "native/undo.rs"]
mod undo;
struct Host {
    root: tempfile::TempDir,
    fixture: PathBuf,
    process: Child,
}
impl Host {
    fn spawn(root: &Path, fixture: &Path) -> Child {
        Self::command(root, fixture).spawn().unwrap()
    }
    fn command(root: &Path, fixture: &Path) -> Command {
        let mut command = Command::new(SERVER);
        command
            .args(["serve", "--state-dir"])
            .arg(root.join("state"))
            .env("LATTE_WORK_CLAUDE", fixture)
            .env("LATTE_WORK_AGENT_ENV", "inherit")
            .env("LATTE_WORK_TEST_TERMINAL_NO_PROFILE", "1")
            .env("LATTE_NATIVE_PROMPT_LOG", root.join("prompts.log"))
            .env("LATTE_NATIVE_ACTION_LOG", root.join("actions.log"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .kill_on_drop(true);
        command
    }
    async fn start() -> Self {
        #[cfg(unix)]
        let root = tempfile::Builder::new()
            .prefix("lwn-")
            .tempdir_in("/tmp")
            .unwrap();
        #[cfg(windows)]
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("工具 bin");
        std::fs::create_dir(&bin).unwrap();
        let fixture = bin.join("native-agent.exe");
        std::fs::copy(env!("CARGO_BIN_EXE_native-claude-fixture"), &fixture).unwrap();
        let process = Self::spawn(root.path(), &fixture);
        let mut host = Self {
            root,
            fixture,
            process,
        };
        host.ready().await;
        host
    }
    async fn ready(&mut self) {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                assert!(
                    self.process.try_wait().unwrap().is_none(),
                    "server exited before readiness"
                );
                match Endpoint::open(&self.state()) {
                    Ok(endpoint) => match endpoint.connect(Duration::from_secs(1)).await {
                        Ok(_) => break,
                        Err(error) if ipc::daemon_absent(&error) => {}
                        Err(error) => panic!("{error}"),
                    },
                    Err(error) if ipc::daemon_absent(&error) => {}
                    Err(error) => panic!("{error}"),
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }
    fn state(&self) -> PathBuf {
        self.root.path().join("state")
    }
    async fn client(&self) -> Client {
        Client::local(Path::new(SERVER), Some(&self.state()))
            .await
            .unwrap()
    }
    async fn stop(&mut self, client: &mut Client) {
        assert!(client.shutdown_if_idle().await.unwrap());
        assert!(
            tokio::time::timeout(Duration::from_secs(10), self.process.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
        let endpoint = Endpoint::open(&self.state()).unwrap();
        assert!(endpoint.try_lock_server().is_ok());
    }
}
async fn ask(client: &mut Client, value: Value) -> Value {
    let request: Request = serde_json::from_value(value).unwrap();
    serde_json::to_value(client.request(request).await.unwrap()).unwrap()
}
async fn finished(client: &mut Client, session: &str, status: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let result = ask(
                client,
                json!({"method":"poll", "session_id":session,"after":0}),
            )
            .await;
            if result["session"]["status"] == status {
                return result;
            }
            assert_ne!(result["session"]["status"], "failed", "{result}");
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap()
}

async fn create_session(client: &mut Client, project: &Path) -> String {
    let added = ask(
        client,
        json!({"method":"add_project","path":project,"name":null}),
    )
    .await;
    let created = ask(
        client,
        json!({"method":"create_session",
        "project_id":added["project"]["id"],"agent":"claude"}),
    )
    .await;
    created["session"]["id"].as_str().unwrap().to_owned()
}

fn pending_approval(state: &Value) -> String {
    state["events"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|event| event["event"]["kind"] == "approval")
        .unwrap()["event"]["request_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn native_approval_is_explicit_session_bound_and_single_use() {
    let mut host = Host::start().await;
    let mut client = host.client().await;
    let first = create_session(&mut client, host.root.path()).await;
    let second = create_session(&mut client, host.root.path()).await;
    for (session, request) in [(&first, "first"), (&second, "second")] {
        assert_eq!(
            ask(
                &mut client,
                json!({"method":"send","session_id":session,
            "request_id":request,"text":"approve"})
            )
            .await,
            json!({"kind":"accepted","duplicate":false})
        );
    }
    let first_approval = pending_approval(&finished(&mut client, &first, "waiting").await);
    let second_approval = pending_approval(&finished(&mut client, &second, "waiting").await);
    assert_ne!(first_approval, second_approval);
    for (session, request) in [
        (&first, "wrong"),
        (&second, first_approval.as_str()),
        (&first, second_approval.as_str()),
    ] {
        let rejected = ask(
            &mut client,
            json!({"method":"approve","session_id":session,
            "request_id":request,"allow":true}),
        )
        .await;
        assert_eq!(rejected["kind"], "error", "{rejected}");
    }
    assert!(!host.root.path().join("actions.log").exists());
    assert!(!host.root.path().join("approved.txt").exists());
    assert_eq!(
        ask(
            &mut client,
            json!({"method":"approve","session_id":first,
        "request_id":first_approval,"allow":true})
        )
        .await["kind"],
        "ok"
    );
    finished(&mut client, &first, "completed").await;
    assert_eq!(
        std::fs::read_to_string(host.root.path().join("approved.txt")).unwrap(),
        "approved"
    );
    let actions = std::fs::read_to_string(host.root.path().join("actions.log")).unwrap();
    assert_eq!(actions.lines().count(), 1);
    assert_eq!(
        ask(
            &mut client,
            json!({"method":"approve","session_id":first,
        "request_id":first_approval,"allow":true})
        )
        .await["kind"],
        "error"
    );
    assert_eq!(
        ask(
            &mut client,
            json!({"method":"approve","session_id":second,
        "request_id":second_approval,"allow":false})
        )
        .await["kind"],
        "ok"
    );
    finished(&mut client, &second, "completed").await;
    ask(
        &mut client,
        json!({"method":"send","session_id":first,
        "request_id":"next-turn","text":"approve"}),
    )
    .await;
    let next_approval = pending_approval(&finished(&mut client, &first, "waiting").await);
    assert_ne!(next_approval, first_approval);
    assert_eq!(
        ask(
            &mut client,
            json!({"method":"approve","session_id":first,
        "request_id":first_approval,"allow":true})
        )
        .await["kind"],
        "error"
    );
    assert_eq!(
        std::fs::read_to_string(host.root.path().join("actions.log")).unwrap(),
        actions
    );
    assert_eq!(
        ask(
            &mut client,
            json!({"method":"approve","session_id":first,
        "request_id":next_approval,"allow":false})
        )
        .await["kind"],
        "ok"
    );
    finished(&mut client, &first, "completed").await;
    assert_eq!(
        std::fs::read_to_string(host.root.path().join("actions.log")).unwrap(),
        actions
    );
    host.stop(&mut client).await;
}

#[cfg(windows)]
#[tokio::test]
async fn native_daemon_crash_preserves_unknown_and_reconciles_without_replay() {
    let mut host = Host::start().await;
    let mut client = host.client().await;
    let project = host.root.path().join("crash project");
    std::fs::create_dir(&project).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&project)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(project.join("task.txt"), "before\n").unwrap();
    let session = create_session(&mut client, &project).await;
    let send = json!({"method":"send","session_id":session,
        "request_id":"crash-request","text":"wait"});
    assert_eq!(
        ask(&mut client, send.clone()).await,
        json!({"kind":"accepted","duplicate":false})
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if std::fs::read_to_string(host.root.path().join("prompts.log"))
                .is_ok_and(|log| log == "wait\n")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    finished(&mut client, &session, "running").await;
    let old_server = client.server_id().to_owned();
    tokio::time::timeout(Duration::from_secs(3), host.process.kill())
        .await
        .unwrap()
        .unwrap();
    assert!(
        client
            .request(Request::Hello { version: VERSION })
            .await
            .is_err()
    );
    drop(client);
    host.process = Host::spawn(host.root.path(), &host.fixture);
    host.ready().await;
    let mut client = host.client().await;
    assert_ne!(client.server_id(), old_server);
    let history = ask(
        &mut client,
        json!({"method":"poll","session_id":session,"after":0}),
    )
    .await;
    assert_eq!(history["session"]["status"], "unknown", "{history}");
    assert_eq!(
        history["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|event| event["event"]["kind"] == "user"
                && event["event"]["request_id"] == "crash-request")
            .count(),
        1
    );
    let changes = ask(
        &mut client,
        json!({"method":"turn_change_summary","session_id":session,
        "request_id":"crash-request"}),
    )
    .await;
    assert_eq!(changes["kind"], "error", "{changes}");
    let interrupted = history["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| &event["event"])
        .find(|event| {
            event["kind"] == "turn_changes" && event["changes"]["request_id"] == "crash-request"
        })
        .unwrap();
    assert_eq!(interrupted["changes"]["interrupted"], true, "{interrupted}");
    assert_eq!(interrupted["changes"]["undo"], "unknown", "{interrupted}");
    assert!(interrupted["changes"]["summary"]["unavailable"].is_string());
    assert_eq!(
        ask(&mut client, send).await,
        json!({"kind":"accepted","duplicate":true})
    );
    assert_eq!(
        ask(
            &mut client,
            json!({"method":"send","session_id":session,
        "request_id":"crash-request","text":"different"})
        )
        .await["kind"],
        "error"
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        std::fs::read_to_string(host.root.path().join("prompts.log")).unwrap(),
        "wait\n"
    );
    let reconciled = ask(
        &mut client,
        json!({"method":"poll","session_id":session,"after":0}),
    )
    .await;
    assert_eq!(reconciled["session"]["status"], "unknown");
    host.stop(&mut client).await;
}

#[tokio::test]
async fn native_agent_state_files_reconnect_and_restart_share_real_server() {
    let mut host = Host::start().await;
    let project = host.root.path().join("项目 files");
    std::fs::create_dir(&project).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&project)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(project.join("task.txt"), "before\n").unwrap();
    let mut client = host.client().await;
    let Response::Hello {
        agents,
        path_syntax,
        ..
    } = client
        .request(Request::Hello { version: VERSION })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        path_syntax,
        Some(if cfg!(windows) {
            latte_work_protocol::PathSyntax::Windows
        } else {
            latte_work_protocol::PathSyntax::Posix
        })
    );
    let listing = ask(
        &mut client,
        json!({"method":"browse_directories","path":project}),
    )
    .await;
    assert_eq!(listing["name"], "项目 files");
    assert_eq!(
        listing["path"],
        project.canonicalize().unwrap().to_str().unwrap()
    );
    assert!(agents[0].available, "{}", agents[0].detail);
    assert!(agents[0].detail.contains("native-fixture"));
    let added = ask(
        &mut client,
        json!({"method":"add_project","path":project,"name":null}),
    )
    .await;
    let project_id = added["project"]["id"].as_str().unwrap();
    let permissions = ask(
        &mut client,
        json!({"method":"agent_permissions","agent":"claude"}),
    )
    .await;
    assert_eq!(permissions["kind"], "agent_permissions");
    let commands = ask(
        &mut client,
        json!({"method":"agent_commands","agent":"claude","project_id":project_id}),
    )
    .await;
    assert_eq!(commands["kind"], "agent_commands", "{commands}");
    let provider = ask(&mut client, json!({"method":"save_provider","provider":{
        "id":null,"name":"Native test","protocol":"anthropic_messages","base_url":"https://fixture.invalid",
        "model":"fixture-model","models":[],"model_labels":{},"auth":"api_key","credential":"native-private-test-key"
    }})).await;
    assert_eq!(provider["kind"], "providers", "{provider}");
    assert!(!provider.to_string().contains("native-private-test-key"));
    let provider_id = provider["providers"][0]["id"].as_str().unwrap();
    assert_ne!(
        ask(
            &mut client,
            json!({"method":"bind_agent_provider","agent":"claude","provider_id":provider_id})
        )
        .await["kind"],
        "error"
    );
    let created = ask(
        &mut client,
        json!({"method":"create_session","project_id":project_id,"agent":"claude"}),
    )
    .await;
    let session = created["session"]["id"].as_str().unwrap();
    let opened = ask(
        &mut client,
        json!({"method":"open_agent_session","session_id":session}),
    )
    .await;
    assert_ne!(opened["kind"], "error", "{opened}");
    assert!(!host.root.path().join("prompts.log").exists());
    let send =
        json!({"method":"send","session_id":session,"request_id":"native-edit","text":"edit"});
    assert_ne!(ask(&mut client, send.clone()).await["kind"], "error");
    finished(&mut client, session, "completed").await;
    let snapshot = ask(
        &mut client,
        json!({"method":"turn_change_summary","session_id":session,"request_id":"native-edit"}),
    )
    .await;
    assert_eq!(snapshot["kind"], "turn_changes", "{snapshot}");
    assert_eq!(
        snapshot["changes"]["summary"]["entries"][0]["path"],
        "task.txt"
    );
    let patch = ask(&mut client, json!({"method":"turn_change_diff","session_id":session,"request_id":"native-edit","path":"task.txt"})).await;
    assert!(
        patch["text"].as_str().unwrap().contains("+fixture edit"),
        "{patch}"
    );
    #[cfg(windows)]
    {
        let undo =
            json!({"method":"undo_turn_changes","session_id":session,"request_id":"native-edit"});
        let terminal_id = uuid::Uuid::new_v4().to_string();
        let opened = ask(&mut client, json!({"method":"create_terminal","project_id":project_id,"terminal_id":terminal_id,"cols":80,"rows":24})).await;
        assert_eq!(opened["kind"], "terminal", "{opened}");
        let refused = ask(&mut client, undo.clone()).await;
        assert_eq!(refused["kind"], "error", "{refused}");
        assert_eq!(
            ask(
                &mut client,
                json!({"method":"close_terminal","terminal_id":terminal_id})
            )
            .await["kind"],
            "ok"
        );
        std::fs::write(project.join("task.txt"), "external edit\n").unwrap();
        assert_eq!(ask(&mut client, undo.clone()).await["kind"], "error");
        assert_eq!(
            std::fs::read_to_string(project.join("task.txt")).unwrap(),
            "external edit\n"
        );
        std::fs::write(project.join("task.txt"), "fixture edit\n").unwrap();
        let restored = ask(&mut client, undo.clone()).await;
        assert_eq!(restored["changes"]["undo"], "reverted", "{restored}");
        assert_eq!(
            std::fs::read_to_string(project.join("task.txt")).unwrap(),
            "before\n"
        );
        std::fs::write(project.join("task.txt"), "post-undo manual edit\n").unwrap();
        assert_eq!(ask(&mut client, undo).await["changes"]["undo"], "reverted");
        assert_eq!(
            std::fs::read_to_string(project.join("task.txt")).unwrap(),
            "post-undo manual edit\n"
        );
        for path in ["task.txt:stream", "NUL", "task.txt.", "..\\outside"] {
            let refused = ask(
                &mut client,
                json!({"method":"read_file","project_id":project_id,"path":path}),
            )
            .await;
            assert_eq!(refused["kind"], "error", "{refused}");
        }
    }
    let old_server = client.server_id().to_owned();
    drop(client);
    let mut client = Client::local_cleanup(Path::new(SERVER), Some(&host.state()))
        .await
        .unwrap();
    assert_eq!(client.server_id(), old_server);
    ask(&mut client, send).await; // same request ID reconciles, never replays
    assert_eq!(
        std::fs::read_to_string(host.root.path().join("prompts.log")).unwrap(),
        "edit\n"
    );
    let reference = ask(&mut client, json!({"method":"resolve_reference","project_id":project_id,"path":project.join("task.txt")})).await;
    assert_eq!(reference["entry"]["path"], "task.txt", "{reference}");
    let attachment = ask(
        &mut client,
        json!({"method":"begin_attachment","name":"图片.txt","size":3}),
    )
    .await;
    let upload = attachment["id"].as_str().unwrap();
    ask(
        &mut client,
        json!({"method":"attachment_chunk","id":upload,"offset":0,"data":[97,98,99]}),
    )
    .await;
    let attachment = ask(
        &mut client,
        json!({"method":"finish_attachment","id":upload}),
    )
    .await;
    assert_ne!(attachment["kind"], "error", "{attachment}");
    ask(
        &mut client,
        json!({"method":"send","session_id":session,"request_id":"native-wait","text":"wait"}),
    )
    .await;
    finished(&mut client, session, "running").await;
    assert!(!client.shutdown_if_idle().await.unwrap());
    ask(&mut client, json!({"method":"cancel","session_id":session})).await;
    finished(&mut client, session, "stopped").await;
    let closed = ask(
        &mut client,
        json!({"method":"close_agent_session","session_id":session}),
    )
    .await;
    assert_ne!(closed["kind"], "error", "{closed}");
    host.stop(&mut client).await;
    drop(client);
    host.process = Host::spawn(host.root.path(), &host.fixture);
    host.ready().await;
    let mut client = host.client().await;
    assert_ne!(client.server_id(), old_server);
    let providers = ask(&mut client, json!({"method":"providers"})).await;
    assert_eq!(providers["providers"][0]["id"], provider_id);
    assert!(!providers.to_string().contains("native-private-test-key"));
    let history = ask(
        &mut client,
        json!({"method":"poll","session_id":session,"after":0}),
    )
    .await;
    assert_eq!(history["session"]["status"], "stopped");
    assert_eq!(
        std::fs::read_to_string(host.root.path().join("prompts.log")).unwrap(),
        "edit\nwait\n"
    );
    host.stop(&mut client).await;
}

#[cfg(windows)]
#[tokio::test]
async fn windows_capabilities_and_invalid_cli_fail_explicitly() {
    let mut host = Host::start().await;
    let mut client = host.client().await;
    let added = ask(
        &mut client,
        json!({"method":"add_project","path":host.root.path(),"name":null}),
    )
    .await;
    let project = added["project"]["id"].as_str().unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let terminal = ask(&mut client, json!({"method":"create_terminal","project_id":project,"terminal_id":id,"cols":80,"rows":24})).await;
    assert_eq!(terminal["kind"], "terminal", "{terminal}");
    assert_eq!(
        ask(
            &mut client,
            json!({"method":"close_terminal","terminal_id":id})
        )
        .await["kind"],
        "ok"
    );
    host.stop(&mut client).await;
    drop(client);
    host.fixture = host.root.path().join("unsupported.cmd");
    std::fs::write(&host.fixture, "@echo must-not-run").unwrap();
    host.process = Host::spawn(host.root.path(), &host.fixture);
    host.ready().await;
    let mut client = host.client().await;
    let Response::Hello { agents, .. } = client
        .request(Request::Hello { version: VERSION })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(!agents[0].available);
    assert!(agents[0].detail.contains("claude.exe"));
    assert!(!host.root.path().join("prompts.log").exists());
    host.stop(&mut client).await;
}
