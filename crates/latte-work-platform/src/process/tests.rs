use super::*;
use crate::lock::FileLock;
use std::{
    fs::TryLockError,
    path::{Path, PathBuf},
};

fn fixture(mode: &str, dir: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "process::tests::child_fixture", "--nocapture"])
        .env("LATTE_PROCESS_TEST_MODE", mode)
        .env("LATTE_PROCESS_TEST_DIR", dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

const FIXTURE_STDERR_LIMIT: usize = 16 * 1024;
const STDERR_TRUNCATED: &str = "\n[stderr truncated at 16384 bytes]";

async fn fixture_status_with_stderr(
    command: &mut Command,
    run: impl AsyncFnOnce(&mut Child) -> io::Result<ExitStatus>,
) -> (io::Result<ExitStatus>, String) {
    let mut child = match command.stderr(Stdio::piped()).kill_on_drop(true).spawn() {
        Ok(child) => child,
        Err(error) => return (Err(error), String::new()),
    };
    let mut stderr = child.stderr.take().expect("piped child stderr missing");
    let mut bytes = Vec::with_capacity(FIXTURE_STDERR_LIMIT);
    let mut truncated = false;
    let (status, stderr_result, cleanup_result) = {
        let drain = async {
            let mut chunk = [0; 4096];
            loop {
                let count = stderr.read(&mut chunk).await?;
                if count == 0 {
                    return io::Result::Ok(());
                }
                let retained = count.min(FIXTURE_STDERR_LIMIT - bytes.len());
                bytes.extend_from_slice(&chunk[..retained]);
                truncated |= retained < count;
            }
        };
        tokio::pin!(drain);
        let (status, drained) = {
            let running = run(&mut child);
            tokio::pin!(running);
            tokio::select! {
                status = &mut running => (status, None),
                result = &mut drain => (running.await, Some(result)),
            }
        };
        let cleanup_result = if status.is_err() {
            // Resolve child ownership before any caller assertion can panic.
            match child.start_kill() {
                Ok(()) => fixture_wait(&mut child, Duration::from_secs(3))
                    .await
                    .map(|_| ()),
                Err(error) => Err(error),
            }
        } else {
            Ok(())
        };
        let stderr_result = match drained {
            Some(result) => result,
            None => tokio::time::timeout(Duration::from_secs(1), &mut drain)
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "stderr drain timed out"))
                .and_then(|result| result),
        };
        (status, stderr_result, cleanup_result)
    };
    let mut diagnostic = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        diagnostic.push_str(STDERR_TRUNCATED);
    }
    if let Err(error) = stderr_result {
        diagnostic.push_str(&format!("\n[stderr collection failed: {error}]"));
    }
    if let Err(error) = cleanup_result {
        diagnostic.push_str(&format!("\n[fixture cleanup failed: {error}]"));
    }
    (status, diagnostic)
}

async fn fixture_wait(child: &mut Child, timeout: Duration) -> io::Result<ExitStatus> {
    tokio::time::timeout(timeout, child.wait())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "fixture wait timed out"))?
}

#[tokio::test]
async fn fixture_stderr_preserves_failure_and_drains_past_capture_limit() {
    let dir = tempfile::tempdir().unwrap();
    let (status, stderr) =
        fixture_status_with_stderr(&mut fixture("stderr-failure", dir.path()), async |child| {
            fixture_wait(child, Duration::from_secs(5)).await
        })
        .await;
    assert!(!status.unwrap().success(), "{stderr}");
    assert!(stderr.starts_with("fixture failure detail\n"), "{stderr}");
    assert!(stderr.ends_with(STDERR_TRUNCATED), "{stderr}");
    assert_eq!(stderr.len(), FIXTURE_STDERR_LIMIT + STDERR_TRUNCATED.len());
}

#[tokio::test]
async fn fixture_stderr_timeout_reaps_child_and_preserves_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let (status, stderr) =
        fixture_status_with_stderr(&mut fixture("stderr-timeout", dir.path()), async |child| {
            fixture_ready(dir.path()).await?;
            fixture_wait(child, Duration::from_millis(100)).await
        })
        .await;
    assert_eq!(status.unwrap_err().kind(), io::ErrorKind::TimedOut);
    assert_eq!(stderr, "fixture timeout detail\n");
    released(dir.path()).await;
}

