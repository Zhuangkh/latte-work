//! Real independently linked Server images exercise the executable-identity handoff.
#![cfg(feature = "test-fixture")]
use futures_util::StreamExt;
use latte_work_platform::ipc::{self, Endpoint};
use latte_work_protocol::{MAX_FRAME, VERSION};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, ChildStdout, Command},
};
use tokio_util::codec::{FramedRead, LinesCodec};

const SERVER: &str = env!("CARGO_BIN_EXE_latte-work-server");
const ALTERNATE: &str = env!("CARGO_BIN_EXE_native-alternate-server");
const FIXTURE: &str = env!("CARGO_BIN_EXE_native-claude-fixture");

fn image_id(binary: &str) -> String {
    let mut file = std::fs::File::open(binary).unwrap();
    let mut hash = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    loop {
        let read = file.read(&mut bytes).unwrap();
        if read == 0 {
            break;
        }
        hash.update(&bytes[..read]);
    }
    format!("{:x}", hash.finalize())
}

fn command(binary: &str, mode: &str, state: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .arg(mode)
        .arg("--state-dir")
        .arg(state)
        .env("LATTE_WORK_CLAUDE", FIXTURE)
        .env("LATTE_WORK_AGENT_ENV", "inherit")
        .env(
            "LATTE_NATIVE_PROMPT_LOG",
            state.parent().unwrap().join("prompts.log"),
        )
        .kill_on_drop(true);
    command
}

async fn lifecycle_exchange(state: &Path, request: Value) -> anyhow::Result<Value> {
    use anyhow::Context;
    let stream = Endpoint::open(state)?
        .connect(Duration::from_secs(3))
        .await?;
    let (reader, mut writer) = stream.into_split();
    let mut bytes = serde_json::to_vec(&request)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    let mut output = FramedRead::new(reader, LinesCodec::new_with_max_length(4096));
    let line = output.next().await.context("lifecycle peer closed")??;
    Ok(serde_json::from_str(&line)?)
}

async fn lifecycle(state: &Path, request: Value) -> Value {
    tokio::time::timeout(Duration::from_secs(3), lifecycle_exchange(state, request))
        .await
        .unwrap()
        .unwrap()
}

struct Host {
    root: tempfile::TempDir,
    process: Child,
}
impl Host {
    async fn start() -> Self {
        #[cfg(unix)]
        let root = tempfile::Builder::new()
            .prefix("lwu-")
            .tempdir_in("/tmp")
            .unwrap();
        #[cfg(windows)]
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("state");
        let process = command(SERVER, "serve", &state)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let mut host = Self { root, process };
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                assert!(host.process.try_wait().unwrap().is_none());
                match Endpoint::open(&state) {
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
        host
    }
    fn state(&self) -> PathBuf {
        self.root.path().join("state")
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        // The old daemon is owned; the replacement is detached by the real coordinator.
        // On assertion failure, stop the old child and attempt bounded authenticated
        // idle shutdown of the replacement before TempDir removes its isolated state.
        let _ = self.process.start_kill();
        let state = self.state();
        let cleanup = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async {
                let _ = tokio::time::timeout(Duration::from_secs(8), async {
                    let Ok(status) =
                        lifecycle_exchange(&state, json!({"method":"server_status"})).await
                    else {
                        return;
                    };
                    let _ = lifecycle_exchange(
                        &state,
                        json!({"method":"prepare_upgrade",
                        "server_id":status["server_id"]}),
                    )
                    .await;
                    let Ok(endpoint) = Endpoint::open(&state) else {
                        return;
                    };
                    while endpoint.try_lock_server().is_err() {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                })
                .await;
            });
        });
        let _ = cleanup.join();
    }
}

