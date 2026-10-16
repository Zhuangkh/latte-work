use super::*;

#[tokio::test]
async fn connect_existing_reports_absent_daemon_without_starting_one() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let mut command = Command::new(SERVER);
    command
        .args(["connect-existing", "--state-dir"])
        .arg(&state)
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .unwrap()
        .unwrap();
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["code"], "server_unavailable");
    let endpoint = Endpoint::open(&state).unwrap();
    assert!(endpoint.try_lock_server().is_ok());
    assert!(!state.join("state.sqlite").exists());
}

#[cfg(windows)]
#[tokio::test]
async fn windows_ssh_shells_preserve_stdio_paths_and_independent_daemon() {
    use latte_work_client::{SshAuthentication, SshPlatform, ssh_command_for_platform};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    tokio::time::timeout(Duration::from_secs(60), async {
        let mut host = Host::start().await;
        let special = host
            .root
            .path()
            .join("工具 & %PATH% ! ^ $() ` ' ; ‘ ’ “ ” bin");
        std::fs::create_dir(&special).unwrap();
        let binary = special.join("server.exe");
        std::fs::copy(&host.fixture, &binary).unwrap();
        let command = ssh_command_for_platform(
            "fixture",
            binary.to_str().unwrap(),
            None,
            SshAuthentication::OpenSsh,
            SshPlatform::Windows,
        )
        .unwrap();
        let remote = command.as_std().get_args().last().unwrap().to_owned();
        let mut client = host.client().await;
        let identity = client.server_id().to_owned();
        let mut shells = vec!["cmd.exe", "powershell.exe"];
        if which::which("pwsh.exe").is_ok() {
            shells.push("pwsh.exe");
        }
        for shell in shells {
            let mut command = Command::new(shell);
            if shell == "cmd.exe" {
                command.args(["/d", "/s", "/c"]);
            } else {
                command.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]);
            }
            let mut bridge = command
                .arg(&remote)
                .env("LATTE_NATIVE_BRIDGE_SERVER", SERVER)
                .env("LATTE_NATIVE_BRIDGE_STATE", host.state())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let mut input = bridge.stdin.take().unwrap();
            let mut output = BufReader::new(bridge.stdout.take().unwrap()).lines();
            input
                .write_all(b"{\"method\":\"hello\",\"version\":1}\n")
                .await
                .unwrap();
            let line = tokio::time::timeout(Duration::from_secs(10), output.next_line())
                .await
                .unwrap_or_else(|_| panic!("hello timed out through {shell}"))
                .unwrap()
                .unwrap_or_else(|| panic!("EOF through {shell}"));
            let hello: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(hello["server_id"], identity, "{shell}: {hello}");
            assert_eq!(hello["path_syntax"], "windows");
            let project = host.root.path().join(format!("项目 {shell}"));
            std::fs::create_dir(&project).unwrap();
            let mut request = serde_json::to_vec(
                &json!({"method":"add_project", "path":project, "name":"中文 🎉 & '$()"}),
            )
            .unwrap();
            request.push(b'\n');
            // A second interactive exchange proves stdin wasn't consumed at startup.
            input.write_all(&request).await.unwrap();
            let line = tokio::time::timeout(Duration::from_secs(10), output.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(line.contains("中文 🎉 & '$()"), "{shell}: {line}");
            drop(input);
            assert!(
                tokio::time::timeout(Duration::from_secs(10), bridge.wait())
                    .await
                    .unwrap()
                    .unwrap()
                    .success(),
                "{shell}"
            );
            assert!(host.process.try_wait().unwrap().is_none());
            let reconnected = host.client().await;
            assert_eq!(reconnected.server_id(), identity);
        }
        host.stop(&mut client).await;
    })
    .await
    .unwrap();
}
