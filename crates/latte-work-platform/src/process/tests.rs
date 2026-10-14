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
async fn ready(dir: &Path) {
    tokio::time::timeout(Duration::from_secs(8), async {
        while !dir.join("ready").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("child never became ready");
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
    let mut host = fixture("signal", dir.path())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    ready(dir.path()).await;
    kill(Pid::from_raw(host.id().unwrap() as i32), Signal::SIGTERM).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), host.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
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