async fn fixture_ready(dir: &Path) -> io::Result<()> {
    tokio::time::timeout(Duration::from_secs(8), async {
        while !dir.join("ready").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "child never became ready"))
}

async fn ready(dir: &Path) {
    fixture_ready(dir).await.expect("child never became ready");
}
async fn released(dir: &Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match FileLock::try_acquire(&dir.join("alive.lock")) {
                Ok(_) => break,
                Err(TryLockError::WouldBlock) => {
                    tokio::time::sleep(Duration::from_millis(10)).await
                }
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await
    .expect("descendant survived cleanup");
}
fn alive(dir: &Path) {
    assert!(matches!(
        FileLock::try_acquire(&dir.join("alive.lock")),
        Err(TryLockError::WouldBlock)
    ));
}

#[tokio::test]
async fn close_cleans_descendants_after_root_exit_and_is_repeatable() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = ManagedChild::spawn(&mut fixture("orphan", dir.path())).unwrap();
    ready(dir.path()).await;
    assert!(
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    alive(dir.path());
    child.close(Duration::from_millis(50)).await.unwrap();
    released(dir.path()).await;
    child.close(Duration::ZERO).await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn failed_close_retains_tree_for_later_retry() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = ManagedChild::spawn(&mut fixture("tree", dir.path())).unwrap();
    ready(dir.path()).await;
    child.tree.fail_kill(Some(nix::errno::Errno::EIO));
    let error = child.close(Duration::ZERO).await.unwrap_err();
    assert!(error.to_string().contains("SIGKILL"), "{error}");
    assert!(!child.closed);
    alive(dir.path());
    child.tree.fail_kill(None);
    child.close(Duration::ZERO).await.unwrap();
    released(dir.path()).await;
    assert!(child.closed);
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn cancelled_permission_retry_retains_tree_for_later_close() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = ManagedChild::spawn(&mut fixture("tree", dir.path())).unwrap();
    ready(dir.path()).await;
    child.tree.fail_kill(Some(nix::errno::Errno::EPERM));
    {
        let close = child.close(Duration::ZERO);
        tokio::pin!(close);
        tokio::select! {
            result = &mut close => panic!("close completed before cancellation: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(25)) => {},
        }
    }
    assert!(!child.closed);
    alive(dir.path());
    child.tree.fail_kill(None);
    child.close(Duration::ZERO).await.unwrap();
    released(dir.path()).await;
}
#[tokio::test]
async fn dropping_or_aborting_owner_cleans_tree() {
    for abort in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let child = ManagedChild::spawn(&mut fixture("tree", dir.path())).unwrap();
        ready(dir.path()).await;
        alive(dir.path());
        if abort {
            let task = tokio::spawn(async move {
                let _child = child;
                std::future::pending::<()>().await;
            });
            task.abort();
            let _ = task.await;
        } else {
            drop(child);
        }
        released(dir.path()).await;
    }
}
#[tokio::test]
async fn probes_bound_output_and_timeout_and_clean_descendants() {
    let dir = tempfile::tempdir().unwrap();
    let error = output(
        &mut fixture("flood", dir.path()),
        Duration::from_secs(3),
        128,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("limit"), "{error}");
    let error = output(
        &mut fixture("tree", dir.path()),
        Duration::from_millis(800),
        4096,
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    ready(dir.path()).await;
    released(dir.path()).await;
}
#[tokio::test]
async fn native_unicode_path_arguments_and_environment_survive_supervision() {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("进程 空格.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    let mut command = Command::new(executable);
    command
        .args(["--exact", "process::tests::child_fixture", "--nocapture"])
        .env("LATTE_PROCESS_TEST_MODE", "echo-env")
        .env("LATTE_PROCESS_TEST_DIR", dir.path())
        .env("LATTE_PROCESS_TEST_VALUE", "路径 with spaces & symbols");
    let (status, bytes) = output(&mut command, Duration::from_secs(5), 4096)
        .await
        .unwrap();
    assert!(status.success());
    assert!(
        String::from_utf8(bytes)
            .unwrap()
            .contains("路径 with spaces & symbols")
    );
}

#[tokio::test]
async fn launch_error_is_not_a_successful_child() {
    let dir = tempfile::tempdir().unwrap();
    assert!(ManagedChild::spawn(&mut Command::new(dir.path().join("missing.exe"))).is_err());
}
#[tokio::test]
async fn daemon_survives_bridge_exit_and_handle_drop() {
    for forced in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mode = if forced { "bridge-hold" } else { "bridge" };
        let mut bridge = fixture(mode, dir.path())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        ready(dir.path()).await;
        if forced {
            bridge.kill().await.unwrap();
        } else {
            assert!(
                tokio::time::timeout(Duration::from_secs(5), bridge.wait())
                    .await
                    .unwrap()
                    .unwrap()
                    .success()
            );
        }
        alive(dir.path());
        let endpoint = crate::ipc::Endpoint::open(dir.path()).unwrap();
        let mut stream = endpoint.connect(Duration::from_secs(2)).await.unwrap();
        use tokio::io::AsyncWriteExt;
        stream.write_all(b"ping").await.unwrap();
        let mut pong = [0; 4];
        tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut pong))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&pong, b"pong");
        assert!(matches!(
            endpoint.try_lock_server(),
            Err(TryLockError::WouldBlock)
        ));
        // The daemon must not inherit its bridge's lock handle.
        let _released_bridge = FileLock::try_acquire(&dir.path().join("bridge.lock")).unwrap();
        stream.write_all(b"stop").await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut pong))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&pong, b"done");
        drop(stream);
        released(dir.path()).await;
        let _singleton = endpoint.try_lock_server().unwrap();
    }
}

