//! The askpass helper is this same executable. This spawns the real binary and
//! serves one password over the private platform pipe, proving the handshake.
#![cfg(windows)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

#[path = "../src/askpass.rs"]
mod askpass;

/// Run through scripts/smoke-windows-ssh-password.py. The fixture listens only on
/// localhost, pins an ephemeral host key and accepts a public test password.
#[tokio::test]
#[ignore = "requires the localhost SSH password fixture"]
async fn openssh_password_uses_the_real_askpass_helper() {
    use latte_work_client::{SshAuthentication, SshPlatform, ssh_command_for_platform};
    let port = std::env::var("LATTE_TEST_SSH_PORT")
        .unwrap()
        .parse()
        .unwrap();
    let config = std::env::var_os("LATTE_TEST_SSH_CONFIG").unwrap();
    let executable = std::env::var_os("LATTE_TEST_ASKPASS_EXE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_latte-work-desktop").into());
    let ssh = std::env::var_os("LATTE_TEST_SSH_EXE").unwrap();
    let challenge = askpass::Askpass::start("latte-fixture-password".into()).unwrap();
    let generated = ssh_command_for_platform(
        "fixture@127.0.0.1",
        r"C:\fixture\server.exe",
        Some(port),
        SshAuthentication::Password {
            askpass: &executable,
            socket: challenge.target(),
        },
        SshPlatform::Windows,
    )
    .unwrap();
    let mut command = tokio::process::Command::new(ssh);
    command
        .args([std::ffi::OsStr::new("-F"), &config])
        .args(generated.as_std().get_args());
    for (key, value) in generated.as_std().get_envs() {
        if let Some(value) = value {
            command.env(key, value);
        }
    }
    let mut child = command
        .creation_flags(0x0800_0000)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"method\":\"hello\",\"version\":1}\n")
        .await
        .unwrap();
    let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .expect("SSH password fixture timed out")
        .unwrap();
    assert!(
        output.status.success(),
        "SSH stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let hello: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(hello["server_id"], "ssh-password-fixture");
}

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
