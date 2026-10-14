//! The askpass helper is this same executable. This spawns the real binary and
//! serves one password over the private platform pipe, proving the handshake.
#![cfg(windows)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

#[test]
fn askpass_helper_reads_the_password_from_a_private_pipe() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let pipe = runtime
        .block_on(async { latte_work_platform::pipe::PrivatePipe::bind("askpass-test").unwrap() });
    let mut child = Command::new(env!("CARGO_BIN_EXE_latte-work-desktop"))
        .env("LATTE_WORK_SSH_ASKPASS", "1")
        .env("LATTE_WORK_SSH_ASKPASS_SOCKET", pipe.name())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let accepted = runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(10), pipe.accept_once()).await
    });
    let Ok(Ok(mut stream)) = accepted else {
        let _ = child.kill();
        let output = child.wait_with_output().unwrap();
        panic!(
            "no authenticated client; helper stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    runtime.block_on(async {
        stream.write_all(b"test-secret\0").await.unwrap();
        stream.flush().await.unwrap();
        stream.evade_limbo();
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "askpass helper timed out; stderr: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "askpass helper failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"test-secret");
}
