use super::*;

fn fixture_command(fail: bool) -> Command {
    #[cfg(windows)]
    {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let end = if fail {
            "[Console]::Error.WriteLine('fixture authentication failed'); exit 1"
        } else {
            "[Console]::Out.WriteLine('{\"kind\":\"hello\",\"version\":1,\"server_id\":\"diagnostic-fixture\",\"agents\":[]}'); [Console]::In.ReadLine() > $null; [Console]::Error.WriteLine('post-handshake-private-value'); exit 0"
        };
        let script = format!(
            "[Console]::Error.WriteLine('discarded-initial-marker'); [Console]::Error.Write(('x' * 262144)); {end}"
        );
        let bytes: Vec<_> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
        ]);
        command.arg(STANDARD.encode(bytes));
        command
    }
    #[cfg(unix)]
    {
        let end = if fail {
            "printf '\\nfixture authentication failed\\n' >&2; exit 1"
        } else {
            "printf '%s\\n' '{\"kind\":\"hello\",\"version\":1,\"server_id\":\"diagnostic-fixture\",\"agents\":[]}'; read request; printf 'post-handshake-private-value\\n' >&2"
        };
        let mut command = Command::new("/bin/sh");
        command.args(["-c", &format!(
            "printf 'discarded-initial-marker\\n' >&2; head -c 262144 /dev/zero | tr '\\000' x >&2; {end}"
        )]);
        command
    }
}

#[tokio::test]
async fn failed_startup_reports_bounded_stderr_without_blocking_the_child() {
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        Client::spawn(fixture_command(true)),
    )
    .await
    .unwrap();
    let error = match result {
        Ok(_) => panic!("fixture should not complete Hello"),
        Err(error) => format!("{error:#}"),
    };
    assert!(error.contains("fixture authentication failed"), "{error}");
    assert!(!error.contains("discarded-initial-marker"));
    assert!(error.len() < 9000);
}

#[tokio::test]
async fn successful_startup_discards_diagnostics_and_still_drains_stderr() {
    let mut client = tokio::time::timeout(
        Duration::from_secs(10),
        Client::spawn(fixture_command(false)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(client.server_id(), "diagnostic-fixture");
    assert_eq!(client.diagnostics.failure_detail().await, "");
}