// Same executable, real OS processes; lock release is kernel evidence of exit,
// independent of PID reuse, stdout buffering and process enumeration permissions.
#[test]
fn child_fixture() {
    let Some(mode) = std::env::var_os("LATTE_PROCESS_TEST_MODE") else {
        return;
    };
    let dir = PathBuf::from(std::env::var_os("LATTE_PROCESS_TEST_DIR").unwrap());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        match mode.to_str().unwrap() {
            "leaf" => {
                let _lock = FileLock::try_acquire(&dir.join("alive.lock")).unwrap();
                std::fs::write(dir.join("ready"), b"").unwrap();
                // Safety ceiling even if a broken implementation fails to clean up.
                tokio::time::timeout(Duration::from_secs(20), async {
                    while !dir.join("stop").exists() {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                })
                .await
                .ok();
            }
            "tree" | "orphan" => {
                let _grandchild = fixture("leaf", &dir).spawn().unwrap();
                ready(&dir).await;
                if mode == "tree" {
                    tokio::time::sleep(Duration::from_secs(20)).await;
                }
            }
            "bridge" | "bridge-hold" => {
                let _lock = FileLock::try_acquire(&dir.join("bridge.lock")).unwrap();
                drop(spawn_daemon(&mut fixture("daemon", &dir)).unwrap());
                if mode == "bridge-hold" {
                    tokio::time::sleep(Duration::from_secs(20)).await;
                }
            }
            "daemon" => {
                let endpoint = crate::ipc::Endpoint::prepare(&dir).unwrap();
                let listener = endpoint.bind().await.unwrap();
                let lock = FileLock::try_acquire(&dir.join("alive.lock")).unwrap();
                std::fs::write(dir.join("ready"), b"").unwrap();
                tokio::time::timeout(Duration::from_secs(15), async {
                    use tokio::io::AsyncWriteExt;
                    let mut stream = listener.accept().await.unwrap();
                    let mut bytes = [0; 4];
                    stream.read_exact(&mut bytes).await.unwrap();
                    assert_eq!(&bytes, b"ping");
                    stream.write_all(b"pong").await.unwrap();
                    stream.read_exact(&mut bytes).await.unwrap();
                    assert_eq!(&bytes, b"stop");
                    stream.write_all(b"done").await.unwrap();
                    // Keep the pipe open until the client consumes the acknowledgment.
                    assert_eq!(stream.read(&mut bytes).await.unwrap_or(0), 0);
                })
                .await
                .unwrap();
                drop(listener);
                drop(lock);
            }
            "signal" => {
                let mut shutdown = Shutdown::new().unwrap();
                let mut child = ManagedChild::spawn(&mut fixture("tree", &dir)).unwrap();
                tokio::time::timeout(Duration::from_secs(15), shutdown.recv())
                    .await
                    .unwrap();
                child.close(Duration::from_millis(100)).await.unwrap();
                std::fs::write(dir.join("stopped"), b"").unwrap();
            }
            "echo-env" => {
                println!("{}", std::env::var("LATTE_PROCESS_TEST_VALUE").unwrap());
            }
            "flood" => {
                println!("{}", "x".repeat(16_384));
            }
            "stderr-failure" => {
                use std::io::Write;
                let mut stderr = std::io::stderr().lock();
                stderr.write_all(b"fixture failure detail\n").unwrap();
                stderr.write_all(&[b'x'; 128 * 1024]).unwrap();
                panic!("fixture failed after stderr flood");
            }
            "stderr-timeout" => {
                let _lock = FileLock::try_acquire(&dir.join("alive.lock")).unwrap();
                eprintln!("fixture timeout detail");
                std::fs::write(dir.join("ready"), b"").unwrap();
                tokio::time::sleep(Duration::from_secs(20)).await;
            }
            "supervisor" => {
                let _child = ManagedChild::spawn(&mut fixture("tree", &dir)).unwrap();
                ready(&dir).await;
                std::fs::write(dir.join("supervisor-ready"), b"").unwrap();
                tokio::time::sleep(Duration::from_secs(20)).await;
            }
            _ => panic!("unknown fixture"),
        }
    });
}