struct Bridge {
    process: Child,
    output: FramedRead<ChildStdout, LinesCodec>,
}
impl Bridge {
    fn spawn(binary: &str, mode: &str, state: &Path) -> Self {
        let mut process = command(binary, mode, state)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let output = FramedRead::new(
            process.stdout.take().unwrap(),
            LinesCodec::new_with_max_length(MAX_FRAME),
        );
        Self { process, output }
    }
    async fn request(&mut self, request: Value) -> Value {
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        tokio::time::timeout(Duration::from_secs(25), async {
            self.process
                .stdin
                .as_mut()
                .unwrap()
                .write_all(&bytes)
                .await
                .unwrap();
            let line = self.output.next().await.unwrap().unwrap();
            serde_json::from_str(&line).unwrap()
        })
        .await
        .unwrap()
    }
    async fn exited_with_open_stdin(&mut self) {
        assert!(self.process.stdin.is_some());
        assert!(
            tokio::time::timeout(Duration::from_secs(5), self.output.next())
                .await
                .expect("old bridge stdout must close without parent stdin EOF")
                .is_none()
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(3), self.process.wait())
                .await
                .expect("bridge process must exit without parent stdin EOF")
                .unwrap()
                .success()
        );
    }
    async fn wait_status(&mut self, session: &Value, expected: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let state = self
                    .request(json!({"method":"poll","session_id":session,"after":0}))
                    .await;
                assert_ne!(state["kind"], "error", "{state}");
                if state["session"]["status"] == expected {
                    break;
                }
                assert_ne!(state["session"]["status"], "failed", "{state}");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn native_different_image_upgrade_veto_then_concurrent_handoff() {
    let original_id = image_id(SERVER);
    let replacement_id = image_id(ALTERNATE);
    assert_ne!(
        original_id, replacement_id,
        "test needs independently linked images with different SHA-256"
    );
    let mut host = Host::start().await;
    let state = host.state();
    let endpoint = Endpoint::open(&state).unwrap();
    let mut old = Bridge::spawn(SERVER, "connect", &state);
    let hello = old
        .request(json!({"method":"hello","version":VERSION}))
        .await;
    assert_eq!(hello["kind"], "hello");
    let old_server = hello["server_id"].clone();
    let status = lifecycle(&state, json!({"method":"server_status"})).await;
    assert_eq!(status["build_id"], original_id);
    assert_eq!(status["server_id"], old_server);
    let project = old
        .request(json!({"method":"add_project","path":host.root.path(),"name":null}))
        .await;
    let created = old.request(json!({"method":"create_session","project_id":project["project"]["id"],"agent":"claude"})).await;
    let session = &created["session"]["id"];
    assert!(session.is_string(), "{created}");
    assert_eq!(
        old.request(json!({"method":"send","session_id":session,
        "request_id":"upgrade-active","text":"wait"}))
            .await,
        json!({"kind":"accepted","duplicate":false})
    );
    old.wait_status(session, "running").await;
    let mut vetoed = Bridge::spawn(ALTERNATE, "connect-local", &state);
    let refused = vetoed
        .request(json!({"method":"hello","version":VERSION}))
        .await;
    assert_eq!(refused["code"], "local_upgrade_required", "{refused}");
    vetoed.exited_with_open_stdin().await;
    assert!(host.process.try_wait().unwrap().is_none());
    let status = lifecycle(&state, json!({"method":"server_status"})).await;
    assert_eq!(status["server_id"], old_server);
    assert_eq!(status["build_id"], original_id);
    assert_eq!(status["draining"], false);
    assert_eq!(
        old.request(json!({"method":"cancel","session_id":session}))
            .await["kind"],
        "ok"
    );
    old.wait_status(session, "stopped").await;
    assert_eq!(
        old.request(json!({"method":"close_agent_session","session_id":session}))
            .await["kind"],
        "ok"
    );
    let prompts = std::fs::read_to_string(host.root.path().join("prompts.log")).unwrap();
    assert_eq!(prompts, "wait\n");
    let mut first = Bridge::spawn(ALTERNATE, "connect-local", &state);
    let mut second = Bridge::spawn(ALTERNATE, "connect-local", &state);
    let (first_hello, second_hello) = tokio::join!(
        first.request(json!({"method":"hello","version":VERSION})),
        second.request(json!({"method":"hello","version":VERSION})),
    );
    assert_eq!(first_hello["kind"], "hello", "{first_hello}");
    assert_eq!(second_hello["kind"], "hello", "{second_hello}");
    let new_server = &first_hello["server_id"];
    assert_ne!(new_server, &old_server);
    assert_eq!(second_hello["server_id"], *new_server);
    for bridge in [&mut first, &mut second] {
        let status = bridge.request(json!({"method":"server_status"})).await;
        assert_eq!(status["server_id"], *new_server);
        assert_eq!(status["build_id"], replacement_id);
        assert_eq!(status["draining"], false);
    }
    old.exited_with_open_stdin().await;
    assert!(
        tokio::time::timeout(Duration::from_secs(3), host.process.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(matches!(
        endpoint.try_lock_server(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    let mut duplicate = command(SERVER, "serve", &state)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert!(
        !tokio::time::timeout(Duration::from_secs(3), duplicate.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert_eq!(
        lifecycle(&state, json!({"method":"server_status"})).await["server_id"],
        *new_server
    );
    assert_eq!(
        first
            .request(json!({"method":"poll","session_id":session,"after":0}))
            .await["session"]["status"],
        "stopped"
    );
    assert_eq!(
        std::fs::read_to_string(host.root.path().join("prompts.log")).unwrap(),
        prompts
    );
    assert_eq!(
        first
            .request(json!({"method":"prepare_upgrade","server_id":new_server}))
            .await["kind"],
        "upgrade_ready"
    );
    first.exited_with_open_stdin().await;
    second.exited_with_open_stdin().await;
    assert!(endpoint.try_lock_server().is_ok());
}
