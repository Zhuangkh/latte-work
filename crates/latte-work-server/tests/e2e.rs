//! Final-binary tests: real daemon, real socket/bridge, deterministic Claude process.
use latte_work_client::Client;
use latte_work_protocol::{EventKind, Request, Response, Status, VERSION};
use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Host {
    directory: tempfile::TempDir,
    process: Child,
}
impl Host {
    async fn start() -> Self {
        Self::start_with_environment(&[]).await
    }
    async fn start_with_environment(env: &[(&str, &std::ffi::OsStr)]) -> Self {
        let directory = tempfile::Builder::new()
            .prefix("lw-")
            .tempdir_in("/tmp")
            .unwrap();
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude.py");
        let process = Command::new(env!("CARGO_BIN_EXE_latte-work-server"))
            .env("LATTE_WORK_AGENT_ENV", "inherit")
            .arg("serve")
            .arg("--state-dir")
            .arg(directory.path())
            .env("LATTE_WORK_CLAUDE", fixture)
            .env("CLAUDE_CONFIG_DIR", directory.path().join("claude-config"))
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let host = Self { directory, process };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !host.directory.path().join("control.sock").exists() {
            assert!(Instant::now() < deadline, "server readiness timeout");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        host
    }
    async fn client(&self) -> Client {
        Client::local(
            Path::new(env!("CARGO_BIN_EXE_latte-work-server")),
            Some(self.directory.path()),
        )
        .await
        .unwrap()
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = kill(Pid::from_raw(self.process.id() as i32), Signal::SIGINT);
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.process.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}
async fn ask(c: &mut Client, r: Request) -> Response {
    c.request(r).await.unwrap()
}
#[tokio::test]
async fn recent_sessions_page_across_projects_with_live_evidence() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let roots = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let mut projects = Vec::new();
    for root in &roots {
        let Response::Project { project } = ask(
            &mut client,
            Request::AddProject {
                path: root.path().to_string_lossy().into_owned(),
                name: None,
            },
        )
        .await
        else {
            panic!("project response")
        };
        projects.push(project);
    }
    let mut ids = std::collections::HashSet::new();
    for n in 0..102 {
        let Response::Session { session } = ask(
            &mut client,
            Request::CreateSession {
                project_id: projects[n % 2].id.clone(),
                agent: "claude".into(),
            },
        )
        .await
        else {
            panic!("session response")
        };
        ids.insert(session.id);
    }
    let Response::RecentSessions { sessions, next } =
        ask(&mut client, Request::RecentSessions { before: None }).await
    else {
        panic!("recent response")
    };
    assert_eq!(sessions.len(), 100);
    assert!(
        sessions
            .iter()
            .all(|row| row.session.agent_session_open == Some(false)
                && row.session.agent_session_busy == Some(false))
    );
    assert!(
        sessions
            .iter()
            .any(|row| row.session.project_id == projects[0].id)
    );
    assert!(
        sessions
            .iter()
            .any(|row| row.session.project_id == projects[1].id)
    );
    let Response::RecentSessions {
        sessions: older,
        next: end,
    } = ask(&mut client, Request::RecentSessions { before: next }).await
    else {
        panic!("older response")
    };
    assert_eq!(older.len(), 2);
    assert!(end.is_none());
    let loaded: std::collections::HashSet<_> = sessions
        .iter()
        .chain(older.iter())
        .map(|row| row.session.id.clone())
        .collect();
    assert_eq!(loaded, ids);
    let id = sessions[0].session.id.clone();
    ask(
        &mut client,
        Request::ArchiveSession {
            session_id: id.clone(),
            archived: true,
        },
    )
    .await;
    let Response::RecentSessions { sessions, .. } =
        ask(&mut client, Request::RecentSessions { before: None }).await
    else {
        panic!("recent response")
    };
    assert!(!sessions.iter().any(|row| row.session.id == id));
}
#[tokio::test]
async fn browse_before_registration_and_named_projects_are_host_scoped() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let folder = project.path().join("中文 project");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(project.path().join("not-a-folder.txt"), "private").unwrap();
    let root = project.path().canonicalize().unwrap();
    match ask(
        &mut client,
        Request::BrowseDirectories {
            path: Some(root.to_string_lossy().into_owned()),
        },
    )
    .await
    {
        Response::Directories {
            path,
            entries,
            truncated,
            ..
        } => {
            assert_eq!(path, root.to_string_lossy());
            assert!(!truncated);
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].name, "中文 project");
            assert!(entries[0].directory);
        }
        other => panic!("{other:?}"),
    }
    for path in ["relative", "/missing-latte-work-test-directory"] {
        assert!(matches!(
            ask(
                &mut client,
                Request::BrowseDirectories {
                    path: Some(path.into())
                }
            )
            .await,
            Response::Error { .. }
        ));
    }
    let id = match ask(
        &mut client,
        Request::AddProject {
            path: folder.to_string_lossy().into_owned(),
            name: Some(" 自定义名称 ".into()),
        },
    )
    .await
    {
        Response::Project { project } => {
            assert_eq!(project.name, "自定义名称");
            project.id
        }
        other => panic!("{other:?}"),
    };
    let mut reconnect = host.client().await;
    assert!(
        matches!(ask(&mut reconnect, Request::Projects).await, Response::Projects { projects } if projects.len() == 1 && projects[0].id == id && projects[0].name == "自定义名称")
    );
    let other_host = Host::start().await;
    let mut other = other_host.client().await;
    assert!(
        matches!(ask(&mut other, Request::Projects).await, Response::Projects { projects } if projects.is_empty())
    );
}
async fn session(c: &mut Client, path: &Path) -> String {
    let p = match ask(
        c,
        Request::AddProject {
            path: path.to_string_lossy().into_owned(),
            name: None,
        },
    )
    .await
    {
        Response::Project { project } => project.id,
        r => panic!("{r:?}"),
    };
    match ask(
        c,
        Request::CreateSession {
            project_id: p,
            agent: "claude".into(),
        },
    )
    .await
    {
        Response::Session { session } => session.id,
        r => panic!("{r:?}"),
    }
}
async fn wait(c: &mut Client, id: &str, status: Status) -> Vec<latte_work_protocol::Event> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match ask(
            c,
            Request::Poll {
                session_id: id.into(),
                after: 0.0,
            },
        )
        .await
        {
            Response::Events {
                session, events, ..
            } if session.status == status => return events,
            Response::Events { session, .. } => assert!(
                Instant::now() < deadline,
                "expected {status:?}, got {:?}",
                session.status
            ),
            r => panic!("{r:?}"),
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
fn permission(events: &[latte_work_protocol::Event]) -> String {
    events
        .iter()
        .rev()
        .find_map(|e| {
            if let EventKind::Approval { request_id, .. } = &e.event {
                Some(request_id.clone())
            } else {
                None
            }
        })
        .unwrap()
}
async fn send(c: &mut Client, id: &str, request: &str, text: &str) -> Response {
    ask(
        c,
        Request::Send {
            permission_mode: None,
            provider: None,
            session_id: id.into(),
            request_id: request.into(),
            text: text.into(),
            model: None,
            effort: None,
        },
    )
    .await
}
#[tokio::test]
async fn singleton_multi_client_reconnect_approval_dedup_and_resume() {
    let host = Host::start().await;
    let mut a = host.client().await;
    let mut b = host.client().await;
    let hello = |r| match r {
        Response::Hello { server_id, .. } => server_id,
        r => panic!("{r:?}"),
    };
    assert_eq!(
        hello(ask(&mut a, Request::Hello { version: VERSION }).await),
        hello(ask(&mut b, Request::Hello { version: VERSION }).await)
    );
    let status = Command::new(env!("CARGO_BIN_EXE_latte-work-server"))
        .args(["serve", "--state-dir"])
        .arg(host.directory.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut a, project.path()).await;
    assert!(matches!(
        send(&mut a, &id, "request-1", "approve").await,
        Response::Accepted { duplicate: false }
    ));
    wait(&mut a, &id, Status::Waiting).await;
    drop(a);
    let events = wait(&mut b, &id, Status::Waiting).await;
    let approval_id = permission(&events);
    assert!(
        events
            .iter()
            .any(|e| matches!(e.event, EventKind::Approval { .. }))
    );
    assert!(matches!(
        send(&mut b, &id, "request-1", "approve").await,
        Response::Accepted { duplicate: true }
    ));
    assert!(matches!(
        ask(
            &mut b,
            Request::Approve {
                session_id: id.clone(),
                request_id: "wrong".into(),
                allow: true
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(!project.path().join("approved.txt").exists());
    assert!(matches!(
        ask(
            &mut b,
            Request::Approve {
                session_id: id.clone(),
                request_id: approval_id.clone(),
                allow: true
            }
        )
        .await,
        Response::Ok
    ));
    wait(&mut b, &id, Status::Completed).await;
    assert_eq!(
        std::fs::read_to_string(project.path().join("approved.txt")).unwrap(),
        "approved"
    );
    assert!(matches!(
        ask(
            &mut b,
            Request::Approve {
                session_id: id.clone(),
                request_id: approval_id.clone(),
                allow: true
            }
        )
        .await,
        Response::Error { .. }
    ));
    send(&mut b, &id, "request-2", "hello").await;
    let events = wait(&mut b, &id, Status::Completed).await;
    let text: String = events
        .iter()
        .filter_map(|e| {
            if let EventKind::Text { text } = &e.event {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(text, "resumed:你好");
    let cursor = events.last().unwrap().seq;
    assert!(
        matches!(ask(&mut b,Request::Poll{session_id:id,after:cursor}).await,Response::Events{events,..} if events.is_empty())
    );
}
#[tokio::test]
async fn denies_permissions_and_bounds_paths_and_failures() {
    let host = Host::start().await;
    let mut c = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut c, project.path()).await;
    send(&mut c, &id, "deny", "approve").await;
    let approval_id = permission(&wait(&mut c, &id, Status::Waiting).await);
    ask(
        &mut c,
        Request::Approve {
            session_id: id.clone(),
            request_id: approval_id.clone(),
            allow: false,
        },
    )
    .await;
    wait(&mut c, &id, Status::Completed).await;
    assert!(!project.path().join("approved.txt").exists());
    let project_id = match ask(&mut c, Request::Projects).await {
        Response::Projects { projects } => projects[0].id.clone(),
        _ => unreachable!(),
    };
    assert!(matches!(
        ask(
            &mut c,
            Request::ReadFile {
                project_id: project_id.clone(),
                path: "../../etc/passwd".into()
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(matches!(
        ask(
            &mut c,
            Request::CreateSession {
                project_id,
                agent: "codex".into()
            }
        )
        .await,
        Response::Error { .. }
    ));
    for text in ["malformed", "exit"] {
        assert!(matches!(
            ask(
                &mut c,
                Request::OpenAgentSession {
                    session_id: id.clone(),
                    provider: None
                }
            )
            .await,
            Response::Ok
        ));
        send(&mut c, &id, text, text).await;
        let events = wait(&mut c, &id, Status::Failed).await;
        assert!(events.iter().any(|e| matches!(
            e.event,
            EventKind::State {
                status: Status::Failed,
                ..
            }
        )));
    }
}
#[tokio::test]
async fn concurrent_agents_and_cancellation_are_isolated() {
    let host = Host::start().await;
    let mut c = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let a = session(&mut c, project.path()).await;
    let b = session(&mut c, project.path()).await;
    send(&mut c, &a, "hang", "hang").await;
    send(&mut c, &b, "fast", "hello").await;
    wait(&mut c, &b, Status::Completed).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    let child = loop {
        let events = match ask(
            &mut c,
            Request::Poll {
                session_id: a.clone(),
                after: 0.0,
            },
        )
        .await
        {
            Response::Events { events, .. } => events,
            _ => unreachable!(),
        };
        if let Some(pid) = events.iter().find_map(|e| {
            if let EventKind::Text { text } = &e.event {
                text.strip_prefix("child:")
                    .and_then(|s| s.parse::<i32>().ok())
            } else {
                None
            }
        }) {
            break pid;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    ask(
        &mut c,
        Request::Cancel {
            session_id: a.clone(),
        },
    )
    .await;
    wait(&mut c, &a, Status::Stopped).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while kill(Pid::from_raw(child), None).is_ok() {
        assert!(
            Instant::now() < deadline,
            "child escaped process-group cancellation"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn inspection_reads_host_files_and_both_git_views() {
    let host = Host::start().await;
    let mut c = host.client().await;
    let project = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(project.path())
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(project.path().join("hello.txt"), "staged\n").unwrap();
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(project.path())
            .args(["add", "hello.txt"])
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(project.path().join("hello.txt"), "working\n").unwrap();
    let id = match ask(
        &mut c,
        Request::AddProject {
            path: project.path().to_string_lossy().into_owned(),
            name: None,
        },
    )
    .await
    {
        Response::Project { project } => project.id,
        r => panic!("{r:?}"),
    };
    assert!(
        matches!(ask(&mut c, Request::ReadFile { project_id: id.clone(), path: "hello.txt".into() }).await, Response::Content { text, truncated: false } if text == "working\n")
    );
    assert!(
        matches!(ask(&mut c, Request::Files { project_id: id.clone(), path: "".into() }).await, Response::Files { entries } if entries.iter().any(|e| e.name == "hello.txt") && entries.iter().all(|e| e.name != ".git"))
    );
    let outside = tempfile::tempdir().unwrap();
    let external = outside.path().join("reference.txt");
    std::fs::write(&external, "external reference").unwrap();
    let absolute = external
        .canonicalize()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        matches!(ask(&mut c, Request::ResolveReference { project_id: id.clone(), path: absolute.clone() }).await,
        Response::FileReference { entry } if entry.path == absolute && !entry.directory)
    );
    assert!(matches!(
        ask(
            &mut c,
            Request::ReadFile {
                project_id: id.clone(),
                path: absolute
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(matches!(
        ask(
            &mut c,
            Request::ResolveReference {
                project_id: id.clone(),
                path: "../outside".into()
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(matches!(
        ask(
            &mut c,
            Request::ResolveReference {
                project_id: "unknown".into(),
                path: external.to_str().unwrap().into()
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(
        matches!(ask(&mut c, Request::Diff { project_id: id.clone() }).await, Response::Content { text, .. } if text.contains("+working") && text.contains("+staged"))
    );
    assert!(
        matches!(ask(&mut c, Request::Changes { project_id: id.clone() }).await, Response::Changes { entries, truncated: false } if entries.iter().filter(|entry| entry.path == "hello.txt").count() == 2)
    );
    assert!(
        matches!(ask(&mut c, Request::ChangeDiff { project_id: id.clone(), path: "hello.txt".into(), section: latte_work_protocol::ChangeSection::Staged }).await, Response::Content { text, truncated: false } if text.contains("+staged") && !text.contains("+working"))
    );
    assert!(matches!(
        ask(
            &mut c,
            Request::ChangeDiff {
                project_id: id.clone(),
                path: "../outside".into(),
                section: latte_work_protocol::ChangeSection::Untracked
            }
        )
        .await,
        Response::Error { .. }
    ));
    std::fs::write(project.path().join("binary"), [0, 1, 2]).unwrap();
    assert!(matches!(
        ask(
            &mut c,
            Request::ReadFile {
                project_id: id.clone(),
                path: "binary".into()
            }
        )
        .await,
        Response::Error { .. }
    ));
    std::fs::write(project.path().join("large"), vec![b'a'; 600 * 1024]).unwrap();
    assert!(
        matches!(ask(&mut c, Request::ReadFile { project_id: id, path: "large".into() }).await, Response::Content { text, truncated: true } if text.len() == 512 * 1024)
    );
}

#[tokio::test]
async fn provider_catalog_local_binding_and_remote_sync_use_host_protocol() {
    use latte_work_protocol::{ProviderAuth, ProviderDraft, ProviderProtocol, ProviderTarget};
    use std::os::unix::fs::PermissionsExt;
    let remote = Host::start().await;
    let ssh_dir = tempfile::tempdir().unwrap();
    let ssh = ssh_dir.path().join("ssh");
    let bin = serde_json::to_string(env!("CARGO_BIN_EXE_latte-work-server")).unwrap();
    let state = serde_json::to_string(&remote.directory.path().to_string_lossy()).unwrap();
    std::fs::write(&ssh,format!("#!/usr/bin/env python3\nimport os,sys\nif 'offline-fixture' in sys.argv: sys.exit(1)\nos.execv({bin},[{bin},'connect','--state-dir',{state}])\n")).unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("lw-")
        .tempdir_in("/tmp")
        .unwrap();
    let process = Command::new(env!("CARGO_BIN_EXE_latte-work-server"))
        .args(["serve", "--state-dir"])
        .arg(directory.path())
        .env(
            "LATTE_WORK_CLAUDE",
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude.py"),
        )
        .env(
            "PATH",
            format!(
                "{}:{}",
                ssh_dir.path().display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let local = Host { directory, process };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !local.directory.path().join("control.sock").exists() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut c = local.client().await;
    let draft = ProviderDraft {
        id: None,
        name: "Central".into(),
        protocol: ProviderProtocol::AnthropicMessages,
        base_url: "https://example.test".into(),
        model: "first".into(),
        models: vec!["alternate".into()],
        model_labels: Default::default(),
        auth: ProviderAuth::ApiKey,
        credential: Some("fixture-private-key".into()),
    };
    let saved = ask(
        &mut c,
        Request::SaveProvider {
            provider: draft.clone(),
        },
    )
    .await;
    let id = match &saved {
        Response::Providers {
            providers,
            bindings,
        } => {
            assert!(bindings.is_empty());
            providers[0].id.clone()
        }
        r => panic!("{r:?}"),
    };
    assert!(
        !serde_json::to_string(&saved)
            .unwrap()
            .contains("fixture-private-key")
    );
    let target = Some(ProviderTarget {
        ssh: "fixture-host".into(),
        server_path: env!("CARGO_BIN_EXE_latte-work-server").into(),
    });
    let result = ask(
        &mut c,
        Request::BindAgentProvider {
            agent: "claude".into(),
            provider_id: Some(id.clone()),
            target: target.clone(),
        },
    )
    .await;
    assert!(matches!(result,Response::Providers{bindings,..} if bindings[0].provider_id==id));
    assert!(
        matches!(ask(&mut c,Request::Providers).await,Response::Providers{bindings,..} if bindings.is_empty())
    );
    let mut r = remote.client().await;
    assert!(
        matches!(ask(&mut r, Request::Models { project_id: None, agent: "claude".into(), model: None }).await, Response::Models { models, .. } if models == ["first", "alternate"])
    );

    let project = tempfile::tempdir().unwrap();
    let session = session(&mut r, project.path()).await;
    send(&mut r, &session, "first", "provider").await;
    assert!(
        wait(&mut r, &session, Status::Completed)
            .await
            .iter()
            .any(|e| matches!(&e.event,EventKind::Text{text} if text=="fresh:first:api_key"))
    );
    let mut updated = draft;
    updated.id = Some(id.clone());
    updated.credential = None;
    updated.model = "second".into();
    ask(&mut c, Request::SaveProvider { provider: updated }).await;
    let offline = Some(ProviderTarget {
        ssh: "offline-fixture".into(),
        server_path: "/server".into(),
    });
    assert!(matches!(
        ask(
            &mut c,
            Request::BindAgentProvider {
                agent: "claude".into(),
                provider_id: Some(id.clone()),
                target: offline
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(
        matches!(ask(&mut r,Request::Providers).await,Response::Providers{providers,..} if providers[0].model=="first")
    );
    ask(
        &mut c,
        Request::BindAgentProvider {
            agent: "claude".into(),
            provider_id: Some(id.clone()),
            target: target.clone(),
        },
    )
    .await;
    send(&mut r, &session, "second", "provider").await;
    assert!(
        wait(&mut r, &session, Status::Completed)
            .await
            .iter()
            .any(|e| matches!(&e.event,EventKind::Text{text} if text=="resumed:second:api_key"))
    );
    for protocol in [
        ProviderProtocol::OpenaiChat,
        ProviderProtocol::OpenaiResponses,
    ] {
        let provider = ProviderDraft {
            id: None,
            name: "Other".into(),
            protocol,
            base_url: "https://example.test/v1".into(),
            model: "model".into(),
            models: vec![],
            model_labels: Default::default(),
            auth: ProviderAuth::Bearer,
            credential: Some("other-key".into()),
        };
        let pid = match ask(&mut c, Request::SaveProvider { provider }).await {
            Response::Providers { providers, .. } => providers.last().unwrap().id.clone(),
            _ => unreachable!(),
        };
        assert!(matches!(
            ask(
                &mut c,
                Request::BindAgentProvider {
                    agent: "claude".into(),
                    provider_id: Some(pid),
                    target: target.clone()
                }
            )
            .await,
            Response::Error { .. }
        ));
    }
    ask(
        &mut c,
        Request::BindAgentProvider {
            agent: "claude".into(),
            provider_id: None,
            target,
        },
    )
    .await;
    assert!(
        matches!(ask(&mut r,Request::Providers).await,Response::Providers{providers,bindings} if providers.is_empty()&&bindings.is_empty())
    );
    assert!(
        matches!(ask(&mut c,Request::BindAgentProvider{agent:"claude".into(),provider_id:Some(id),target:None}).await,Response::Providers{bindings,..} if bindings.len()==1)
    );
}

#[tokio::test]
async fn rename_and_remove_project_preserve_files_sessions_and_restore_identity() {
    let host = Host::start().await;
    let mut c = host.client().await;
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("keep.txt"), "keep").unwrap();
    let sid = session(&mut c, directory.path()).await;
    let pid = match ask(&mut c, Request::Projects).await {
        Response::Projects { projects } => projects[0].id.clone(),
        _ => unreachable!(),
    };
    assert!(
        matches!(ask(&mut c,Request::RenameProject{project_id:pid.clone(),name:"  Renamed  ".into()}).await,Response::Project{project} if project.name=="Renamed")
    );
    assert!(matches!(
        ask(
            &mut c,
            Request::RenameProject {
                project_id: pid.clone(),
                name: "\n".into()
            }
        )
        .await,
        Response::Error { .. }
    ));
    send(&mut c, &sid, "pending", "hang").await;
    assert!(matches!(
        ask(
            &mut c,
            Request::RemoveProject {
                project_id: pid.clone()
            }
        )
        .await,
        Response::Error { .. }
    ));
    ask(
        &mut c,
        Request::Cancel {
            session_id: sid.clone(),
        },
    )
    .await;
    wait(&mut c, &sid, Status::Stopped).await;
    assert!(
        matches!(ask(&mut c,Request::RemoveProject{project_id:pid.clone()}).await,Response::Projects{projects} if projects.is_empty())
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("keep.txt")).unwrap(),
        "keep"
    );
    let mut reconnect = host.client().await;
    assert!(
        matches!(ask(&mut reconnect,Request::Projects).await,Response::Projects{projects} if projects.is_empty())
    );
    assert!(
        matches!(ask(&mut reconnect,Request::AddProject{path:directory.path().to_string_lossy().into_owned(),name:None}).await,Response::Project{project} if project.id==pid&&project.name=="Renamed")
    );
    assert!(
        matches!(ask(&mut reconnect,Request::Sessions{project_id:pid}).await,Response::Sessions{sessions} if sessions.iter().any(|s|s.id==sid))
    );
}

#[tokio::test]
async fn models_select_per_turn_validate_and_persist_without_changing_provider() {
    use latte_work_protocol::{ProviderAuth, ProviderDraft, ProviderProtocol};
    let host = Host::start().await;
    let mut c = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let sid = session(&mut c, project.path()).await;
    let aliases = ask(
        &mut c,
        Request::Models {
            project_id: None,
            agent: "claude".into(),
            model: None,
        },
    )
    .await;
    assert!(
        matches!(aliases, Response::Models { models, provider: None, .. } if models.contains(&"sonnet".into()))
    );
    let send_model = |request: &str, model: Option<&str>, text: &str| Request::Send {
        permission_mode: None,
        provider: None,
        session_id: sid.clone(),
        request_id: request.into(),
        text: text.into(),
        model: model.map(str::to_owned),
        effort: None,
    };
    assert!(matches!(
        ask(&mut c, send_model("cli", Some("sonnet"), "model")).await,
        Response::Accepted { duplicate: false }
    ));
    let events = wait(&mut c, &sid, Status::Completed).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, EventKind::Text { text } if text == "fresh:sonnet"))
    );
    assert!(matches!(
        ask(&mut c, send_model("reset", None, "model")).await,
        Response::Accepted { .. }
    ));
    let events = wait(&mut c, &sid, Status::Completed).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, EventKind::Text { text } if text == "resumed:default"))
    );
    let saved = ask(
        &mut c,
        Request::SaveProvider {
            provider: ProviderDraft {
                id: None,
                name: "Model catalog".into(),
                protocol: ProviderProtocol::AnthropicMessages,
                base_url: "https://example.test".into(),
                model: "first".into(),
                models: vec!["second".into(), "third".into()],
                model_labels: Default::default(),
                auth: ProviderAuth::ApiKey,
                credential: Some("fixture-private-key".into()),
            },
        },
    )
    .await;
    let Response::Providers { providers, .. } = saved else {
        panic!("save failed")
    };
    ask(
        &mut c,
        Request::BindAgentProvider {
            agent: "claude".into(),
            provider_id: Some(providers[0].id.clone()),
            target: None,
        },
    )
    .await;
    assert!(matches!(
        ask(&mut c, send_model("invalid", Some("sonnet"), "provider")).await,
        Response::Error { .. }
    ));
    assert!(matches!(
        ask(&mut c, send_model("selected", Some("second"), "provider")).await,
        Response::Accepted { duplicate: false }
    ));
    let events = wait(&mut c, &sid, Status::Completed).await;
    assert!(
        events.iter().any(
            |e| matches!(&e.event, EventKind::Text { text } if text == "resumed:second:api_key")
        )
    );
    assert!(matches!(
        ask(&mut c, send_model("selected", Some("second"), "provider")).await,
        Response::Accepted { duplicate: true }
    ));
    ask(
        &mut c,
        Request::BindAgentProvider {
            agent: "claude".into(),
            provider_id: None,
            target: None,
        },
    )
    .await;
    assert!(matches!(
        ask(&mut c, send_model("selected", Some("second"), "provider")).await,
        Response::Accepted { duplicate: true }
    ));
    assert!(matches!(
        ask(&mut c, send_model("selected", Some("third"), "provider")).await,
        Response::Error { message, .. } if message.contains("请求 ID 已被其他任务使用")
    ));
    assert!(matches!(
        ask(&mut c, send_model("new", Some("second"), "provider")).await,
        Response::Error { .. }
    ));
    ask(
        &mut c,
        Request::BindAgentProvider {
            agent: "claude".into(),
            provider_id: Some(providers[0].id.clone()),
            target: None,
        },
    )
    .await;
    assert!(matches!(
        ask(&mut c, send_model("selected", Some("third"), "provider")).await,
        Response::Error { .. }
    ));
    let mut reconnect = host.client().await;
    assert!(
        matches!(ask(&mut reconnect, Request::Poll { session_id: sid.clone(), after: 0.0 }).await, Response::Events { session, .. } if session.model.as_deref() == Some("second"))
    );
    assert!(
        matches!(ask(&mut c, Request::Models { project_id: None, agent: "claude".into(), model: None }).await, Response::Models { default_model: Some(model), models, .. } if model == "first" && models == ["first", "second", "third"])
    );
    assert!(matches!(
        ask(&mut c, send_model("default", None, "provider")).await,
        Response::Accepted { .. }
    ));
    let events = wait(&mut c, &sid, Status::Completed).await;
    assert!(
        events.iter().any(
            |e| matches!(&e.event, EventKind::Text { text } if text == "resumed:first:api_key")
        )
    );
}

#[tokio::test]
async fn native_session_titles_sync_both_ways_without_sending_or_stopping_turns() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    send(&mut client, &id, "title-r1", "native-title").await;
    let events = wait(&mut client, &id, Status::Completed).await;
    let before = events
        .iter()
        .filter(|e| matches!(e.event, EventKind::User { .. }))
        .count();
    assert!(
        matches!(ask(&mut client,Request::Session{session_id:id.clone()}).await,Response::Session{session} if session.title=="Agent generated title" && !session.custom_title)
    );
    let rename = |title: &str| Request::RenameSession {
        session_id: id.clone(),
        title: title.into(),
    };
    assert!(
        matches!(ask(&mut client,rename("Manual title")).await,Response::Session{session} if session.title=="Manual title" && session.custom_title)
    );
    assert!(matches!(
        ask(&mut client, rename("FAIL_TITLE")).await,
        Response::Error { .. }
    ));
    assert!(
        matches!(ask(&mut client,Request::Poll{session_id:id.clone(),after:0.0}).await,Response::Events{session,events,..} if session.title=="Manual title" && session.status==Status::Completed && events.iter().filter(|e|matches!(e.event,EventKind::User{..})).count()==before)
    );
    send(&mut client, &id, "title-r2", "native-title-update").await;
    wait(&mut client, &id, Status::Completed).await;
    assert!(
        matches!(ask(&mut client,Request::Session{session_id:id.clone()}).await,Response::Session{session} if session.title=="Manual title")
    );
    // Live rename while work is running must not end the turn or create a prompt.
    send(&mut client, &id, "title-hang", "hang").await;
    assert!(
        matches!(ask(&mut client,rename("While running")).await,Response::Session{session} if session.title=="While running" && session.status==Status::Running)
    );
    ask(
        &mut client,
        Request::Cancel {
            session_id: id.clone(),
        },
    )
    .await;
    wait(&mut client, &id, Status::Stopped).await;
    assert!(matches!(
        ask(
            &mut client,
            Request::CloseAgentSession {
                session_id: id.clone(),
                only_if_idle: None,
            },
        )
        .await,
        Response::Ok
    ));
    assert!(
        matches!(ask(&mut client,rename("Closed title")).await,Response::Session{session} if session.title=="Closed title" && session.agent_session_open==Some(false))
    );
    let cwd = project.path().canonicalize().unwrap();
    let name: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let native = host
        .directory
        .path()
        .join("claude-config/projects")
        .join(name)
        .join("11111111-1111-4111-8111-111111111111.jsonl");
    let saved = std::fs::read_to_string(&native).unwrap();
    assert!(saved.contains("Closed title"));
    // An external native rename is reflected on read, including across reconnect.
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&native)
        .unwrap();
    writeln!(file,"{}",serde_json::json!({"type":"custom-title","customTitle":"External title","sessionId":"11111111-1111-4111-8111-111111111111"})).unwrap();
    let mut reconnect = host.client().await;
    // Native file reads are debounced across clients; external renames appear
    // on a later metadata read, without launching an Agent or sending a prompt.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let response = ask(
            &mut reconnect,
            Request::Session {
                session_id: id.clone(),
            },
        )
        .await;
        if matches!(response, Response::Session { session } if session.title=="External title" && session.custom_title)
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "external title was not refreshed"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    ask(
        &mut reconnect,
        Request::OpenAgentSession {
            session_id: id.clone(),
            provider: None,
        },
    )
    .await;
    send(&mut reconnect, &id, "title-r3", "identity").await;
    wait(&mut reconnect, &id, Status::Completed).await;
    assert!(
        matches!(ask(&mut reconnect,Request::Session{session_id:id}).await,Response::Session{session} if session.title=="External title")
    );
}

#[tokio::test]
async fn session_menu_metadata_is_durable_and_host_scoped() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    assert!(
        matches!(ask(&mut client, Request::RenameSession { session_id: id.clone(), title: "Renamed".into() }).await,
        Response::Session { session } if session.title == "Renamed" && session.custom_title)
    );
    ask(
        &mut client,
        Request::PinSession {
            session_id: id.clone(),
            pinned: true,
        },
    )
    .await;
    ask(
        &mut client,
        Request::MarkSessionUnread {
            session_id: id.clone(),
            unread: true,
        },
    )
    .await;
    let mut second = host.client().await;
    assert!(matches!(ask(&mut second, Request::PinnedSessions).await,
        Response::Sessions { sessions } if sessions.len() == 1 && sessions[0].id == id && sessions[0].unread));
    let other_host = Host::start().await;
    let mut other = other_host.client().await;
    assert!(
        matches!(ask(&mut other, Request::PinnedSessions).await, Response::Sessions { sessions } if sessions.is_empty())
    );
    assert!(matches!(
        ask(
            &mut other,
            Request::RenameSession {
                session_id: id.clone(),
                title: "wrong host".into()
            }
        )
        .await,
        Response::Error { .. }
    ));
    ask(
        &mut client,
        Request::ArchiveSession {
            session_id: id.clone(),
            archived: true,
        },
    )
    .await;
    assert!(
        matches!(ask(&mut second, Request::PinnedSessions).await, Response::Sessions { sessions } if sessions.is_empty())
    );
    assert!(
        matches!(ask(&mut second, Request::Poll { session_id: id.clone(), after: 0.0 }).await,
        Response::Events { session, events, .. } if session.archived && session.title == "Renamed" && events.is_empty())
    );
    assert!(
        matches!(ask(&mut client, Request::ArchiveSession { session_id: id, archived: false }).await,
        Response::Session { session } if !session.archived && session.pinned_at.is_none())
    );
}

#[tokio::test]
async fn effort_is_applied_on_launch_resume_and_reset() {
    use latte_work_protocol::Effort;
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    assert!(
        matches!(ask(&mut client, Request::Models { project_id: None, agent: "claude".into(), model: Some("haiku".into()) }).await,
        Response::Models { effort_levels, .. } if effort_levels.is_empty())
    );
    assert!(matches!(
        ask(
            &mut client,
            Request::Send {
                permission_mode: None,
                provider: None,
                session_id: id.clone(),
                request_id: "unsupported".into(),
                text: "effort".into(),
                model: Some("haiku".into()),
                effort: Some(Effort::High)
            }
        )
        .await,
        Response::Error { .. }
    ));
    std::fs::create_dir(project.path().join(".claude")).unwrap();
    std::fs::write(
        project.path().join(".claude/settings.json"),
        r#"{"effortLevel":"medium"}"#,
    )
    .unwrap();
    for (request_id, effort, expected) in [
        ("effort-native", None, "fresh:medium"),
        ("effort-high", Some(Effort::High), "resumed:high"),
        ("effort-low", Some(Effort::Low), "resumed:low"),
        ("effort-inherit", None, "resumed:medium"),
    ] {
        let send = Request::Send {
            permission_mode: None,
            provider: None,
            session_id: id.clone(),
            request_id: request_id.into(),
            text: "effort".into(),
            model: None,
            effort,
        };
        assert!(matches!(
            ask(&mut client, send.clone()).await,
            Response::Accepted { duplicate: false }
        ));
        let events = wait(&mut client, &id, Status::Completed).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(&e.event, EventKind::Text { text } if text == expected))
        );
        assert!(matches!(
            ask(&mut client, send).await,
            Response::Accepted { duplicate: true }
        ));
        let mut reconnect = host.client().await;
        assert!(
            matches!(ask(&mut reconnect, Request::Poll { session_id: id.clone(), after: 0.0 }).await,
            Response::Events { session, .. } if session.effort == effort)
        );
    }
}

#[tokio::test]
async fn native_model_names_are_host_project_scoped_and_do_not_override_provider_models() {
    use latte_work_protocol::{ProviderAuth, ProviderDraft, ProviderProtocol};
    let host = Host::start().await;
    let config = host.directory.path().join("claude-config");
    std::fs::create_dir(&config).unwrap();
    std::fs::write(
        config.join("settings.json"),
        serde_json::json!({"env": {
            "ANTHROPIC_DEFAULT_SONNET_MODEL": "native-id",
            "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME": "Native name",
            "ANTHROPIC_API_KEY": "do-not-return-this"
        }})
        .to_string(),
    )
    .unwrap();
    let mut c = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let Response::Project {
        project: registered,
    } = ask(
        &mut c,
        Request::AddProject {
            path: project.path().to_string_lossy().into_owned(),
            name: None,
        },
    )
    .await
    else {
        panic!("project registration failed")
    };
    let models = |id| Request::Models {
        agent: "claude".into(),
        model: None,
        project_id: id,
    };
    let response = ask(&mut c, models(None)).await;
    assert!(
        matches!(&response, Response::Models { models, model_labels, .. }
        if models.contains(&"sonnet".into()) && model_labels.get("sonnet").map(String::as_str) == Some("Native name"))
    );
    assert!(
        !serde_json::to_string(&response)
            .unwrap()
            .contains("do-not-return-this")
    );
    std::fs::create_dir(project.path().join(".claude")).unwrap();
    std::fs::write(
        project.path().join(".claude/settings.local.json"),
        serde_json::json!({"env": {
            "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME": "Project name"
        }})
        .to_string(),
    )
    .unwrap();
    assert!(
        matches!(ask(&mut c, models(Some(registered.id.clone()))).await,
        Response::Models { model_labels, .. } if model_labels.get("sonnet").map(String::as_str) == Some("Project name"))
    );
    assert!(matches!(
        ask(&mut c, models(Some("unknown-project".into()))).await,
        Response::Error { .. }
    ));
    let Response::Providers { providers, .. } = ask(
        &mut c,
        Request::SaveProvider {
            provider: ProviderDraft {
                id: None,
                name: "Bound".into(),
                protocol: ProviderProtocol::AnthropicMessages,
                base_url: "https://example.test".into(),
                model: "sonnet".into(),
                models: vec![],
                model_labels: Default::default(),
                auth: ProviderAuth::ApiKey,
                credential: Some("fixture-only".into()),
            },
        },
    )
    .await
    else {
        panic!("save failed")
    };
    ask(
        &mut c,
        Request::BindAgentProvider {
            agent: "claude".into(),
            provider_id: Some(providers[0].id.clone()),
            target: None,
        },
    )
    .await;
    assert!(matches!(ask(&mut c, models(Some(registered.id))).await,
        Response::Models { model_labels, provider: Some(_), .. } if model_labels.is_empty()));
}

#[tokio::test]
async fn remote_turns_use_latest_local_provider_without_persisting_snapshots() {
    use latte_work_client::request_with_provider;
    use latte_work_protocol::{ProviderAuth, ProviderDraft, ProviderProtocol, TurnProvider};
    use tokio::sync::Mutex;
    let local_host = Host::start().await;
    let remote_host = Host::start().await;
    let local = Mutex::new(local_host.client().await);
    let remote = Mutex::new(remote_host.client().await);
    let mut draft = ProviderDraft {
        id: None,
        name: "Per turn".into(),
        protocol: ProviderProtocol::AnthropicMessages,
        base_url: "https://example.test".into(),
        model: "first".into(),
        models: vec!["alternate".into()],
        model_labels: Default::default(),
        auth: ProviderAuth::ApiKey,
        credential: Some("fixture-private-key".into()),
    };
    let saved = local
        .lock()
        .await
        .request(Request::SaveProvider {
            provider: draft.clone(),
        })
        .await
        .unwrap();
    let id = match saved {
        Response::Providers { providers, .. } => providers[0].id.clone(),
        other => panic!("{other:?}"),
    };
    let bind = |provider_id| Request::BindHostAgentProvider {
        host_id: "remote-fixture".into(),
        agent: "claude".into(),
        provider_id,
    };
    assert!(
        matches!(local.lock().await.request(bind(Some(id.clone()))).await.unwrap(), Response::Providers { bindings, .. } if bindings[0].provider_id == id)
    );
    // Binding is local: no contact with or configuration file on the remote host.
    assert!(!remote_host.directory.path().join("providers.json").exists());
    let models = || Request::Models {
        agent: "claude".into(),
        model: None,
        project_id: None,
    };
    let result = request_with_provider(&local, &remote, "remote-fixture".into(), models())
        .await
        .unwrap();
    assert!(matches!(result, Response::Models { models, .. } if models == ["first", "alternate"]));
    let mut observer = remote_host.client().await;
    let project = tempfile::tempdir().unwrap();
    let sid = session(&mut observer, project.path()).await;
    let send = |request_id: &str, text: &str| Request::Send {
        permission_mode: Some("plan".into()),
        provider: None,
        session_id: sid.clone(),
        request_id: request_id.into(),
        text: text.into(),
        model: None,
        effort: None,
    };
    let snapshot = match local
        .lock()
        .await
        .request(Request::ExportHostAgentProvider {
            host_id: "remote-fixture".into(),
            agent: "claude".into(),
        })
        .await
        .unwrap()
    {
        Response::ProviderSnapshot {
            snapshot: Some(snapshot),
            ..
        } => snapshot,
        other => panic!("{other:?}"),
    };
    let result = request_with_provider(
        &local,
        &remote,
        "remote-fixture".into(),
        send("first-turn", "provider"),
    )
    .await
    .unwrap();
    assert!(matches!(result, Response::Accepted { duplicate: false }));
    // Losing the initiating transport does not kill the server-owned turn.
    drop(remote);
    let events = wait(&mut observer, &sid, Status::Completed).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, EventKind::Text { text } if text == "fresh:first:api_key"))
    );
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("fixture-private-key")
    );
    let remote = Mutex::new(remote_host.client().await);
    assert!(matches!(
        request_with_provider(
            &local,
            &remote,
            "remote-fixture".into(),
            send("first-turn", "provider")
        )
        .await
        .unwrap(),
        Response::Accepted { duplicate: true }
    ));

    draft.id = Some(id.clone());
    draft.model = "second".into();
    draft.credential = None;
    local
        .lock()
        .await
        .request(Request::SaveProvider { provider: draft })
        .await
        .unwrap();
    // A reused request ID cannot silently acquire the updated provider snapshot.
    assert!(matches!(
        request_with_provider(
            &local,
            &remote,
            "remote-fixture".into(),
            send("first-turn", "provider")
        )
        .await
        .unwrap(),
        Response::Error { .. }
    ));
    let original = Request::Send {
        permission_mode: Some("plan".into()),
        provider: Some(TurnProvider::Snapshot(snapshot.clone())),
        session_id: sid.clone(),
        request_id: "first-turn".into(),
        text: "provider".into(),
        model: None,
        effort: None,
    };
    assert!(matches!(
        remote.lock().await.request(original).await.unwrap(),
        Response::Accepted { duplicate: true }
    ));
    let mut changed = snapshot;
    changed.credential = "different-secret-same-revision".into();
    assert!(matches!(
        remote
            .lock()
            .await
            .request(Request::Send {
                permission_mode: Some("plan".into()),
                provider: Some(TurnProvider::Snapshot(changed)),
                session_id: sid.clone(),
                request_id: "first-turn".into(),
                text: "provider".into(),
                model: None,
                effort: None
            })
            .await
            .unwrap(),
        Response::Error { .. }
    ));
    assert!(
        matches!(request_with_provider(&local, &remote, "remote-fixture".into(), models()).await.unwrap(), Response::Models { default_model: Some(model), .. } if model == "second")
    );
    assert!(matches!(
        request_with_provider(
            &local,
            &remote,
            "remote-fixture".into(),
            send("second-turn", "provider")
        )
        .await
        .unwrap(),
        Response::Accepted { duplicate: false }
    ));
    assert!(
        wait(&mut observer, &sid, Status::Completed)
            .await
            .iter()
            .any(
                |e| matches!(&e.event, EventKind::Text { text } if text == "resumed:second:api_key")
            )
    );

    assert!(
        matches!(local.lock().await.request(bind(None)).await.unwrap(), Response::Providers { bindings, .. } if bindings.is_empty())
    );
    assert!(matches!(
        request_with_provider(&local, &remote, "remote-fixture".into(), models())
            .await
            .unwrap(),
        Response::Models { provider: None, .. }
    ));
    assert!(matches!(
        request_with_provider(
            &local,
            &remote,
            "remote-fixture".into(),
            send("cli-turn", "model")
        )
        .await
        .unwrap(),
        Response::Accepted { duplicate: false }
    ));
    assert!(
        wait(&mut observer, &sid, Status::Completed)
            .await
            .iter()
            .any(|e| matches!(&e.event, EventKind::Text { text } if text == "resumed:default"))
    );
    assert!(
        matches!(remote.lock().await.request(Request::Providers).await.unwrap(), Response::Providers { providers, bindings } if providers.is_empty() && bindings.is_empty())
    );
    assert!(!remote_host.directory.path().join("providers.json").exists());
    for entry in std::fs::read_dir(remote_host.directory.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let bytes = std::fs::read(path).unwrap();
            assert!(
                !bytes
                    .windows(b"fixture-private-key".len())
                    .any(|w| w == b"fixture-private-key")
            );
        }
    }
    // An old remote binding must not silently become CLI defaults on upgrade.
    let legacy = remote
        .lock()
        .await
        .request(Request::SaveProvider {
            provider: ProviderDraft {
                id: None,
                name: "Legacy remote".into(),
                protocol: ProviderProtocol::AnthropicMessages,
                base_url: "https://example.test".into(),
                model: "legacy".into(),
                models: vec![],
                model_labels: Default::default(),
                auth: ProviderAuth::ApiKey,
                credential: Some("legacy-secret".into()),
            },
        })
        .await
        .unwrap();
    let legacy_id = match legacy {
        Response::Providers { providers, .. } => providers[0].id.clone(),
        other => panic!("{other:?}"),
    };
    remote
        .lock()
        .await
        .request(Request::BindAgentProvider {
            agent: "claude".into(),
            provider_id: Some(legacy_id),
            target: None,
        })
        .await
        .unwrap();
    local
        .lock()
        .await
        .request(Request::ForgetHostProviders {
            host_id: "remote-fixture".into(),
        })
        .await
        .unwrap();
    let error = request_with_provider(&local, &remote, "remote-fixture".into(), models())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("旧 Provider 关联"));
    assert!(
        request_with_provider(
            &local,
            &remote,
            "remote-fixture".into(),
            send("blocked-upgrade", "model")
        )
        .await
        .is_err()
    );
    local.lock().await.request(bind(None)).await.unwrap();
    assert!(matches!(
        request_with_provider(&local, &remote, "remote-fixture".into(), models())
            .await
            .unwrap(),
        Response::Models { provider: None, .. }
    ));
}

async fn terminal_project(client: &mut Client, path: &Path) -> String {
    match ask(
        client,
        Request::AddProject {
            path: path.to_string_lossy().into_owned(),
            name: None,
        },
    )
    .await
    {
        Response::Project { project } => project.id,
        r => panic!("{r:?}"),
    }
}
async fn terminal_write(client: &mut Client, id: &str, data: &[u8]) {
    assert!(matches!(
        ask(
            client,
            Request::WriteTerminal {
                terminal_id: id.into(),
                data: data.to_vec()
            }
        )
        .await,
        Response::Ok
    ));
}
async fn terminal_until(client: &mut Client, id: &str, cursor: &mut f64, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut bytes = Vec::new();
    loop {
        match ask(
            client,
            Request::ReadTerminal {
                terminal_id: id.into(),
                after: *cursor,
            },
        )
        .await
        {
            Response::TerminalOutput {
                data,
                next,
                truncated,
                ..
            } => {
                assert!(!truncated);
                *cursor = next;
                bytes.extend(data);
                let output = String::from_utf8_lossy(&bytes);
                if output.contains(needle) {
                    return output.into_owned();
                }
            }
            r => panic!("{r:?}"),
        }
        assert!(
            Instant::now() < deadline,
            "missing {needle:?}, output: {:?}",
            String::from_utf8_lossy(&bytes)
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}
#[tokio::test]
async fn terminal_pty_cwd_resize_unicode_reconnect_interrupt_and_exit() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let project_id = terminal_project(&mut client, project.path()).await;
    let id = uuid::Uuid::new_v4().to_string();
    for _ in 0..2 {
        assert!(matches!(
            ask(
                &mut client,
                Request::CreateTerminal {
                    project_id: project_id.clone(),
                    terminal_id: id.clone(),
                    cols: 80,
                    rows: 24
                }
            )
            .await,
            Response::Terminal { .. }
        ));
    }
    assert!(
        matches!(ask(&mut client, Request::Terminals {project_id:project_id.clone()}).await, Response::Terminals {terminals} if terminals.len()==1)
    );
    let mut cursor = 0.0;
    terminal_write(
        &mut client,
        &id,
        b"stty -echo; printf '\\nREADY:%s\\n' yes\n",
    )
    .await;
    terminal_until(&mut client, &id, &mut cursor, "READY:yes\r\n").await;
    terminal_write(
        &mut client,
        &id,
        "test -t 0 && test -t 1 && printf 'TTY:%s\\n' yes; pwd; printf '中文🙂\\n'\n".as_bytes(),
    )
    .await;
    let output = terminal_until(&mut client, &id, &mut cursor, "中文🙂\r\n").await;
    assert!(output.contains("TTY:yes\r\n"));
    assert!(output.contains(project.path().canonicalize().unwrap().to_str().unwrap()));
    assert!(matches!(
        ask(
            &mut client,
            Request::ResizeTerminal {
                terminal_id: id.clone(),
                cols: 103,
                rows: 37
            }
        )
        .await,
        Response::Ok
    ));
    terminal_write(&mut client, &id, b"stty size\n").await;
    terminal_until(&mut client, &id, &mut cursor, "37 103\r\n").await;
    drop(client);
    let mut client = host.client().await;
    terminal_write(&mut client, &id, b"printf 'RECOVER:%s\\n' yes\n").await;
    terminal_until(&mut client, &id, &mut cursor, "RECOVER:yes\r\n").await;
    terminal_write(&mut client, &id, b"sleep 30\n").await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    terminal_write(&mut client, &id, b"\x03").await;
    terminal_write(&mut client, &id, b"printf 'INTERRUPTED:%s\\n' yes\n").await;
    terminal_until(&mut client, &id, &mut cursor, "INTERRUPTED:yes\r\n").await;
    terminal_write(&mut client, &id, b"exit 7\n").await;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = ask(
            &mut client,
            Request::ReadTerminal {
                terminal_id: id.clone(),
                after: cursor,
            },
        )
        .await;
        match response {
            Response::TerminalOutput { terminal, next, .. } => {
                cursor = next;
                if terminal.exited {
                    assert_eq!(terminal.exit_code, Some(7));
                    break;
                }
            }
            r => panic!("{r:?}"),
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    for _ in 0..2 {
        assert!(matches!(
            ask(
                &mut client,
                Request::CloseTerminal {
                    terminal_id: id.clone()
                }
            )
            .await,
            Response::Ok
        ));
    }
    assert!(matches!(
        ask(
            &mut client,
            Request::ReadTerminal {
                terminal_id: id,
                after: cursor
            }
        )
        .await,
        Response::Error { .. }
    ));
}
fn process_is_running(pid: Pid) -> bool {
    if kill(pid, None).is_err() {
        return false;
    }
    // kill(pid, 0) also succeeds for a terminated process awaiting reaping.
    // In particular, macOS runners can retain an orphaned zombie temporarily.
    let result = Command::new("/bin/ps")
        .args(["-p", &pid.as_raw().to_string(), "-o", "stat="])
        .output()
        .unwrap();
    let state = String::from_utf8(result.stdout).unwrap();
    if state.trim().is_empty() {
        assert_eq!(result.status.code(), Some(1), "process query failed");
        return false;
    }
    assert!(result.status.success(), "process query failed");
    !state.trim_start().starts_with('Z')
}
#[tokio::test]
async fn terminal_close_kills_shell_that_ignores_hangup() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let project_id = terminal_project(&mut client, project.path()).await;
    let id = uuid::Uuid::new_v4().to_string();
    assert!(matches!(
        ask(
            &mut client,
            Request::CreateTerminal {
                project_id,
                terminal_id: id.clone(),
                cols: 80,
                rows: 24,
            }
        )
        .await,
        Response::Terminal { .. }
    ));
    terminal_write(
        &mut client,
        &id,
        b"trap '' HUP; echo $$ > hangup.pid; while :; do sleep 1; done\n",
    )
    .await;
    let pid_file = project.path().join("hangup.pid");
    let deadline = Instant::now() + Duration::from_secs(5);
    let pid = loop {
        if let Ok(pid) = std::fs::read_to_string(&pid_file)
            && let Ok(pid) = pid.trim().parse::<i32>()
        {
            break Pid::from_raw(pid);
        }
        assert!(Instant::now() < deadline, "shell readiness timeout");
        tokio::time::sleep(Duration::from_millis(30)).await;
    };
    assert!(process_is_running(pid));
    assert!(matches!(
        ask(&mut client, Request::CloseTerminal { terminal_id: id }).await,
        Response::Ok
    ));
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_is_running(pid) {
        assert!(
            Instant::now() < deadline,
            "hangup-ignoring shell survived close"
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}
#[tokio::test]
async fn terminal_limits_project_isolation_and_close_foreground_process() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let project_id = terminal_project(&mut client, project.path()).await;
    let other_id = terminal_project(&mut client, other.path()).await;
    let id = uuid::Uuid::new_v4().to_string();
    for (project, cols) in [("missing".to_owned(), 80), (project_id.clone(), 0)] {
        assert!(matches!(
            ask(
                &mut client,
                Request::CreateTerminal {
                    project_id: project,
                    terminal_id: id.clone(),
                    cols,
                    rows: 24
                }
            )
            .await,
            Response::Error { .. }
        ));
    }
    assert!(matches!(
        ask(
            &mut client,
            Request::CreateTerminal {
                project_id: project_id.clone(),
                terminal_id: id.clone(),
                cols: 80,
                rows: 24
            }
        )
        .await,
        Response::Terminal { .. }
    ));
    assert!(matches!(
        ask(
            &mut client,
            Request::CreateTerminal {
                project_id: other_id.clone(),
                terminal_id: id.clone(),
                cols: 80,
                rows: 24
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(
        matches!(ask(&mut client,Request::Terminals {project_id:other_id}).await,Response::Terminals {terminals} if terminals.is_empty())
    );
    assert!(matches!(
        ask(
            &mut client,
            Request::WriteTerminal {
                terminal_id: id.clone(),
                data: vec![0; 16385]
            }
        )
        .await,
        Response::Error { .. }
    ));
    let pid_file = project.path().join("foreground.pid");
    // Exec keeps the foreground job PID, making close observable outside the PTY.
    terminal_write(
        &mut client,
        &id,
        b"sh -c 'echo $$ > foreground.pid; exec sleep 60'\n",
    )
    .await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !pid_file.exists() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let pid = Pid::from_raw(
        std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap(),
    );
    assert!(kill(pid, None).is_ok());
    ask(&mut client, Request::CloseTerminal { terminal_id: id }).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_is_running(pid) {
        assert!(
            Instant::now() < deadline,
            "foreground process survived close"
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    // A second PTY is independent and usable after the first has been terminated.
    let id = uuid::Uuid::new_v4().to_string();
    ask(
        &mut client,
        Request::CreateTerminal {
            project_id,
            terminal_id: id.clone(),
            cols: 80,
            rows: 24,
        },
    )
    .await;
    terminal_write(&mut client, &id, b"printf '\\nSECOND:%s\\n' yes\n").await;
    terminal_until(&mut client, &id, &mut 0.0, "SECOND:yes\r\n").await;
    ask(&mut client, Request::CloseTerminal { terminal_id: id }).await;
}

#[tokio::test]
async fn terminal_daemon_shutdown_cleans_up_and_restart_never_replays() {
    let mut host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let project_id = terminal_project(&mut client, project.path()).await;
    let id = uuid::Uuid::new_v4().to_string();
    ask(
        &mut client,
        Request::CreateTerminal {
            project_id: project_id.clone(),
            terminal_id: id.clone(),
            cols: 80,
            rows: 24,
        },
    )
    .await;
    terminal_write(
        &mut client,
        &id,
        b"echo $$ > shell.pid; printf '\\nLIVE:%s\\n' yes\n",
    )
    .await;
    terminal_until(&mut client, &id, &mut 0.0, "LIVE:yes\r\n").await;
    let pid = Pid::from_raw(
        std::fs::read_to_string(project.path().join("shell.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap(),
    );
    kill(Pid::from_raw(host.process.id() as i32), Signal::SIGTERM).unwrap();
    let deadline = Instant::now() + Duration::from_secs(6);
    while host.process.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "daemon shutdown timed out");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(
        kill(pid, None).is_err(),
        "PTY shell survived daemon shutdown"
    );
    drop(client);
    host.process = Command::new(env!("CARGO_BIN_EXE_latte-work-server"))
        .env("LATTE_WORK_AGENT_ENV", "inherit")
        .arg("serve")
        .arg("--state-dir")
        .arg(host.directory.path())
        .env("LATTE_WORK_CLAUDE", "/missing-test-claude")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !host.directory.path().join("control.sock").exists() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let mut client = host.client().await;
    assert!(
        matches!(ask(&mut client,Request::Terminals {project_id}).await,Response::Terminals {terminals} if terminals.is_empty())
    );
    assert!(matches!(
        ask(
            &mut client,
            Request::ReadTerminal {
                terminal_id: id.clone(),
                after: 0.0
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(matches!(
        ask(
            &mut client,
            Request::WriteTerminal {
                terminal_id: id,
                data: b"touch replayed\n".to_vec()
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(!project.path().join("replayed").exists());
}

#[tokio::test]
async fn completed_turn_is_unread_until_acknowledged_across_clients() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    send(&mut client, &id, "unread-turn", "hello").await;
    wait(&mut client, &id, Status::Completed).await;
    drop(client);
    let mut client = host.client().await;
    assert!(matches!(
        ask(&mut client, Request::Poll { session_id: id.clone(), after: 0.0 }).await,
        Response::Events { session, .. } if session.unread && session.status == Status::Completed
    ));
    assert!(matches!(
        ask(&mut client, Request::MarkSessionUnread { session_id: id.clone(), unread: false }).await,
        Response::Session { session } if !session.unread
    ));
    let mut other = host.client().await;
    assert!(matches!(
        ask(&mut other, Request::Poll { session_id: id, after: 0.0 }).await,
        Response::Events { session, .. } if !session.unread
    ));
}

async fn lifecycle(path: &Path, request: serde_json::Value) -> serde_json::Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut stream = tokio::net::UnixStream::connect(path.join("control.sock"))
            .await
            .unwrap();
        stream
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).await.unwrap();
        serde_json::from_str(&line).unwrap()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn upgrade_refuses_tasks_and_terminals_then_drains_existing_clients() {
    use serde_json::json;
    let mut host = Host::start().await;
    let mut client = host.client().await;
    let dir = host.directory.path();
    let status = lifecycle(dir, json!({"method":"server_status"})).await;
    assert_eq!(status["build_id"].as_str().unwrap().len(), 64);
    let prepare = json!({"method":"prepare_upgrade", "server_id":status["server_id"]});
    let stale = lifecycle(dir, json!({"method":"prepare_upgrade","server_id":"stale"})).await;
    assert_eq!(stale["kind"], "error");
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    send(&mut client, &id, "upgrade-approval", "approve").await;
    wait(&mut client, &id, Status::Waiting).await;
    assert_eq!(
        lifecycle(dir, prepare.clone()).await["code"],
        "upgrade_blocked"
    );
    assert_eq!(
        lifecycle(dir, json!({"method":"server_status"})).await["draining"],
        false
    );
    assert!(host.process.try_wait().unwrap().is_none());
    ask(
        &mut client,
        Request::Cancel {
            session_id: id.clone(),
        },
    )
    .await;
    wait(&mut client, &id, Status::Stopped).await;
    let project_id = terminal_project(&mut client, project.path()).await;
    let terminal_id = uuid::Uuid::new_v4().to_string();
    assert!(matches!(
        ask(
            &mut client,
            Request::CreateTerminal {
                project_id,
                terminal_id: terminal_id.clone(),
                cols: 80,
                rows: 24
            }
        )
        .await,
        Response::Terminal { .. }
    ));
    assert_eq!(
        lifecycle(dir, prepare.clone()).await["code"],
        "upgrade_blocked"
    );
    // Refused upgrades must leave the real shell usable.
    terminal_write(&mut client, &terminal_id, b"printf 'upgrade:%s\\n' alive\n").await;
    terminal_until(&mut client, &terminal_id, &mut 0.0, "upgrade:alive").await;
    ask(&mut client, Request::CloseTerminal { terminal_id }).await;
    assert_eq!(lifecycle(dir, prepare).await["kind"], "upgrade_ready");
    assert!(
        matches!(send(&mut client, &id, "must-not-start", "hello").await, Response::Error { code, .. } if code == "server_upgrading")
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.process.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn upgrade_local_coordinator_replaces_idle_build_once_for_concurrent_clients() {
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let directory = tempfile::Builder::new()
        .prefix("lw-upgrade-")
        .tempdir_in("/tmp")
        .unwrap();
    let dir = directory.path().to_path_buf();
    // A deterministic old-daemon fixture holds the same real singleton lock.
    let guard = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("server.lock"))
        .unwrap();
    guard.try_lock().unwrap();
    let listener = tokio::net::UnixListener::bind(dir.join("control.sock")).unwrap();
    let old_dir = dir.clone();
    let old = tokio::spawn(async move {
        for expected in ["server_status", "prepare_upgrade"] {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut line = String::new();
            BufReader::new(reader).read_line(&mut line).await.unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], expected);
            let response = if expected == "server_status" {
                json!({"kind":"server_status","server_id":"old-instance","build_id":"old-build","draining":false})
            } else {
                assert_eq!(request["server_id"], "old-instance");
                json!({"kind":"upgrade_ready"})
            };
            writer
                .write_all(format!("{response}\n").as_bytes())
                .await
                .unwrap();
        }
        drop(listener);
        std::fs::remove_file(old_dir.join("control.sock")).unwrap();
        drop(guard);
    });
    // Spawn bridges with a fixture CLI even on machines with a real Claude installation.
    async fn connect(dir: &Path) -> (tokio::process::Child, serde_json::Value) {
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_latte-work-server"))
            .args(["connect-local", "--state-dir"])
            .arg(dir)
            .env(
                "LATTE_WORK_CLAUDE",
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude.py"),
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(format!("{{\"method\":\"hello\",\"version\":{VERSION}}}\n").as_bytes())
            .await
            .unwrap();
        let mut line = String::new();
        tokio::time::timeout(
            Duration::from_secs(22),
            BufReader::new(child.stdout.take().unwrap()).read_line(&mut line),
        )
        .await
        .unwrap()
        .unwrap();
        (child, serde_json::from_str(&line).unwrap())
    }
    let ((a, hello_a), (b, hello_b)) = tokio::join!(connect(&dir), connect(&dir));
    old.await.unwrap();
    let status = lifecycle(&dir, json!({"method":"server_status"})).await;
    // Stop the disposable daemon before assertions, including on a failed comparison.
    let stop = lifecycle(
        &dir,
        json!({"method":"prepare_upgrade","server_id":status["server_id"]}),
    )
    .await;
    drop((a, b));
    assert_eq!(stop["kind"], "upgrade_ready");
    let deadline = Instant::now() + Duration::from_secs(5);
    while dir.join("control.sock").exists() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(hello_a["kind"], "hello", "{hello_a}");
    assert_eq!(hello_a["server_id"], hello_b["server_id"]);
    assert_eq!(hello_a["version"], 1);
    assert_ne!(status["build_id"], "old-build");
}

#[tokio::test]
async fn upgrade_legacy_daemon_returns_actionable_error_without_stopping_it() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let directory = tempfile::Builder::new()
        .prefix("lw-legacy-")
        .tempdir_in("/tmp")
        .unwrap();
    let listener = tokio::net::UnixListener::bind(directory.path().join("control.sock")).unwrap();
    let old = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut line = String::new();
        BufReader::new(reader).read_line(&mut line).await.unwrap();
        assert!(line.contains("server_status"));
        writer
            .write_all(
                b"{\"kind\":\"error\",\"code\":\"invalid_request\",\"message\":\"old server\"}\n",
            )
            .await
            .unwrap();
        listener
    });
    let result = Client::local(
        Path::new(env!("CARGO_BIN_EXE_latte-work-server")),
        Some(directory.path()),
    )
    .await;
    let error = match result {
        Ok(_) => panic!("must reject legacy daemon"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("旧版本机后台"), "{error}");
    assert!(error.contains("仅重启桌面应用不会停止后台"));
    let listener = old.await.unwrap();
    assert!(directory.path().join("control.sock").exists());
    assert!(
        tokio::net::UnixStream::connect(directory.path().join("control.sock"))
            .await
            .is_ok()
    );
    drop(listener);
}

#[tokio::test]
async fn upgrade_racing_send_either_preserves_task_or_rejects_before_launch() {
    use serde_json::json;
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    let status = lifecycle(host.directory.path(), json!({"method":"server_status"})).await;
    let (upgrade, sent) = tokio::join!(
        lifecycle(
            host.directory.path(),
            json!({"method":"prepare_upgrade", "server_id":status["server_id"]})
        ),
        send(&mut client, &id, "race-upgrade", "approve")
    );
    match sent {
        Response::Accepted { duplicate: false } => {
            assert_eq!(upgrade["code"], "upgrade_blocked");
            wait(&mut client, &id, Status::Waiting).await;
            ask(
                &mut client,
                Request::Cancel {
                    session_id: id.clone(),
                },
            )
            .await;
            wait(&mut client, &id, Status::Stopped).await;
        }
        Response::Error { code, .. } => {
            assert_eq!(code, "server_upgrading");
            assert_eq!(upgrade["kind"], "upgrade_ready");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn agent_commands_discovery_dispatch_resume_and_unsupported_are_native() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let folder = tempfile::tempdir().unwrap();
    let project_id = terminal_project(&mut client, folder.path()).await;
    let response = ask(
        &mut client,
        Request::AgentCommands {
            agent: "claude".into(),
            project_id: project_id.clone(),
        },
    )
    .await;
    match response {
        Response::AgentCommands { commands } => {
            assert_eq!(commands.len(), 3);
            assert_eq!(commands[0].name, "compact");
            assert_eq!(commands[0].argument_hint, "[instructions]");
            assert_eq!(
                commands[1].description,
                folder.path().file_name().unwrap().to_str().unwrap()
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(
        matches!(ask(&mut client, Request::Sessions { project_id: project_id.clone() }).await, Response::Sessions { sessions } if sessions.is_empty())
    );
    for (agent, project) in [("codex", project_id.as_str()), ("claude", "missing")] {
        assert!(matches!(
            ask(
                &mut client,
                Request::AgentCommands {
                    agent: agent.into(),
                    project_id: project.into()
                }
            )
            .await,
            Response::Error { .. }
        ));
    }
    let id = session(&mut client, folder.path()).await;
    assert!(matches!(
        send(
            &mut client,
            &id,
            "first-command",
            "/project:check 中文 参数"
        )
        .await,
        Response::Accepted { duplicate: false }
    ));
    let events = wait(&mut client, &id, Status::Completed).await;
    assert!(events.iter().any(|event| matches!(&event.event, EventKind::Text { text } if text == "fresh:/project:check 中文 参数")));
    assert!(matches!(
        send(
            &mut client,
            &id,
            "first-command",
            "/project:check 中文 参数"
        )
        .await,
        Response::Accepted { duplicate: true }
    ));
    send(&mut client, &id, "second-command", "/compact 保留关键内容").await;
    let events = wait(&mut client, &id, Status::Completed).await;
    assert!(events.iter().any(|event| matches!(&event.event, EventKind::Text { text } if text == "resumed:/compact 保留关键内容")));
    send(&mut client, &id, "unknown-command", "/not-supported").await;
    let events = wait(&mut client, &id, Status::Failed).await;
    assert!(events.iter().any(|event| matches!(&event.event, EventKind::State { message: Some(message), .. } if message.contains("不支持命令"))));
    assert!(!events.iter().any(
        |event| matches!(&event.event, EventKind::Text { text } if text.contains("not-supported"))
    ));
}

#[tokio::test]
async fn native_child_transcript_is_cached_without_reopening_an_agent_or_replaying_a_prompt() {
    let host = Host::start_with_environment(&[(
        "LATTE_FIXTURE_CHILD_NATIVE_FILE",
        std::ffi::OsStr::new("1"),
    )])
    .await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    send(&mut client, &id, "native-child-process", "subagent-process").await;
    wait(&mut client, &id, Status::Completed).await;
    let Response::Subagents { tasks, .. } = ask(
        &mut client,
        Request::Subagents {
            session_id: id.clone(),
        },
    )
    .await
    else {
        panic!("tasks")
    };
    let task = &tasks[0];
    let read = Request::SubagentTranscript {
        session_id: id.clone(),
        task_id: task.id.clone(),
    };
    // Parent completion precedes the child's notification and the native idle
    // frame. Wait for the actor's idle guard rather than assuming 300ms is enough
    // on a loaded CI runner. Never close an active fixture by overriding it.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match ask(
            &mut client,
            Request::CloseAgentSession {
                session_id: id.clone(),
                only_if_idle: Some(true),
            },
        )
        .await
        {
            Response::Ok => break,
            Response::Error { message, .. } if message.contains("请确认后关闭") => {
                assert!(
                    Instant::now() < deadline,
                    "fixture did not become idle: {message}"
                );
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            response => panic!("unexpected idle-close response: {response:?}"),
        }
    }
    let Response::SubagentTranscript { events, truncated } = ask(&mut client, read.clone()).await
    else {
        panic!("native child")
    };
    assert!(!truncated);
    assert!(events.iter().any(
        |e| matches!(&e.event, EventKind::Text { text } if text == "text only in native file")
    ));
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, EventKind::Text { text } if text == "native file final"))
    );
    let mut reconnected = host.client().await;
    let Response::SubagentTranscript {
        events: restored, ..
    } = ask(&mut reconnected, read).await
    else {
        panic!("cached child")
    };
    assert_eq!(events.len(), restored.len());
    assert!(
        matches!(ask(&mut reconnected, Request::Poll { session_id: id, after: 0.0 }).await, Response::Events { session, events, .. } if session.agent_session_open == Some(false) && events.iter().filter(|e| matches!(e.event, EventKind::User { .. })).count() == 1)
    );
}

#[tokio::test]
async fn excess_child_streams_keep_parent_alive_and_preserve_complete_child_text() {
    async fn completed_history(client: &mut Client, id: &str) -> Vec<latte_work_protocol::Event> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let Response::History {
                session, events, ..
            } = ask(
                client,
                Request::History {
                    session_id: id.into(),
                    before: None,
                },
            )
            .await
            else {
                panic!("history response");
            };
            assert_ne!(
                session.status,
                Status::Failed,
                "fixture failed: {:?}",
                events
                    .iter()
                    .filter_map(|e| match &e.event {
                        EventKind::Notice { text } => Some(text),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            );
            if session.status == Status::Completed {
                return events;
            }
            assert!(Instant::now() < deadline, "fixture did not complete");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    send(&mut client, &id, "child-overflow", "child-stream-overflow").await;
    let events = completed_history(&mut client, &id).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, EventKind::Text { text } if text == "OVERFLOW_PARENT_OK"))
    );
    let Response::SubagentHistory { events, .. } = ask(
        &mut client,
        Request::SubagentHistory {
            session_id: id.clone(),
            task_id: "overflow-512".into(),
            after: None,
            source: None,
            provider: None,
        },
    )
    .await
    else {
        panic!("child history");
    };
    let text: String = events
        .iter()
        .filter_map(|e| match &e.event {
            EventKind::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "prefixtail");
    // The same persistent native actor can accept a subsequent turn.
    send(&mut client, &id, "after-overflow", "identity").await;
    let events = completed_history(&mut client, &id).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, EventKind::Text { text } if text.contains("\"turns\": 2")))
    );
}

#[tokio::test]
async fn subagent_process_history_survives_reconnect_without_entering_parent_transcript() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    send(&mut client, &id, "child-process", "subagent-process").await;
    let parent = wait(&mut client, &id, Status::Completed).await;
    assert!(
        !parent.iter().any(
            |e| matches!(&e.event, EventKind::Text { text } if text.contains("child progress"))
        )
    );
    assert!(
        !parent
            .iter()
            .any(|e| matches!(&e.event, EventKind::Tool { name, .. } if name == "Read"))
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let task = loop {
        let Response::Subagents { tasks, .. } = ask(
            &mut client,
            Request::Subagents {
                session_id: id.clone(),
            },
        )
        .await
        else {
            panic!("tasks")
        };
        if let Some(task) = tasks
            .into_iter()
            .find(|t| t.status == latte_work_protocol::SubagentStatus::Completed)
        {
            break task;
        }
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(task.native_id, "process-native");
    let mut reconnected = host.client().await;
    let Response::SubagentTranscript { events, truncated } = ask(
        &mut reconnected,
        Request::SubagentTranscript {
            session_id: id.clone(),
            task_id: task.id,
        },
    )
    .await
    else {
        panic!("child transcript")
    };
    assert!(!truncated);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(&e.event, EventKind::Text { text } if text == "child progress"))
            .count(),
        1
    );
    assert!(events.iter().any(|e| matches!(&e.event, EventKind::Tool { name, input, .. } if name == "Read" && input["file_path"] == "child.txt")));
    assert!(events.iter().any(|e| matches!(&e.event, EventKind::ToolResult { content, .. } if content == "child read output")));
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, EventKind::Text { text } if text == "child final"))
    );
    assert!(matches!(
        ask(
            &mut reconnected,
            Request::SubagentTranscript {
                session_id: id.clone(),
                task_id: "unrelated".into()
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(
        matches!(ask(&mut reconnected, Request::Poll { session_id: id, after: 0.0 }).await, Response::Events { session, .. } if session.status == Status::Completed)
    );
}

#[tokio::test]
async fn permission_hidden_help_still_accepts_first_and_followup_sends() {
    let host = Host::start_with_environment(&[(
        "LATTE_FIXTURE_PERMISSION_HELP",
        std::ffi::OsStr::new("hidden"),
    )])
    .await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    assert!(matches!(
        ask(&mut client, Request::AgentPermissions { agent: "claude".into() }).await,
        Response::AgentPermissions { modes } if modes.len() == 6 && modes.iter().any(|m| m.id == "manual") && modes.iter().any(|m| m.id == "bypassPermissions" && m.elevated)
    ));
    for request_id in ["first", "followup"] {
        let request = Request::Send {
            provider: None,
            session_id: id.clone(),
            request_id: request_id.into(),
            text: "permission".into(),
            model: None,
            effort: None,
            permission_mode: Some("bypassPermissions".into()),
        };
        assert!(matches!(
            ask(&mut client, request.clone()).await,
            Response::Accepted { duplicate: false }
        ));
        let events = wait(&mut client, &id, Status::Completed).await;
        assert!(events.iter().any(
            |e| matches!(&e.event, EventKind::Text { text } if text.ends_with(":bypassPermissions"))
        ));
        assert!(matches!(
            ask(&mut client, request).await,
            Response::Accepted { duplicate: true }
        ));
        assert_eq!(events.iter().filter(|e| matches!(&e.event, EventKind::User { request_id: recorded, .. } if recorded == request_id)).count(), 1);
    }
}

#[tokio::test]
async fn permission_hidden_help_without_native_choices_remains_unsupported() {
    let host = Host::start_with_environment(&[(
        "LATTE_FIXTURE_PERMISSION_HELP",
        std::ffi::OsStr::new("unsupported"),
    )])
    .await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    assert!(matches!(
        ask(
            &mut client,
            Request::AgentPermissions {
                agent: "claude".into()
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(matches!(
        ask(
            &mut client,
            Request::Send {
                provider: None,
                session_id: id.clone(),
                request_id: "unsupported".into(),
                text: "must not send".into(),
                model: None,
                effort: None,
                permission_mode: Some("bypassPermissions".into()),
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(
        matches!(ask(&mut client, Request::Poll { session_id: id, after: 0.0 }).await,
        Response::Events { events, .. } if !events.iter().any(|e| matches!(e.event, EventKind::User { .. })))
    );
}

#[tokio::test]
async fn permission_discovery_launch_resume_reset_and_approval_stay_native() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    std::fs::create_dir(project.path().join(".claude")).unwrap();
    std::fs::write(
        project.path().join(".claude/settings.json"),
        r#"{"permissions":{"defaultMode":"dontAsk"}}"#,
    )
    .unwrap();
    assert!(matches!(
        ask(&mut client, Request::AgentPermissions { agent: "claude".into() }).await,
        Response::AgentPermissions { modes } if modes.len() == 5 && !modes.iter().any(|m| m.id == "auto")
    ));
    assert!(matches!(
        ask(
            &mut client,
            Request::AgentPermissions {
                agent: "codex".into()
            }
        )
        .await,
        Response::Error { .. }
    ));
    let turn = |request_id: &str, mode: Option<&str>, text: &str| Request::Send {
        provider: None,
        session_id: id.clone(),
        request_id: request_id.into(),
        text: text.into(),
        model: None,
        effort: None,
        permission_mode: mode.map(str::to_owned),
    };
    assert!(matches!(
        ask(&mut client, turn("bad-mode", Some("auto"), "permission")).await,
        Response::Error { .. }
    ));
    assert!(
        matches!(ask(&mut client, Request::Poll { session_id: id.clone(), after: 0.0 }).await,
        Response::Events { events, session, .. } if events.is_empty() && session.permission_mode.is_none())
    );
    for (request_id, mode, expected) in [
        ("native", None, "fresh:dontAsk"),
        ("edits", Some("acceptEdits"), "resumed:acceptEdits"),
        ("plan", Some("plan"), "resumed:plan"),
        (
            "bypass",
            Some("bypassPermissions"),
            "resumed:bypassPermissions",
        ),
        ("reset", None, "resumed:dontAsk"),
    ] {
        let request = turn(request_id, mode, "permission");
        assert!(matches!(
            ask(&mut client, request.clone()).await,
            Response::Accepted { duplicate: false }
        ));
        let events = wait(&mut client, &id, Status::Completed).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(&e.event, EventKind::Text { text } if text == expected))
        );
        assert!(matches!(
            ask(&mut client, request).await,
            Response::Accepted { duplicate: true }
        ));
        assert!(matches!(
            ask(&mut client, turn(request_id, Some("default"), "permission")).await,
            Response::Error { .. }
        ));
        let mut reconnect = host.client().await;
        assert!(
            matches!(ask(&mut reconnect, Request::Poll { session_id: id.clone(), after: 0.0 }).await,
            Response::Events { session, .. } if session.permission_mode.as_deref() == mode)
        );
    }
    // Even if an Agent asks while bypass is selected, the host never fabricates approval.
    assert!(matches!(
        ask(
            &mut client,
            turn("approval", Some("bypassPermissions"), "approve")
        )
        .await,
        Response::Accepted { .. }
    ));
    let events = wait(&mut client, &id, Status::Waiting).await;
    assert!(!project.path().join("approved.txt").exists());
    ask(
        &mut client,
        Request::Approve {
            session_id: id.clone(),
            request_id: permission(&events),
            allow: false,
        },
    )
    .await;
    wait(&mut client, &id, Status::Completed).await;
    assert!(!project.path().join("approved.txt").exists());
}

#[tokio::test]
async fn pasted_attachments_are_chunked_host_owned_and_abortable() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let id = match ask(
        &mut client,
        Request::BeginAttachment {
            name: "image.png".into(),
            size: 70000,
        },
    )
    .await
    {
        Response::AttachmentUpload { id } => id,
        other => panic!("{other:?}"),
    };
    assert!(matches!(
        ask(
            &mut client,
            Request::AttachmentChunk {
                id: id.clone(),
                offset: 1,
                data: vec![1]
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(matches!(
        ask(&mut client, Request::FinishAttachment { id: id.clone() }).await,
        Response::Error { .. }
    ));
    for (offset, data) in [(0, vec![42; 65536]), (65536, vec![42; 4464])] {
        assert!(matches!(
            ask(
                &mut client,
                Request::AttachmentChunk {
                    id: id.clone(),
                    offset,
                    data
                }
            )
            .await,
            Response::Ok
        ));
    }
    let entry = match ask(&mut client, Request::FinishAttachment { id }).await {
        Response::FileReference { entry } => entry,
        other => panic!("{other:?}"),
    };
    assert!(
        Path::new(&entry.path).starts_with(
            host.directory
                .path()
                .canonicalize()
                .unwrap()
                .join("paste-tmp")
        )
    );
    assert_eq!(std::fs::read(&entry.path).unwrap(), vec![42; 70000]);
    let second = Host::start().await;
    let mut remote = second.client().await;
    let id = match ask(
        &mut remote,
        Request::BeginAttachment {
            name: "remote.txt".into(),
            size: 0,
        },
    )
    .await
    {
        Response::AttachmentUpload { id } => id,
        other => panic!("{other:?}"),
    };
    assert!(matches!(
        ask(&mut client, Request::FinishAttachment { id: id.clone() }).await,
        Response::Error { .. }
    ));
    assert!(matches!(
        ask(&mut remote, Request::AbortAttachment { id: id.clone() }).await,
        Response::Ok
    ));
    assert!(matches!(
        ask(&mut remote, Request::FinishAttachment { id }).await,
        Response::Error { .. }
    ));
    assert!(matches!(
        ask(
            &mut client,
            Request::BeginAttachment {
                name: "../escape".into(),
                size: 0
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(matches!(
        ask(
            &mut client,
            Request::BeginAttachment {
                name: "huge".into(),
                size: 64 * 1024 * 1024 + 1
            }
        )
        .await,
        Response::Error { .. }
    ));
}

/// The App owns the catalog. No local Server/client is constructed in this flow.
#[tokio::test]
async fn app_providers_and_native_defaults_only_contact_the_selected_host() {
    use latte_work_client::request_with_app_provider;
    use latte_work_config::ProviderStore;
    use latte_work_protocol::{ProviderAuth, ProviderDraft, ProviderProtocol};
    use tokio::sync::Mutex;
    let app = tempfile::tempdir().unwrap();
    let first = Host::start().await;
    let second = Host::start().await;
    let draft = ProviderDraft {
        id: None,
        name: "App-only".into(),
        protocol: ProviderProtocol::AnthropicMessages,
        base_url: "https://example.test".into(),
        model: "first".into(),
        models: vec![],
        model_labels: Default::default(),
        auth: ProviderAuth::ApiKey,
        credential: Some("fixture-private-key".into()),
    };
    let id = {
        let mut store = ProviderStore::open(app.path(), None).unwrap();
        let Response::Providers { providers, .. } = store.save(draft.clone()).unwrap() else {
            panic!()
        };
        let id = providers[0].id.clone();
        // A broken/offline local host's explicit Provider must not affect either remote default.
        store.bind("local", "claude", Some(id.clone())).unwrap();
        id
    };
    let app_path = app.path();
    let models = || Request::Models {
        agent: "claude".into(),
        model: None,
        project_id: None,
    };
    for (host, host_id, name) in [
        (&first, "first-host", "First native"),
        (&second, "second-host", "Second native"),
    ] {
        let config = host.directory.path().join("claude-config");
        std::fs::create_dir(&config).unwrap();
        std::fs::write(
            config.join("settings.json"),
            serde_json::json!({"env": {"ANTHROPIC_DEFAULT_SONNET_MODEL": "native-id", "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME": name}}).to_string(),
        )
        .unwrap();
        let target = Mutex::new(host.client().await);
        let response = request_with_app_provider(&target, models(), |agent| async move {
            ProviderStore::open(app_path, None)?.snapshot_for_host(host_id, &agent)
        })
        .await
        .unwrap();
        assert!(
            matches!(response, Response::Models { provider: None, model_labels, .. } if model_labels.get("sonnet").map(String::as_str) == Some(name))
        );
    }
    // Kill one environment entirely; the other still supports native and custom turns.
    drop(first);
    let target = Mutex::new(second.client().await);
    let resolve = |agent: String| async move {
        ProviderStore::open(app_path, None)?.snapshot_for_host("second-host", &agent)
    };
    let mut observer = second.client().await;
    let project = tempfile::tempdir().unwrap();
    let sid = session(&mut observer, project.path()).await;
    let send = |request_id: &str, text: &str| Request::Send {
        session_id: sid.clone(),
        request_id: request_id.into(),
        text: text.into(),
        model: None,
        effort: None,
        permission_mode: None,
        provider: None,
    };
    assert!(matches!(
        request_with_app_provider(&target, send("native", "model"), resolve)
            .await
            .unwrap(),
        Response::Accepted { .. }
    ));
    assert!(
        wait(&mut observer, &sid, Status::Completed)
            .await
            .iter()
            .any(|e| matches!(&e.event, EventKind::Text { text } if text == "fresh:cli-config"))
    );
    ProviderStore::open(app.path(), None)
        .unwrap()
        .bind("second-host", "claude", Some(id.clone()))
        .unwrap();
    assert!(
        matches!(request_with_app_provider(&target, models(), resolve).await.unwrap(), Response::Models { default_model: Some(model), .. } if model == "first")
    );
    assert!(matches!(
        request_with_app_provider(&target, send("custom", "provider"), resolve)
            .await
            .unwrap(),
        Response::Accepted { .. }
    ));
    let events = wait(&mut observer, &sid, Status::Completed).await;
    assert!(
        events.iter().any(
            |e| matches!(&e.event, EventKind::Text { text } if text == "resumed:first:api_key")
        )
    );
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("fixture-private-key")
    );
    // An App edit is used for the next turn, including after reconnect.
    let mut edit = draft.clone();
    edit.id = Some(id);
    edit.model = "second".into();
    edit.credential = None;
    ProviderStore::open(app.path(), None)
        .unwrap()
        .save(edit)
        .unwrap();
    let target = Mutex::new(second.client().await);
    assert!(matches!(
        request_with_app_provider(&target, send("updated", "provider"), resolve)
            .await
            .unwrap(),
        Response::Accepted { .. }
    ));
    assert!(
        wait(&mut observer, &sid, Status::Completed)
            .await
            .iter()
            .any(
                |e| matches!(&e.event, EventKind::Text { text } if text == "resumed:second:api_key")
            )
    );
    assert!(!second.directory.path().join("providers.json").exists());
    // A failed config read must not silently fall back or submit the request.
    assert!(
        request_with_app_provider(&target, send("not-sent", "model"), |_| async {
            anyhow::bail!("App config invalid")
        })
        .await
        .is_err()
    );
    let Response::Session { session: before } = observer
        .request(Request::Session {
            session_id: sid.clone(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(before.status, Status::Completed);
    // Explicit CLI defaults also bypass a stale legacy binding on the target.
    let Response::Providers { providers, .. } = observer
        .request(Request::SaveProvider { provider: draft })
        .await
        .unwrap()
    else {
        panic!()
    };
    observer
        .request(Request::BindAgentProvider {
            agent: "claude".into(),
            provider_id: Some(providers[0].id.clone()),
            target: None,
        })
        .await
        .unwrap();
    ProviderStore::open(app.path(), None)
        .unwrap()
        .bind("second-host", "claude", None)
        .unwrap();
    assert!(
        matches!(request_with_app_provider(&target, models(), resolve).await.unwrap(), Response::Models { provider: None, model_labels, .. } if model_labels.get("sonnet").map(String::as_str) == Some("Second native"))
    );
    assert!(matches!(
        request_with_app_provider(&target, send("native-again", "model"), resolve)
            .await
            .unwrap(),
        Response::Accepted { .. }
    ));
    assert!(
        wait(&mut observer, &sid, Status::Completed)
            .await
            .iter()
            .any(|e| matches!(&e.event, EventKind::Text { text } if text == "resumed:default"))
    );
}

#[tokio::test]
async fn usage_snapshot_survives_bridge_reconnect_without_billing_inflation() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    assert!(matches!(
        send(&mut client, &id, "usage-request", "usage").await,
        Response::Accepted { duplicate: false }
    ));
    let events = wait(&mut client, &id, Status::Completed).await;
    assert!(events.iter().any(|e| matches!(
        e.event,
        EventKind::Progress {
            phase: latte_work_protocol::ExecutionPhase::Thinking
        }
    )));
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("private fixture reasoning")
    );
    let latest = events
        .iter()
        .rev()
        .find(|e| matches!(e.event, EventKind::Usage { .. }))
        .unwrap();
    assert!(
        matches!(&latest.event,EventKind::Usage {context:Some(c),totals:Some(t)} if c.used_tokens == 1000.0 && c.window_tokens == Some(200000.0) && t.input_tokens == Some(200.0) && t.cache_read_tokens == Some(1200.0))
    );
    drop(client);
    let mut reconnected = host.client().await;
    let replay = wait(&mut reconnected, &id, Status::Completed).await;
    assert_eq!(
        serde_json::to_value(&events).unwrap(),
        serde_json::to_value(&replay).unwrap()
    );
}

#[tokio::test]
async fn usage_alias_capacity_is_persisted_and_replayed() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    assert!(matches!(
        ask(
            &mut client,
            Request::Send {
                session_id: id.clone(),
                request_id: "usage-alias-request".into(),
                text: "usage-alias".into(),
                model: Some("relay/seed-0812".into()),
                provider: Some(latte_work_protocol::TurnProvider::Snapshot(
                    serde_json::from_value(serde_json::json!({
                        "provider": {
                            "id": "usage-provider", "name": "Usage fixture",
                            "protocol": "anthropic_messages", "base_url": "https://example.test",
                            "model": "relay/seed-0812", "models": [], "model_labels": {},
                            "auth": "bearer", "has_credential": true, "revision": "fixture"
                        },
                        "credential": "fixture-only-key"
                    }))
                    .unwrap()
                )),
                effort: None,
                permission_mode: None,
            }
        )
        .await,
        Response::Accepted { duplicate: false }
    ));
    let events = wait(&mut client, &id, Status::Completed).await;
    let latest = events
        .iter()
        .rev()
        .find(|e| matches!(e.event, EventKind::Usage { .. }))
        .unwrap();
    assert!(
        matches!(&latest.event, EventKind::Usage {context:Some(c),..} if c.model == "model_api/experimental_0812" && c.used_tokens == 69692.0 && c.window_tokens == Some(200000.0))
    );
    drop(client);
    let mut reconnected = host.client().await;
    let replay = wait(&mut reconnected, &id, Status::Completed).await;
    assert_eq!(
        serde_json::to_value(&events).unwrap(),
        serde_json::to_value(&replay).unwrap()
    );
}

#[tokio::test]
async fn native_idle_shutdown_preserves_busy_terminal_then_exits_after_close() {
    let mut host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let project_id = terminal_project(&mut client, project.path()).await;
    let id = uuid::Uuid::new_v4().to_string();
    assert!(matches!(
        ask(
            &mut client,
            Request::CreateTerminal {
                project_id,
                terminal_id: id.clone(),
                cols: 80,
                rows: 24
            }
        )
        .await,
        Response::Terminal { .. }
    ));
    assert!(!client.shutdown_if_idle().await.unwrap());
    terminal_write(
        &mut client,
        &id,
        b"echo $$ > shell.pid; printf '\\nIDLE:%s\\n' alive\n",
    )
    .await;
    terminal_until(&mut client, &id, &mut 0.0, "IDLE:alive\r\n").await;
    let pid = Pid::from_raw(
        std::fs::read_to_string(project.path().join("shell.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap(),
    );
    assert!(matches!(
        ask(&mut client, Request::CloseTerminal { terminal_id: id }).await,
        Response::Ok
    ));
    assert!(client.shutdown_if_idle().await.unwrap());
    let deadline = Instant::now() + Duration::from_secs(6);
    while host.process.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(
        kill(pid, None).is_err(),
        "terminal shell survived idle shutdown"
    );
}

#[tokio::test]
async fn agents_inherit_shell_exports_with_a_minimal_daemon_path() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let shell = dir.path().join("bash");
    let tool = dir.path().join("latte-env-fixture");
    std::fs::write(
        &tool,
        r#"#!/bin/sh
printf '%s' "$LATTE_TEST_VALUE"
"#,
    )
    .unwrap();
    std::fs::write(&shell, format!("#!/bin/sh\nexport PATH='{}':/usr/bin:/bin\nexport LATTE_TEST_VALUE='shell-loaded'\necho startup-banner\nexec /bin/sh -c \"$2\"\n", dir.path().display())).unwrap();
    for path in [&shell, &tool] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let host = Host::start_with_environment(&[
        ("SHELL", shell.as_os_str()),
        ("PATH", std::ffi::OsStr::new("/usr/bin:/bin")),
        ("LATTE_WORK_AGENT_ENV", std::ffi::OsStr::new("shell")),
    ])
    .await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    assert!(matches!(
        send(&mut client, &id, "env-request", "environment").await,
        Response::Accepted { .. }
    ));
    let events = wait(&mut client, &id, Status::Completed).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, EventKind::Text {text} if text == "shell-loaded"))
    );
}

fn agent_pid(project: &Path) -> i32 {
    std::fs::read_to_string(project.join("fixture-agent-pid"))
        .unwrap()
        .parse()
        .unwrap()
}
async fn assert_gone(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while kill(Pid::from_raw(pid), None).is_ok() {
        assert!(
            Instant::now() < deadline,
            "owned process {pid} survived close"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
#[tokio::test]
async fn persistent_sessions_survive_turns_transport_and_interrupt_until_explicit_close() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let other_project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    let other = session(&mut client, other_project.path()).await;
    for id in [&id, &other] {
        assert!(matches!(
            ask(
                &mut client,
                Request::OpenAgentSession {
                    session_id: id.clone(),
                    provider: None
                }
            )
            .await,
            Response::Ok
        ));
        assert!(
            matches!(ask(&mut client, Request::Poll { session_id: id.clone(), after: 0.0 }).await, Response::Events { session, events, .. } if session.status == Status::Ready && events.is_empty() && session.agent_session_open == Some(true))
        );
    }
    let pid = agent_pid(project.path());
    let other_pid = agent_pid(other_project.path());
    send(&mut client, &id, "background", "background").await;
    let events = wait(&mut client, &id, Status::Completed).await;
    let background = events
        .iter()
        .find_map(|e| {
            if let EventKind::Text { text } = &e.event {
                text.strip_prefix("background:")
                    .and_then(|s| s.parse::<i32>().ok())
            } else {
                None
            }
        })
        .unwrap();
    assert!(kill(Pid::from_raw(background), None).is_ok());
    assert!(
        matches!(ask(&mut client, Request::Poll { session_id: id.clone(), after: 0.0 }).await, Response::Events { session, .. } if session.agent_session_open == Some(true) && session.agent_session_busy == Some(true))
    );
    assert!(
        matches!(ask(&mut client, Request::CloseAgentSession { session_id: id.clone(), only_if_idle: Some(true) }).await, Response::Error { message, .. } if message.contains("确认后关闭"))
    );
    assert!(kill(Pid::from_raw(background), None).is_ok());
    assert!(kill(Pid::from_raw(pid), None).is_ok());
    drop(client);
    let mut client = host.client().await;
    assert!(matches!(
        ask(
            &mut client,
            Request::OpenAgentSession {
                session_id: id.clone(),
                provider: None
            }
        )
        .await,
        Response::Ok
    ));
    send(&mut client, &id, "identity", "identity").await;
    let events = wait(&mut client, &id, Status::Completed).await;
    let identity: serde_json::Value = events
        .iter()
        .rev()
        .find_map(|e| {
            if let EventKind::Text { text } = &e.event {
                serde_json::from_str(text).ok()
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(identity["pid"], pid);
    assert_eq!(identity["turns"], 2);
    assert_eq!(identity["background_pid"], background);
    assert!(matches!(
        send(&mut client, &id, "identity", "identity").await,
        Response::Accepted { duplicate: true }
    ));
    let result = ask(
        &mut client,
        Request::Send {
            session_id: id.clone(),
            request_id: "changed".into(),
            text: "identity".into(),
            model: Some("opus".into()),
            effort: None,
            permission_mode: None,
            provider: None,
        },
    )
    .await;
    assert!(matches!(result, Response::Error { message, .. } if message.contains("后台")));
    assert_eq!(agent_pid(project.path()), pid);
    assert!(kill(Pid::from_raw(background), None).is_ok());
    send(&mut client, &id, "hang", "hang").await;
    assert!(matches!(
        ask(
            &mut client,
            Request::Cancel {
                session_id: id.clone()
            }
        )
        .await,
        Response::Ok
    ));
    wait(&mut client, &id, Status::Stopped).await;
    assert!(kill(Pid::from_raw(pid), None).is_ok());
    assert!(kill(Pid::from_raw(background), None).is_ok());
    assert!(
        matches!(ask(&mut client, Request::Session { session_id: id.clone() }).await, Response::Session { session } if session.agent_session_open == Some(true))
    );
    send(&mut client, &id, "after-stop", "identity").await;
    wait(&mut client, &id, Status::Completed).await;
    assert_eq!(agent_pid(project.path()), pid);
    send(&mut client, &id, "pending-approval", "approve").await;
    let events = wait(&mut client, &id, Status::Waiting).await;
    let approval = events
        .iter()
        .rev()
        .find_map(|event| match &event.event {
            EventKind::Approval { request_id, .. } => Some(request_id.clone()),
            _ => None,
        })
        .unwrap();
    assert!(matches!(
        ask(
            &mut client,
            Request::Cancel {
                session_id: id.clone()
            }
        )
        .await,
        Response::Ok
    ));
    wait(&mut client, &id, Status::Stopped).await;
    assert!(matches!(
        ask(
            &mut client,
            Request::Approve {
                session_id: id.clone(),
                request_id: approval,
                allow: true
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(!project.path().join("approved.txt").exists());
    assert_eq!(agent_pid(project.path()), pid);
    for _ in 0..2 {
        assert!(matches!(
            ask(
                &mut client,
                Request::CloseAgentSession {
                    session_id: id.clone(),
                    only_if_idle: None
                }
            )
            .await,
            Response::Ok
        ));
    }
    assert!(
        matches!(ask(&mut client, Request::Session { session_id: id.clone() }).await, Response::Session { session } if session.agent_session_open == Some(false))
    );
    assert!(
        matches!(ask(&mut client, Request::Session { session_id: other.clone() }).await, Response::Session { session } if session.agent_session_open == Some(true))
    );
    assert_gone(pid).await;
    assert_gone(background).await;
    assert!(kill(Pid::from_raw(other_pid), None).is_ok());
    assert!(matches!(
        ask(
            &mut client,
            Request::OpenAgentSession {
                session_id: id.clone(),
                provider: None
            }
        )
        .await,
        Response::Ok
    ));
    assert_ne!(agent_pid(project.path()), pid);
    assert!(
        matches!(ask(&mut client, Request::Poll { session_id: id, after: 0.0 }).await, Response::Events { events, session, .. } if session.agent_session_open == Some(true) && session.native_id.is_some() && events.iter().filter(|e| matches!(e.event, EventKind::User { .. })).count() == 5)
    );
}
#[tokio::test]
async fn persistent_background_completion_delivers_autonomous_followup_without_new_prompt() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    send(&mut client, &id, "background-report", "background-report").await;
    wait(&mut client, &id, Status::Completed).await;
    let pid = agent_pid(project.path());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let events = wait(&mut client, &id, Status::Completed).await;
        if events
            .iter()
            .any(|e| matches!(&e.event, EventKind::Text { text } if text == "BACKGROUND_REPORT_OK"))
        {
            assert_eq!(
                events
                    .iter()
                    .filter(|e| matches!(e.event, EventKind::User { .. }))
                    .count(),
                1
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "background report lost after result"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(agent_pid(project.path()), pid);
}

#[tokio::test]
async fn agent_session_projection_waits_for_initialization_and_never_persists() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let fixture = directory.path().join("claude");
    std::fs::write(&fixture, r#"#!/usr/bin/env python3
import json,sys,time,os
if '--version' in sys.argv:
    print('fixture 1.0');sys.exit(0)
for line in sys.stdin:
    value=json.loads(line)
    open('initializing','w').close()
    if os.path.exists('fail-init'): sys.exit(1)
    time.sleep(0.4)
    print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':value['request_id'],'response':{}}}),flush=True)
"#).unwrap();
    std::fs::set_permissions(&fixture, std::fs::Permissions::from_mode(0o755)).unwrap();
    let host = Host::start_with_environment(&[("LATTE_WORK_CLAUDE", fixture.as_os_str())]).await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    let mut opener = host.client().await;
    let target = id.clone();
    let opening = tokio::spawn(async move {
        ask(
            &mut opener,
            Request::OpenAgentSession {
                session_id: target,
                provider: None,
            },
        )
        .await
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    while !project.path().join("initializing").exists() {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        matches!(ask(&mut client, Request::Session { session_id: id.clone() }).await, Response::Session { session } if session.agent_session_open == Some(false))
    );
    let history = tokio::time::timeout(
        Duration::from_millis(200),
        ask(
            &mut client,
            Request::History {
                session_id: id.clone(),
                before: None,
            },
        ),
    )
    .await
    .unwrap();
    assert!(
        matches!(history, Response::History { session, .. } if session.agent_session_open == Some(false))
    );
    assert!(matches!(opening.await.unwrap(), Response::Ok));
    let Response::Session { session } = ask(
        &mut client,
        Request::PinSession {
            session_id: id.clone(),
            pinned: true,
        },
    )
    .await
    else {
        panic!("missing session")
    };
    assert_eq!(session.agent_session_open, Some(true));
    assert!(
        matches!(ask(&mut client, Request::Sessions { project_id: session.project_id }).await, Response::Sessions { sessions } if sessions[0].agent_session_open == Some(true))
    );
    assert!(
        matches!(ask(&mut client, Request::PinnedSessions).await, Response::Sessions { sessions } if sessions[0].agent_session_open == Some(true))
    );
    let database = rusqlite::Connection::open(host.directory.path().join("state.sqlite")).unwrap();
    let stored: String = database
        .query_row("SELECT data FROM sessions WHERE id=?1", [&id], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(
        serde_json::from_str::<serde_json::Value>(&stored)
            .unwrap()
            .get("agent_session_open")
            .is_none()
    );
    assert!(matches!(
        ask(
            &mut client,
            Request::CloseAgentSession {
                session_id: id.clone(),
                only_if_idle: None
            }
        )
        .await,
        Response::Ok
    ));
    assert!(
        matches!(ask(&mut client, Request::PinnedSessions).await, Response::Sessions { sessions } if sessions[0].agent_session_open == Some(false))
    );
    std::fs::write(project.path().join("fail-init"), "").unwrap();
    assert!(matches!(
        ask(
            &mut client,
            Request::OpenAgentSession {
                session_id: id.clone(),
                provider: None
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(
        matches!(ask(&mut client, Request::Session { session_id: id }).await, Response::Session { session } if session.agent_session_open == Some(false))
    );
}

#[tokio::test]
async fn opening_old_provider_history_resolves_model_without_replaying_prompt() {
    use latte_work_protocol::{Provider, ProviderSnapshot, TurnProvider};
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    assert!(matches!(
        ask(
            &mut client,
            Request::Send {
                provider: Some(TurnProvider::Cli),
                session_id: id.clone(),
                request_id: "old-alias".into(),
                text: "identity".into(),
                model: Some("opus".into()),
                effort: None,
                permission_mode: None,
            }
        )
        .await,
        Response::Accepted { .. }
    ));
    wait(&mut client, &id, Status::Completed).await;
    ask(
        &mut client,
        Request::CloseAgentSession {
            session_id: id.clone(),
            only_if_idle: Some(true),
        },
    )
    .await;
    let provider: Provider = serde_json::from_value(serde_json::json!({
        "id":"dev", "name":"Dev", "protocol":"anthropic_messages", "base_url":"http://localhost:8000", "model":"real-id", "models":["other-id"], "model_labels":{}, "auth":"none", "has_credential":false,
    })).unwrap();
    let snapshot = ProviderSnapshot {
        provider,
        credential: String::new(),
    };
    assert!(matches!(
        ask(
            &mut client,
            Request::OpenAgentSession {
                session_id: id.clone(),
                provider: Some(TurnProvider::Snapshot(snapshot))
            }
        )
        .await,
        Response::Ok
    ));
    let Response::Events {
        session, events, ..
    } = ask(
        &mut client,
        Request::Poll {
            session_id: id.clone(),
            after: 0.0,
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!(session.agent_session_open, Some(true));
    assert_eq!(session.agent_session_busy, Some(false));
    assert_eq!(session.model.as_deref(), Some("opus"));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.event, EventKind::User { .. }))
            .count(),
        1
    );
    assert!(matches!(
        ask(
            &mut client,
            Request::CloseAgentSession {
                session_id: id.clone(),
                only_if_idle: Some(true)
            }
        )
        .await,
        Response::Ok
    ));
}

#[tokio::test]
async fn disconnected_app_quit_preserves_remote_execution_and_reattaches_without_replay() {
    use latte_work_protocol::lifecycle::Resource;
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    let resource = Resource::AgentSession(id.clone());
    let server_id = client.server_id().to_owned();
    client
        .claim_resources("old-app".into(), vec![resource.clone()])
        .await
        .unwrap();
    assert!(matches!(
        ask(
            &mut client,
            Request::OpenAgentSession {
                session_id: id.clone(),
                provider: None
            }
        )
        .await,
        Response::Ok
    ));
    let pid = agent_pid(project.path());
    assert!(matches!(
        send(&mut client, &id, "remote-task", "hang").await,
        Response::Accepted { .. }
    ));
    wait(&mut client, &id, Status::Running).await;
    // EOF is only a lost transport. No synthetic quit/close or lease expiry.
    drop(client);
    assert!(kill(Pid::from_raw(pid), None).is_ok());
    let mut restored = host.client().await;
    assert_eq!(restored.server_id(), server_id);
    for _ in 0..2 {
        assert_eq!(
            restored
                .adopt_resources(
                    server_id.clone(),
                    "old-app".into(),
                    "new-app".into(),
                    vec![resource.clone()]
                )
                .await
                .unwrap(),
            vec![resource.clone()]
        );
    }
    assert_eq!(
        agent_pid(project.path()),
        pid,
        "reattach must not respawn the Agent"
    );
    assert!(
        matches!(ask(&mut restored, Request::Poll { session_id: id.clone(), after: 0.0 }).await,
        Response::Events { session, events, .. } if session.status == Status::Running && session.agent_session_open == Some(true) && session.agent_session_busy == Some(true)
        && events.iter().filter(|e| matches!(&e.event, EventKind::User { .. })).count() == 1)
    );
    // A delayed close from the old App cannot kill the newly attached resource.
    restored
        .close_owned_resources(server_id.clone(), "old-app".into(), vec![resource.clone()])
        .await
        .unwrap();
    assert!(kill(Pid::from_raw(pid), None).is_ok());
    assert!(
        restored
            .adopt_resources(
                server_id.clone(),
                "unrelated-owner".into(),
                "other-app".into(),
                vec![resource.clone()]
            )
            .await
            .unwrap()
            .is_empty()
    );
    restored
        .close_owned_resources(
            "old-daemon".into(),
            "new-app".into(),
            vec![resource.clone()],
        )
        .await
        .unwrap();
    assert!(kill(Pid::from_raw(pid), None).is_ok());
    restored
        .close_owned_resources(server_id, "new-app".into(), vec![resource])
        .await
        .unwrap();
    assert_gone(pid).await;
    wait(&mut restored, &id, Status::Stopped).await;
}

#[tokio::test]
async fn persisted_handback_report_is_read_without_reopening_or_replaying_agent() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let folder = tempfile::tempdir().unwrap();
    let id = session(&mut client, folder.path()).await;
    send(&mut client, &id, "child-request", "subagents").await;
    wait(&mut client, &id, Status::Completed).await;
    let Response::Subagents { tasks, .. } = ask(
        &mut client,
        Request::Subagents {
            session_id: id.clone(),
        },
    )
    .await
    else {
        panic!()
    };
    let child = &tasks[0];
    ask(
        &mut client,
        Request::CloseAgentSession {
            session_id: id.clone(),
            only_if_idle: Some(false),
        },
    )
    .await;
    let report = "完整历史报告".repeat(1000);
    let events = vec![
        latte_work_protocol::Event {
            seq: 1.0,
            session_id: id.clone(),
            at: 0.0,
            event: EventKind::Tool {
                id: "handback".into(),
                name: "SubagentHandback".into(),
                input: serde_json::json!({"message":report}),
            },
        },
        latte_work_protocol::Event {
            seq: 2.0,
            session_id: id.clone(),
            at: 0.0,
            event: EventKind::ToolResult {
                id: "handback".into(),
                content: serde_json::json!([{"type":"text","text":"{\"success\":true}"}]),
                is_error: false,
            },
        },
    ];
    let db = rusqlite::Connection::open(host.directory.path().join("state.sqlite")).unwrap();
    db.execute(
        "INSERT INTO subagent_native_transcripts(session_id,task_id,data) VALUES (?1,?2,?3)",
        rusqlite::params![
            id,
            child.id,
            serde_json::to_string(&(events, true)).unwrap()
        ],
    )
    .unwrap();
    let requests: i64 = db
        .query_row("SELECT count(*) FROM requests", [], |row| row.get(0))
        .unwrap();
    drop(client);
    let mut client = host.client().await;
    let Response::SubagentTranscript { events, truncated } = ask(
        &mut client,
        Request::SubagentTranscript {
            session_id: id.clone(),
            task_id: child.id.clone(),
        },
    )
    .await
    else {
        panic!()
    };
    let output: String = events
        .iter()
        .filter_map(|e| match &e.event {
            EventKind::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(output, report);
    assert!(truncated && events.iter().all(|e| e.session_id == id));
    assert_eq!(
        db.query_row("SELECT count(*) FROM requests", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        requests
    );
    assert!(
        matches!(ask(&mut client,Request::Session { session_id:id }).await,Response::Session { session } if session.agent_session_open==Some(false))
    );
}

#[tokio::test]
async fn subagents_native_commands_snapshot_reconnect_and_close_share_one_protocol() {
    use latte_work_protocol::SubagentStatus;
    let host = Host::start().await;
    let mut client = host.client().await;
    let folder = tempfile::tempdir().unwrap();
    let id = session(&mut client, folder.path()).await;
    send(&mut client, &id, "child-request", "subagents").await;
    let events = wait(&mut client, &id, Status::Completed).await;
    assert!(
        events
            .iter()
            .any(|event| matches!(event.event, EventKind::Subagent { .. }))
    );
    let snapshot = ask(
        &mut client,
        Request::Subagents {
            session_id: id.clone(),
        },
    )
    .await;
    assert!(
        matches!(snapshot, Response::Subagents {tasks, truncated:false} if tasks.len()==1 && tasks[0].status == SubagentStatus::Running && tasks[0].native_id == "native-child" && tasks[0].last_tool.as_deref()==Some("Read"))
    );
    drop(client);
    let mut client = host.client().await;
    send(&mut client, &id, "agents-command", "/agents").await;
    let events = wait(&mut client, &id, Status::Completed).await;
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.event, EventKind::User {text,..} if text=="/agents"))
    );
    assert!(
        matches!(ask(&mut client, Request::Subagents {session_id:id.clone()}).await, Response::Subagents {tasks,..} if tasks.len()==1 && tasks[0].status==SubagentStatus::Stopped && tasks[0].summary.as_deref()==Some("Stopped by native command"))
    );
    let another = session(&mut client, folder.path()).await;
    assert!(
        matches!(ask(&mut client, Request::Subagents {session_id:another}).await, Response::Subagents {tasks,..} if tasks.is_empty())
    );
    send(&mut client, &id, "background-child", "background").await;
    wait(&mut client, &id, Status::Completed).await;
    ask(
        &mut client,
        Request::CloseAgentSession {
            session_id: id.clone(),
            only_if_idle: Some(false),
        },
    )
    .await;
    assert!(
        matches!(ask(&mut client, Request::Subagents {session_id:id}).await, Response::Subagents {tasks,..} if tasks.iter().all(|task| !task.status.active()) && tasks.iter().any(|task| task.native_id=="bg-task" && task.status==SubagentStatus::Stopped))
    );
}

#[tokio::test]
async fn task_baselines_and_sources_integrate_with_real_process_reconnect_and_bounded_preview() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    assert!(
        Command::new("git")
            .current_dir(&root)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(root.join("task.txt"), "initial\n").unwrap();
    assert!(
        Command::new("git")
            .current_dir(&root)
            .args(["add", "."])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&root)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "commit",
                "-qm",
                "initial"
            ])
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(root.join("task.txt"), "existing dirty\n").unwrap();
    std::fs::write(root.join("fixture-agent-pid"), "already present\n").unwrap();
    let id = session(&mut client, &root).await;
    let Response::Session { session: task } = ask(
        &mut client,
        Request::Session {
            session_id: id.clone(),
        },
    )
    .await
    else {
        panic!()
    };
    let external = tempfile::tempdir().unwrap();
    let file = external.path().canonicalize().unwrap().join("image.png");
    std::fs::write(&file, b"\x89PNG\r\n\x1a\n").unwrap();
    let preview = |path: String, offset| Request::PreviewSource {
        project_id: task.project_id.clone(),
        session_id: Some(id.clone()),
        path,
        offset,
    };
    assert!(matches!(
        ask(&mut client, preview(file.to_string_lossy().into(), 0)).await,
        Response::Error { .. }
    ));
    let marker = "\n文件引用（相对路径基于当前项目根目录；绝对路径位于当前执行主机。仅引用路径，按当前权限读取）：\n";
    let text = format!(
        "task-edit{marker}{}",
        serde_json::json!([{"type":"file","path":file,"name":"image.png","mime_type":"image/png"}])
    );
    send(&mut client, &id, "task-r1", &text).await;
    wait(&mut client, &id, Status::Completed).await;
    let summary_request = || Request::ChangeSummary {
        project_id: task.project_id.clone(),
        session_id: Some(id.clone()),
    };
    let Response::ChangeSummary { summary } = ask(&mut client, summary_request()).await else {
        panic!()
    };
    let change = summary
        .entries
        .iter()
        .find(|c| c.path == "task.txt")
        .unwrap();
    assert_eq!((change.added, change.removed), (Some(2), Some(1)));
    let baseline_at = summary.baseline_at;
    let Response::Content { text: patch, .. } = ask(
        &mut client,
        Request::TaskChangeDiff {
            session_id: id.clone(),
            path: "task.txt".into(),
        },
    )
    .await
    else {
        panic!()
    };
    assert!(patch.contains("-existing dirty") && patch.contains("+agent change"));
    let mut reconnect = host.client().await;
    let Response::Sources { entries, truncated } = ask(
        &mut reconnect,
        Request::Sources {
            session_id: id.clone(),
        },
    )
    .await
    else {
        panic!()
    };
    assert!(!truncated);
    let connector = entries
        .iter()
        .find(|s| s.id == "connector:fixture_docs")
        .unwrap();
    assert_eq!(connector.uses, 2);
    assert_eq!(connector.tools.len(), 2);
    assert!(entries.iter().any(|s| s.name == "image.png"));
    let Response::Sources { entries: again, .. } = ask(
        &mut reconnect,
        Request::Sources {
            session_id: id.clone(),
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!(again.iter().find(|s| s.id == connector.id).unwrap().uses, 2);
    assert!(
        matches!(ask(&mut reconnect, preview(file.to_string_lossy().into(), 0)).await, Response::SourcePreview { mime_type, data, has_more: false, .. } if mime_type=="image/png" && data.len()==8)
    );
    assert!(matches!(
        ask(&mut reconnect, preview(file.to_string_lossy().into(), 100)).await,
        Response::Error { .. }
    ));
    let other = external.path().join("secret.txt");
    std::fs::write(&other, "private").unwrap();
    std::os::unix::fs::symlink(&other, root.join("escape")).unwrap();
    assert!(matches!(
        ask(&mut reconnect, preview("escape".into(), 0)).await,
        Response::Error { .. }
    ));
    assert!(matches!(
        ask(&mut reconnect, preview(other.to_string_lossy().into(), 0)).await,
        Response::Error { .. }
    ));
    assert!(matches!(
        ask(
            &mut reconnect,
            Request::ResolveReference {
                project_id: task.project_id.clone(),
                path: other.to_string_lossy().into()
            }
        )
        .await,
        Response::FileReference { .. }
    ));
    assert!(matches!(
        ask(&mut reconnect, preview(other.to_string_lossy().into(), 0)).await,
        Response::SourcePreview { .. }
    ));
    assert!(matches!(
        ask(
            &mut reconnect,
            Request::ReadFile {
                project_id: task.project_id.clone(),
                path: other.to_string_lossy().into()
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(matches!(
        send(&mut reconnect, &id, "task-r1", &text).await,
        Response::Accepted { duplicate: true }
    ));
    let Response::ChangeSummary { summary: later } = ask(&mut reconnect, summary_request()).await
    else {
        panic!()
    };
    assert_eq!(later.baseline_at, baseline_at);
}

#[tokio::test]
async fn ordinary_directory_turn_changes_survive_followup_and_reconnect() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    std::fs::write(root.join("task.txt"), "existing\n").unwrap();
    let id = session(&mut client, &root).await;
    assert!(matches!(
        ask(
            &mut client,
            Request::OpenAgentSession {
                session_id: id.clone(),
                provider: None
            }
        )
        .await,
        Response::Ok
    ));
    send(&mut client, &id, "empty", "identity").await;
    let events = wait(&mut client, &id, Status::Completed).await;
    assert!(events.iter().any(|e| matches!(&e.event, EventKind::TurnChanges {changes} if changes.request_id == "empty" && changes.summary.entries.is_empty() && changes.summary.unavailable.is_none() && !changes.background_pending)));
    for (request_id, prompt) in [
        ("ordinary-r1", "task-edit"),
        ("ordinary-r2", "task-edit-second"),
    ] {
        assert!(matches!(
            send(&mut client, &id, request_id, prompt).await,
            Response::Accepted { duplicate: false }
        ));
        wait(&mut client, &id, Status::Completed).await;
        let Response::TurnChanges { changes } = ask(
            &mut client,
            Request::TurnChangeSummary {
                session_id: id.clone(),
                request_id: request_id.into(),
            },
        )
        .await
        else {
            panic!()
        };
        assert_eq!(changes.summary.entries.len(), 1);
        assert_eq!(changes.summary.entries[0].path, "task.txt");
        assert!(changes.summary.unavailable.is_none());
        assert!(
            !changes.background_pending,
            "busy session telemetry is not a background task"
        );
    }
    let mut reconnect = host.client().await;
    let Response::Content { text, .. } = ask(
        &mut reconnect,
        Request::TurnChangeDiff {
            session_id: id.clone(),
            request_id: "ordinary-r1".into(),
            path: "task.txt".into(),
        },
    )
    .await
    else {
        panic!()
    };
    assert!(
        text.contains("-existing")
            && text.contains("+agent change")
            && !text.contains("third line")
    );
    assert!(matches!(
        send(&mut reconnect, &id, "ordinary-r2", "task-edit-second").await,
        Response::Accepted { duplicate: true }
    ));
    let Response::TurnChanges { changes } = ask(
        &mut reconnect,
        Request::UndoTurnChanges {
            session_id: id.clone(),
            request_id: "ordinary-r2".into(),
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!(changes.undo, latte_work_protocol::TurnUndoStatus::Reverted);
    assert_eq!(
        std::fs::read_to_string(root.join("task.txt")).unwrap(),
        "agent change\nsecond line\n"
    );
}

#[tokio::test]
async fn turn_changes_freeze_each_request_and_undo_rejects_conflicts_then_reconciles_once() {
    use latte_work_protocol::TurnUndoStatus;
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    assert!(
        Command::new("git")
            .current_dir(&root)
            .args(["init", "-q"])
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(root.join(".gitignore"), "fixture-agent-pid\n").unwrap();
    std::fs::write(root.join("task.txt"), "existing dirty\n").unwrap();
    let id = session(&mut client, &root).await;
    let summary = |request_id: &str| Request::TurnChangeSummary {
        session_id: id.clone(),
        request_id: request_id.into(),
    };
    let diff = |request_id: &str| Request::TurnChangeDiff {
        session_id: id.clone(),
        request_id: request_id.into(),
        path: "task.txt".into(),
    };
    let undo = |request_id: &str| Request::UndoTurnChanges {
        session_id: id.clone(),
        request_id: request_id.into(),
    };
    let latest = || Request::LastTurnChanges {
        session_id: id.clone(),
    };
    assert!(matches!(
        ask(&mut client, latest()).await,
        Response::LastTurnChanges { changes: None }
    ));
    send(&mut client, &id, "turn-r1", "task-edit").await;
    let events = wait(&mut client, &id, Status::Completed).await;
    assert!(events.iter().any(|e| matches!(&e.event,EventKind::TurnChanges { changes } if changes.request_id=="turn-r1" && changes.summary.entries.len()==1)));
    let Response::Content { text: first, .. } = ask(&mut client, diff("turn-r1")).await else {
        panic!()
    };
    assert!(first.contains("-existing dirty") && first.contains("+agent change"));
    assert!(matches!(
        send(&mut client, &id, "turn-r1", "task-edit").await,
        Response::Accepted { duplicate: true }
    ));
    send(&mut client, &id, "turn-r2", "task-edit-second").await;
    wait(&mut client, &id, Status::Completed).await;
    let Response::TurnChanges { changes } = ask(&mut client, summary("turn-r2")).await else {
        panic!()
    };
    assert_eq!((changes.summary.added, changes.summary.removed), (1, 2));
    assert_eq!(changes.summary.entries.len(), 1);
    let mut reconnect = host.client().await;
    let Response::Content { text, .. } = ask(&mut reconnect, diff("turn-r1")).await else {
        panic!()
    };
    assert_eq!(text, first, "old diff must not read the latest workspace");
    assert!(matches!(
        ask(&mut reconnect, undo("turn-r1")).await,
        Response::Error { .. }
    ));
    std::fs::write(root.join("task.txt"), "external writer\n").unwrap();
    assert!(matches!(
        ask(&mut reconnect, undo("turn-r2")).await,
        Response::Error { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(root.join("task.txt")).unwrap(),
        "external writer\n"
    );
    std::fs::write(root.join("task.txt"), "third line\n").unwrap();
    let index_before = Command::new("git")
        .current_dir(&root)
        .args(["ls-files", "--stage"])
        .output()
        .unwrap()
        .stdout;
    let Response::Session { session } = ask(
        &mut client,
        Request::Session {
            session_id: id.clone(),
        },
    )
    .await
    else {
        panic!()
    };
    let terminal_id = uuid::Uuid::new_v4().to_string();
    ask(
        &mut client,
        Request::CreateTerminal {
            project_id: session.project_id,
            terminal_id: terminal_id.clone(),
            cols: 80,
            rows: 24,
        },
    )
    .await;
    assert!(matches!(
        ask(&mut reconnect, undo("turn-r2")).await,
        Response::Error { .. }
    ));
    ask(&mut client, Request::CloseTerminal { terminal_id }).await;
    let Response::TurnChanges { changes } = ask(&mut reconnect, undo("turn-r2")).await else {
        panic!()
    };
    assert_eq!(changes.undo, TurnUndoStatus::Reverted);
    assert_eq!(
        std::fs::read_to_string(root.join("task.txt")).unwrap(),
        "agent change\nsecond line\n"
    );
    assert_eq!(
        Command::new("git")
            .current_dir(&root)
            .args(["ls-files", "--stage"])
            .output()
            .unwrap()
            .stdout,
        index_before
    );
    std::fs::write(root.join("task.txt"), "post-undo manual edit\n").unwrap();
    let Response::TurnChanges { changes } = ask(&mut reconnect, undo("turn-r2")).await else {
        panic!()
    };
    assert_eq!(changes.undo, TurnUndoStatus::Reverted);
    assert_eq!(
        std::fs::read_to_string(root.join("task.txt")).unwrap(),
        "post-undo manual edit\n"
    );
    let Response::Content { text, .. } = ask(&mut reconnect, diff("turn-r2")).await else {
        panic!()
    };
    assert!(text.contains("-agent change") && text.contains("+third line"));
    assert!(
        matches!(ask(&mut reconnect, latest()).await, Response::LastTurnChanges { changes: Some(changes) } if changes.request_id=="turn-r2" && changes.undo==TurnUndoStatus::Reverted && changes.summary.added==1 && changes.summary.removed==2)
    );
    assert!(matches!(
        ask(
            &mut reconnect,
            Request::LastTurnChanges {
                session_id: "missing".into()
            }
        )
        .await,
        Response::Error { .. }
    ));
    assert!(matches!(
        ask(
            &mut reconnect,
            Request::TurnChangeSummary {
                session_id: "another-session".into(),
                request_id: "turn-r1".into()
            }
        )
        .await,
        Response::Error { .. }
    ));
}

#[tokio::test]
async fn git_review_wire_preserves_scope_and_branch_snapshot_after_reconnect() {
    use latte_work_protocol::GitReviewScope;
    let host = Host::start().await;
    let mut client = host.client().await;
    let root = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(root.path())
                .output()
                .unwrap()
                .status
                .success()
        );
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.name", "Fixture"]);
    run(&["config", "user.email", "fixture@example.test"]);
    std::fs::write(root.path().join("file.txt"), "original\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-qm", "base"]);
    run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    run(&["checkout", "-qb", "feature"]);
    std::fs::write(root.path().join("file.txt"), "committed\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-qm", "feature"]);
    std::fs::write(root.path().join("file.txt"), "working\nextra\n").unwrap();
    let Response::Project { project } = ask(
        &mut client,
        Request::AddProject {
            path: root.path().to_string_lossy().into(),
            name: None,
        },
    )
    .await
    else {
        panic!()
    };
    let Response::GitInfo { info } = ask(
        &mut client,
        Request::GitInfo {
            project_id: project.id.clone(),
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!(info.branch.as_deref(), Some("feature"));
    assert_eq!(
        info.default_base.as_deref(),
        Some("refs/remotes/origin/main")
    );
    let Response::GitReview { review } = ask(
        &mut client,
        Request::GitReview {
            project_id: project.id.clone(),
            scope: GitReviewScope::Branch,
            base: None,
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!((review.added, review.removed), (1, 1));
    let Response::GitCommits {
        head,
        commits,
        has_more: false,
    } = ask(
        &mut client,
        Request::GitCommits {
            project_id: project.id.clone(),
            head: None,
            offset: 0,
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].subject, "feature");
    assert_eq!(head.as_deref(), Some(commits[0].oid.as_str()));
    let Response::GitReview { review: committed } = ask(
        &mut client,
        Request::GitReview {
            project_id: project.id.clone(),
            scope: GitReviewScope::Commit,
            base: Some(commits[0].oid.clone()),
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!((committed.added, committed.removed), (1, 1));
    let Response::GitReview {
        review: root_commit,
    } = ask(
        &mut client,
        Request::GitReview {
            project_id: project.id.clone(),
            scope: GitReviewScope::Commit,
            base: Some(commits[1].oid.clone()),
        },
    )
    .await
    else {
        panic!()
    };
    assert!(root_commit.base.is_none());
    assert!(
        matches!(ask(&mut client, Request::GitReviewDiff { project_id: project.id.clone(), scope: GitReviewScope::Commit, base: None, head: root_commit.head, path: "file.txt".into(), full_context: false }).await, Response::Content { text, .. } if text.contains("+original"))
    );
    let mut reconnect = host.client().await;
    run(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let request = |path: &str| Request::GitReviewDiff {
        project_id: project.id.clone(),
        scope: GitReviewScope::Branch,
        base: review.base.clone(),
        head: review.head.clone(),
        path: path.into(),
        full_context: false,
    };
    assert!(
        matches!(ask(&mut reconnect,request("file.txt")).await,Response::Content{text,truncated:false} if text.contains("+committed")&&!text.contains("+working"))
    );
    assert!(matches!(
        ask(&mut reconnect, request("../outside")).await,
        Response::Error { .. }
    ));
    let Response::GitReview { review } = ask(
        &mut reconnect,
        Request::GitReview {
            project_id: project.id.clone(),
            scope: GitReviewScope::Worktree,
            base: None,
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!((review.added, review.removed), (2, 1));
    assert!(matches!(
        ask(
            &mut reconnect,
            Request::GitReview {
                project_id: project.id,
                scope: GitReviewScope::Branch,
                base: Some("--output=/tmp/escape".into())
            }
        )
        .await,
        Response::Error { .. }
    ));
}

#[tokio::test]
async fn complete_child_history_recovers_every_native_page_and_long_report_without_agent_replay() {
    let host = Host::start_with_environment(&[
        ("LATTE_FIXTURE_CHILD_NATIVE_FILE", std::ffi::OsStr::new("1")),
        (
            "LATTE_FIXTURE_CHILD_HISTORY_LONG",
            std::ffi::OsStr::new("1"),
        ),
    ])
    .await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    ask(
        &mut client,
        Request::Send {
            provider: Some(latte_work_protocol::TurnProvider::Cli),
            session_id: id.clone(),
            request_id: "full-history".into(),
            text: "subagent-process".into(),
            model: None,
            effort: None,
            permission_mode: None,
        },
    )
    .await;
    wait(&mut client, &id, Status::Completed).await;
    // Main completion precedes the background child and its native file flush.
    // Wait for live idle evidence; a fixed sleep races large records on Linux.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let Response::Events { session, .. } = ask(
            &mut client,
            Request::Poll {
                session_id: id.clone(),
                after: 0.0,
            },
        )
        .await
        else {
            panic!("session status");
        };
        if session.agent_session_busy == Some(false) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "background child never became idle"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert!(matches!(
        ask(
            &mut client,
            Request::CloseAgentSession {
                session_id: id.clone(),
                only_if_idle: Some(true)
            }
        )
        .await,
        Response::Ok
    ));
    // Simulate a pre-upgrade daemon: only the legacy clipped snapshot exists.
    // Recovery must read the owned native file without opening an Agent.
    let database = rusqlite::Connection::open(host.directory.path().join("state.sqlite")).unwrap();
    database
        .execute("DELETE FROM subagent_history WHERE session_id=?1", [&id])
        .unwrap();
    database
        .execute(
            "DELETE FROM subagent_history_sources WHERE session_id=?1",
            [&id],
        )
        .unwrap();
    drop(database);
    let read = |after, source| Request::SubagentHistory {
        provider: Some(latte_work_protocol::TurnProvider::Cli),
        session_id: id.clone(),
        task_id: "tool:process-agent".into(),
        after,
        source,
    };
    let changed_context = Request::SubagentHistory {
        provider: None,
        session_id: id.clone(),
        task_id: "tool:process-agent".into(),
        after: None,
        source: None,
    };
    assert!(
        matches!(ask(&mut client,changed_context).await,Response::SubagentHistory{incomplete:true,source,..} if source=="legacy")
    );
    let (mut after, mut source, mut found) = (None, None, Vec::new());
    for _ in 0..30 {
        let response = ask(&mut client, read(after, source.clone())).await;
        assert!(serde_json::to_vec(&response).unwrap().len() < latte_work_protocol::MAX_FRAME);
        let Response::SubagentHistory {
            events,
            has_more,
            after: next,
            source_has_more,
            incomplete,
            source: new_source,
        } = response
        else {
            panic!("history");
        };
        assert!(!incomplete);
        found.extend(events);
        after = next;
        source = Some(new_source);
        if !has_more && !source_has_more {
            break;
        }
    }
    assert_eq!(
        found
            .iter()
            .filter(|e| matches!(&e.event,EventKind::Tool{id,..} if id.starts_with("long-read-")))
            .count(),
        700
    );
    let expected = format!(
        "COMPLETE_NATIVE_REPORT\n{}COMPLETE_REPORT_END",
        "full-report-segment\n".repeat(70000)
    );
    assert!(
        found
            .iter()
            .any(|e| matches!(&e.event,EventKind::Text{text} if text==&expected))
    );
    assert!(found.windows(2).all(|p| p[0].seq < p[1].seq));
    assert!(matches!(
        ask(
            &mut client,
            Request::CloseAgentSession {
                session_id: id.clone(),
                only_if_idle: Some(true)
            }
        )
        .await,
        Response::Ok
    ));
    let target = tokio::sync::Mutex::new(host.client().await);
    assert!(
        matches!(latte_work_client::request_with_app_provider(&target,read(Some(found[found.len()-2].seq),Some("native".into())),|_|async {Ok(None)}).await.unwrap(),Response::SubagentHistory{events,..} if events.iter().any(|e|matches!(&e.event,EventKind::Text{text} if text==&expected)))
    );
    let mut reconnected = host.client().await;
    let Response::SubagentHistory {
        events,
        has_more: false,
        incomplete: false,
        ..
    } = ask(
        &mut reconnected,
        read(Some(found[found.len() - 2].seq), Some("native".into())),
    )
    .await
    else {
        panic!("persisted report");
    };
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event,EventKind::Text{text} if text==&expected))
    );
    assert!(
        matches!(ask(&mut reconnected,Request::Poll{session_id:id,after:0.0}).await,Response::Events{session,events,..} if session.agent_session_open==Some(false) && events.iter().filter(|e|matches!(e.event,EventKind::User{..})).count()==1)
    );
}

#[tokio::test]
async fn user_questions_are_not_permissions_and_answers_are_bound_single_use_and_persisted() {
    use latte_work_protocol::QuestionAnswer;
    let host = Host::start().await;
    let mut client = host.client().await;
    let root = tempfile::tempdir().unwrap();
    let sid = session(&mut client, root.path()).await;
    let send_question = |request_id: String| Request::Send {
        permission_mode: Some("bypassPermissions".into()),
        provider: None,
        session_id: sid.clone(),
        request_id,
        text: "ask-user".into(),
        model: None,
        effort: None,
    };
    assert!(matches!(
        ask(&mut client, send_question(uuid::Uuid::new_v4().to_string())).await,
        Response::Accepted { .. }
    ));
    let waiting = wait(&mut client, &sid, Status::Waiting).await;
    assert!(
        !waiting
            .iter()
            .any(|e| matches!(e.event, EventKind::Approval { .. }))
    );
    let public = waiting
        .iter()
        .find_map(|e| {
            if let EventKind::Question {
                request_id,
                questions,
                tool_use_id,
            } = &e.event
            {
                assert_eq!(questions.len(), 2);
                assert_eq!(tool_use_id.as_deref(), Some("question-tool"));
                Some(request_id.clone())
            } else {
                None
            }
        })
        .unwrap();
    assert!(matches!(
        ask(
            &mut client,
            Request::Approve {
                session_id: sid.clone(),
                request_id: public.clone(),
                allow: true
            }
        )
        .await,
        Response::Error { .. }
    ));
    let answer = |answers| Request::AnswerQuestion {
        session_id: sid.clone(),
        request_id: public.clone(),
        answers,
    };
    assert!(matches!(
        ask(&mut client, answer(Some(vec![]))).await,
        Response::Error { .. }
    ));
    let answers = vec![
        QuestionAnswer {
            question_id: "q0".into(),
            option_indices: vec![1],
            text: None,
        },
        QuestionAnswer {
            question_id: "q1".into(),
            option_indices: vec![0, 1],
            text: Some("Extra".into()),
        },
    ];
    let mut invalid = answers.clone();
    invalid[0].option_indices = vec![0, 1];
    assert!(matches!(
        ask(&mut client, answer(Some(invalid))).await,
        Response::Error { .. }
    ));
    let other = session(&mut client, root.path()).await;
    assert!(matches!(
        ask(
            &mut client,
            Request::AnswerQuestion {
                session_id: other,
                request_id: public.clone(),
                answers: Some(answers.clone())
            }
        )
        .await,
        Response::Error { .. }
    ));
    wait(&mut client, &sid, Status::Waiting).await;
    let mut reconnect = host.client().await;
    assert!(matches!(
        ask(&mut reconnect, answer(Some(answers))).await,
        Response::Ok
    ));
    let events = wait(&mut reconnect, &sid, Status::Completed).await;
    assert!(events.iter().any(|e| matches!(&e.event, EventKind::QuestionResolved { request_id, answers: Some(answers) } if request_id==&public && answers.len()==2)));
    let values: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("question-answers.json")).unwrap())
            .unwrap();
    assert_eq!(values["How much detail?"], "Detailed");
    assert_eq!(values["Which sections?"], "Intro, Conclusion, Extra");
    assert!(matches!(
        ask(&mut reconnect, answer(None)).await,
        Response::Error { .. }
    ));
    assert!(matches!(
        ask(
            &mut reconnect,
            send_question(uuid::Uuid::new_v4().to_string())
        )
        .await,
        Response::Accepted { .. }
    ));
    let next = wait(&mut reconnect, &sid, Status::Waiting).await;
    let second = next
        .iter()
        .rev()
        .find_map(|e| {
            if let EventKind::Question { request_id, .. } = &e.event {
                Some(request_id.clone())
            } else {
                None
            }
        })
        .unwrap();
    assert_ne!(public, second);
    assert!(matches!(
        ask(
            &mut reconnect,
            Request::AnswerQuestion {
                session_id: sid.clone(),
                request_id: second,
                answers: None
            }
        )
        .await,
        Response::Ok
    ));
    let events = wait(&mut reconnect, &sid, Status::Completed).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(&e.event, EventKind::Text { text } if text=="QUESTION_SKIPPED"))
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.event, EventKind::User { .. }))
            .count(),
        2
    );
}

#[tokio::test]
async fn compact_and_legacy_reference_prompts_persist_and_preview_on_the_owning_host() {
    let host = Host::start().await;
    let mut client = host.client().await;
    let project = tempfile::tempdir().unwrap();
    let id = session(&mut client, project.path()).await;
    let Response::Events { session: task, .. } = ask(
        &mut client,
        Request::Poll {
            session_id: id.clone(),
            after: 0.0,
        },
    )
    .await
    else {
        panic!("session");
    };
    let outside = tempfile::tempdir().unwrap();
    let png = b"\x89PNG\r\n\x1a\n";
    let paths = [
        outside
            .path()
            .canonicalize()
            .unwrap()
            .join("图 \"compact\".png"),
        outside.path().canonicalize().unwrap().join("legacy.png"),
    ];
    let markers = [
        "\n\n引用（相对项目；绝对路径在当前主机，按现有权限读取）：\n",
        "\n\n文件引用（相对路径基于当前项目根目录；绝对路径位于当前执行主机。仅引用路径，按当前权限读取）：\n",
    ];
    for (index, path) in paths.iter().enumerate() {
        std::fs::write(path, png).unwrap();
        let preview = || Request::PreviewSource {
            project_id: task.project_id.clone(),
            session_id: Some(id.clone()),
            path: path.to_string_lossy().into_owned(),
            offset: 0,
        };
        assert!(matches!(
            ask(&mut client, preview()).await,
            Response::Error { .. }
        ));
        let text = format!(
            "查看图片{}{}",
            markers[index],
            serde_json::json!([{"path":path,"type":"file","name":"original.png","kind":"image"}])
        );
        send(&mut client, &id, &format!("image-ref-{index}"), &text).await;
        wait(&mut client, &id, Status::Completed).await;
        let mut reconnect = host.client().await;
        assert!(
            matches!(ask(&mut reconnect, Request::History { session_id:id.clone(), before:None }).await,
            Response::History { events, .. } if events.iter().any(|event| matches!(&event.event, EventKind::User { text: saved, .. } if saved == &text)))
        );
        assert!(
            matches!(ask(&mut reconnect, Request::Sources { session_id:id.clone() }).await,
            Response::Sources { entries, .. } if entries.iter().any(|source| source.path.as_deref() == path.to_str() && source.name == "original.png"))
        );
        assert!(matches!(ask(&mut reconnect, preview()).await,
            Response::SourcePreview { data, mime_type, has_more, .. } if data == png && mime_type == "image/png" && !has_more));
    }
}