#[cfg(windows)]
#[test]
fn scripts_are_rejected_before_launch() {
    let dir = tempfile::tempdir().unwrap();
    for extension in ["cmd", "BAT", "ps1"] {
        let script = dir.path().join(format!("agent.{extension}"));
        std::fs::write(&script, "exit 0").unwrap();
        let error = ManagedChild::spawn(&mut Command::new(script)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn termination_notification_runs_owned_cleanup() {
    use nix::{
        sys::signal::{Signal, kill},
        unistd::Pid,
    };
    let dir = tempfile::tempdir().unwrap();
    let (status, stderr) =
        fixture_status_with_stderr(&mut fixture("signal", dir.path()), async |host| {
            fixture_ready(dir.path()).await?;
            let pid = host
                .id()
                .ok_or_else(|| io::Error::other("host PID missing"))?;
            kill(Pid::from_raw(pid as i32), Signal::SIGTERM)?;
            fixture_wait(host, Duration::from_secs(5)).await
        })
        .await;
    let status =
        status.unwrap_or_else(|error| panic!("signal fixture failed: {error}; stderr:\n{stderr}"));
    assert!(
        status.success(),
        "signal fixture exited with {status}; stderr:\n{stderr}"
    );
    assert!(dir.path().join("stopped").exists());
    released(dir.path()).await;
}

#[cfg(windows)]
#[tokio::test]
async fn supervisor_crash_closes_job_and_nested_jobs_work() {
    let dir = tempfile::tempdir().unwrap();
    // No outer managed Job: only the crashing supervisor owns the Job handle.
    let mut supervisor = fixture("supervisor", dir.path())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(8), async {
        while !dir.path().join("supervisor-ready").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    alive(dir.path());
    supervisor.kill().await.unwrap();
    released(dir.path()).await;

    let nested = tempfile::tempdir().unwrap();
    let mut outer = ManagedChild::spawn(&mut fixture("supervisor", nested.path())).unwrap();
    ready(nested.path()).await;
    outer.close(Duration::ZERO).await.unwrap();
    released(nested.path()).await;
}
