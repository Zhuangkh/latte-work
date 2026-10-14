//! Final-binary bridge lifetime checks deliberately keep the parent's stdin open.
use latte_work_platform::{fs::prepare_state_dir, ipc::Endpoint};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
};

const SERVER: &str = env!("CARGO_BIN_EXE_latte-work-server");
const EXIT_TIMEOUT: Duration = Duration::from_secs(3);

fn temporary_state() -> tempfile::TempDir {
    #[cfg(unix)]
    return tempfile::Builder::new()
        .prefix("lwb-")
        .tempdir_in("/tmp")
        .unwrap();
    #[cfg(windows)]
    return tempfile::tempdir().unwrap();
}

fn bridge(command: &str, state: &Path) -> Child {
    Command::new(SERVER)
        .arg(command)
        .arg("--state-dir")
        .arg(state)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

async fn assert_bridge_exited_with_stdin_open(process: &mut Child) {
    assert!(process.stdin.is_some(), "test must keep bridge stdin open");
    let mut output = process.stdout.take().unwrap();
    let mut byte = [0];
    assert_eq!(
        tokio::time::timeout(EXIT_TIMEOUT, output.read(&mut byte))
            .await
            .expect("bridge stdout did not reach EOF while stdin remained open")
            .unwrap(),
        0,
        "bridge emitted unexpected trailing bytes"
    );
    assert!(
        tokio::time::timeout(EXIT_TIMEOUT, process.wait())
            .await
            .expect("bridge did not terminate while stdin remained open")
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn daemon_eof_exits_idle_bridge_with_stdin_open() {
    let root = temporary_state();
    let state = prepare_state_dir(root.path()).unwrap();
    let endpoint = Endpoint::prepare(&state).unwrap();
    let listener = endpoint.bind().await.unwrap();
    let mut process = bridge("connect", &state);
    let stream = tokio::time::timeout(EXIT_TIMEOUT, listener.accept())
        .await
        .unwrap()
        .unwrap();
    // Give the bridge time to start its idle stdin read before the daemon closes.
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(stream);
    assert_bridge_exited_with_stdin_open(&mut process).await;
}

#[tokio::test]
async fn bidirectional_bytes_and_daemon_eof_preserve_open_stdin() {
    let root = temporary_state();
    let state = prepare_state_dir(root.path()).unwrap();
    let endpoint = Endpoint::prepare(&state).unwrap();
    let listener = endpoint.bind().await.unwrap();
    let mut process = bridge("connect", &state);
    let mut stream = tokio::time::timeout(EXIT_TIMEOUT, listener.accept())
        .await
        .unwrap()
        .unwrap();
    let bytes: Vec<u8> = (0..256 * 1024).map(|i| (i % 251) as u8).collect();
    let expected: Vec<u8> = bytes.iter().map(|byte| byte ^ 0xff).collect();
    let mut input = process.stdin.take().unwrap();
    let mut output = process.stdout.take().unwrap();
    let mut received = vec![0; bytes.len()];
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::try_join!(
            async { input.write_all(&bytes).await },
            async {
                let mut request = vec![0; bytes.len()];
                stream.read_exact(&mut request).await?;
                assert_eq!(request, bytes);
                stream.write_all(&expected).await
            },
            async { output.read_exact(&mut received).await.map(|_| ()) },
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(received, expected);
    process.stdin = Some(input);
    process.stdout = Some(output);
    drop(stream);
    assert_bridge_exited_with_stdin_open(&mut process).await;
}

#[tokio::test]
async fn stdin_eof_exits_bridge_and_leaves_listener_available() {
    let root = temporary_state();
    let state = prepare_state_dir(root.path()).unwrap();
    let endpoint = Endpoint::prepare(&state).unwrap();
    let listener = endpoint.bind().await.unwrap();
    let mut process = bridge("connect", &state);
    let mut stream = tokio::time::timeout(EXIT_TIMEOUT, listener.accept())
        .await
        .unwrap()
        .unwrap();
    drop(process.stdin.take());
    assert!(
        tokio::time::timeout(EXIT_TIMEOUT, process.wait())
            .await
            .expect("stdin EOF must terminate the bridge")
            .unwrap()
            .success()
    );
    let mut byte = [0];
    assert_eq!(
        tokio::time::timeout(EXIT_TIMEOUT, stream.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    drop(stream);
    let mut replacement = bridge("connect", &state);
    let replacement_stream = tokio::time::timeout(EXIT_TIMEOUT, listener.accept())
        .await
        .expect("bridge EOF must not terminate its independent daemon")
        .unwrap();
    drop(replacement.stdin.take());
    assert!(
        tokio::time::timeout(EXIT_TIMEOUT, replacement.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    drop(replacement_stream);
}

#[tokio::test]
async fn killed_bridge_with_open_stdin_leaves_listener_available() {
    let root = temporary_state();
    let state = prepare_state_dir(root.path()).unwrap();
    let endpoint = Endpoint::prepare(&state).unwrap();
    let listener = endpoint.bind().await.unwrap();
    let mut process = bridge("connect", &state);
    let mut stream = tokio::time::timeout(EXIT_TIMEOUT, listener.accept())
        .await
        .unwrap()
        .unwrap();
    let input = process.stdin.take().unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    tokio::time::timeout(EXIT_TIMEOUT, process.kill())
        .await
        .unwrap()
        .unwrap();
    let mut byte = [0];
    assert_eq!(
        tokio::time::timeout(EXIT_TIMEOUT, stream.read(&mut byte))
            .await
            .unwrap()
            .unwrap(),
        0
    );
    drop(input);
    drop(stream);
    let mut replacement = bridge("connect", &state);
    let replacement_stream = tokio::time::timeout(EXIT_TIMEOUT, listener.accept())
        .await
        .expect("bridge termination must leave the independent daemon available")
        .unwrap();
    drop(replacement.stdin.take());
    assert!(
        tokio::time::timeout(EXIT_TIMEOUT, replacement.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    drop(replacement_stream);
}

#[cfg(feature = "test-fixture")]
#[tokio::test]
async fn prepare_upgrade_exits_connect_local_bridge_with_stdin_open() {
    use futures_util::StreamExt;
    use latte_work_protocol::{MAX_FRAME, VERSION};
    use serde_json::{Value, json};
    use tokio_util::codec::{FramedRead, LinesCodec};

    let root = temporary_state();
    let state = root.path().join("state");
    let mut daemon = Command::new(SERVER)
        .args(["serve", "--state-dir"])
        .arg(&state)
        .env(
            "LATTE_WORK_CLAUDE",
            env!("CARGO_BIN_EXE_native-claude-fixture"),
        )
        .env("LATTE_WORK_AGENT_ENV", "inherit")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    // Wait for this fixture-backed daemon before allowing the bridge to auto-start one.
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            assert!(daemon.try_wait().unwrap().is_none());
            match Endpoint::open(&state) {
                Ok(endpoint) => match endpoint.connect(Duration::from_secs(1)).await {
                    Ok(_) => break,
                    Err(error) if latte_work_platform::ipc::daemon_absent(&error) => {}
                    Err(error) => panic!("{error}"),
                },
                Err(error) if latte_work_platform::ipc::daemon_absent(&error) => {}
                Err(error) => panic!("{error}"),
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let mut process = bridge("connect-local", &state);
    let mut output = FramedRead::new(
        process.stdout.take().unwrap(),
        LinesCodec::new_with_max_length(MAX_FRAME),
    );
    async fn exchange(
        process: &mut Child,
        output: &mut FramedRead<tokio::process::ChildStdout, LinesCodec>,
        request: Value,
    ) -> Value {
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        process
            .stdin
            .as_mut()
            .unwrap()
            .write_all(&bytes)
            .await
            .unwrap();
        let line = tokio::time::timeout(Duration::from_secs(15), output.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        serde_json::from_str(&line).unwrap()
    }
    let hello = exchange(
        &mut process,
        &mut output,
        json!({"method":"hello","version":VERSION}),
    )
    .await;
    assert_eq!(hello["kind"], "hello", "{hello}");
    assert_eq!(hello["agents"][0]["available"], true, "{hello}");
    assert!(
        hello["agents"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("native-fixture")
    );
    let status = exchange(&mut process, &mut output, json!({"method":"server_status"})).await;
    assert_eq!(status["kind"], "server_status", "{status}");
    let result = exchange(
        &mut process,
        &mut output,
        json!({"method":"prepare_upgrade","server_id":status["server_id"]}),
    )
    .await;
    assert_eq!(result["kind"], "upgrade_ready", "{result}");
    assert!(process.stdin.is_some());
    assert!(
        // Server retains its existing three-second graceful drain before IPC EOF.
        tokio::time::timeout(Duration::from_secs(6), output.next())
            .await
            .expect("upgraded bridge stdout must close with parent stdin open")
            .is_none()
    );
    assert!(
        tokio::time::timeout(EXIT_TIMEOUT, process.wait())
            .await
            .expect("upgraded bridge must terminate with parent stdin open")
            .unwrap()
            .success()
    );
    assert!(
        tokio::time::timeout(EXIT_TIMEOUT, daemon.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(Endpoint::open(&state).unwrap().try_lock_server().is_ok());
}
