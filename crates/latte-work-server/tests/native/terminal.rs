use super::*;
use latte_work_protocol::lifecycle::Resource;

async fn wait_marker(path: &Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
fn locked(path: &Path) -> bool {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    matches!(file.try_lock(), Err(std::fs::TryLockError::WouldBlock))
}
async fn tree(client: &mut Client, id: &str, fixture: &Path, dir: &Path) {
    std::fs::create_dir(dir).unwrap();
    let command = format!(
        "& '{}' --terminal-tree '{}'\r",
        fixture.display().to_string().replace('\'', "''"),
        dir.display().to_string().replace('\'', "''")
    );
    write(client, id, command.as_bytes()).await;
    wait_marker(&dir.join("root.ready")).await;
    wait_marker(&dir.join("leaf.ready")).await;
    assert!(locked(&dir.join("root.lock")) && locked(&dir.join("leaf.lock")));
}
async fn released(dir: &Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while locked(&dir.join("root.lock")) || locked(&dir.join("leaf.lock")) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn windows_terminal_owned_close_and_server_crash_reap_descendants_without_replay() {
    let mut host = Host::start().await;
    let mut client = host.client().await;
    let added = ask(
        &mut client,
        json!({"method":"add_project","path":host.root.path(),"name":null}),
    )
    .await;
    let project = &added["project"]["id"];
    let first = uuid::Uuid::new_v4().to_string();
    let second = uuid::Uuid::new_v4().to_string();
    let resources = vec![
        Resource::Terminal(first.clone()),
        Resource::Terminal(second.clone()),
    ];
    client
        .claim_resources("app-current".into(), resources.clone())
        .await
        .unwrap();
    for id in [&first, &second] {
        let response = ask(&mut client, json!({"method":"create_terminal","project_id":project,"terminal_id":id,"cols":80,"rows":24})).await;
        assert_eq!(response["kind"], "terminal", "{response}");
    }
    let first_dir = host.root.path().join("first-tree");
    let second_dir = host.root.path().join("second-tree");
    tree(&mut client, &first, &host.fixture, &first_dir).await;
    tree(&mut client, &second, &host.fixture, &second_dir).await;
    let server = client.server_id().to_owned();
    client
        .close_owned_resources(server.clone(), "app-stale".into(), resources.clone())
        .await
        .unwrap();
    assert!(locked(&first_dir.join("leaf.lock")) && locked(&second_dir.join("leaf.lock")));
    let start = std::time::Instant::now();
    client
        .close_owned_resources(server.clone(), "app-current".into(), resources)
        .await
        .unwrap();
    assert!(start.elapsed() < Duration::from_secs(6));
    released(&first_dir).await;
    released(&second_dir).await;
    let crashed = uuid::Uuid::new_v4().to_string();
    let response = ask(&mut client, json!({"method":"create_terminal","project_id":project,"terminal_id":crashed,"cols":80,"rows":24})).await;
    assert_eq!(response["kind"], "terminal", "{response}");
    let crash_dir = host.root.path().join("crash-tree");
    tree(&mut client, &crashed, &host.fixture, &crash_dir).await;
    host.process.kill().await.unwrap();
    released(&crash_dir).await;
    drop(client);
    host.process = Host::spawn(host.root.path(), &host.fixture);
    host.ready().await;
    let mut client = host.client().await;
    assert_ne!(client.server_id(), server);
    let listed = ask(
        &mut client,
        json!({"method":"terminals","project_id":project}),
    )
    .await;
    assert_eq!(listed["terminals"], json!([]));
    let old = ask(
        &mut client,
        json!({"method":"write_terminal","terminal_id":crashed,"data":[120]}),
    )
    .await;
    assert_eq!(old["kind"], "error");
    host.stop(&mut client).await;
}
#[tokio::test]
async fn windows_terminal_finds_pwsh_in_known_install_when_path_is_empty() {
    let mut host = Host::start().await;
    let mut client = host.client().await;
    host.stop(&mut client).await;
    drop(client);
    host.process = Host::command(host.root.path(), &host.fixture)
        .env("PATH", host.root.path().join("empty-path"))
        .spawn()
        .unwrap();
    host.ready().await;
    let mut client = host.client().await;
    let added = ask(
        &mut client,
        json!({"method":"add_project","path":host.root.path(),"name":null}),
    )
    .await;
    let response = ask(&mut client, json!({"method":"create_terminal","project_id":added["project"]["id"],"terminal_id":uuid::Uuid::new_v4().to_string(),"cols":80,"rows":24})).await;
    assert_eq!(response["kind"], "terminal", "{response}");
    assert_eq!(response["terminal"]["title"], "pwsh");
    let closed = ask(
        &mut client,
        json!({"method":"close_terminal","terminal_id":response["terminal"]["id"]}),
    )
    .await;
    assert_eq!(closed["kind"], "ok", "{closed}");
    host.stop(&mut client).await;
}
async fn write(client: &mut Client, id: &str, data: &[u8]) {
    let response = ask(
        client,
        json!({"method":"write_terminal","terminal_id":id,"data":data}),
    )
    .await;
    assert_eq!(response["kind"], "ok", "{response}");
}
async fn output_until(client: &mut Client, id: &str, cursor: &mut f64, needle: &str) -> String {
    let mut bytes = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let response = ask(
                client,
                json!({"method":"read_terminal","terminal_id":id,"after":cursor}),
            )
            .await;
            assert_eq!(response["kind"], "terminal_output", "{response}");
            let chunk: Vec<u8> = serde_json::from_value(response["data"].clone()).unwrap();
            bytes.extend(chunk);
            assert!(bytes.len() <= 1024 * 1024);
            *cursor = response["next"].as_f64().unwrap();
            if String::from_utf8_lossy(&bytes).contains(needle) {
                return String::from_utf8_lossy(&bytes).into_owned();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    result.unwrap_or_else(|error| {
        panic!(
            "waiting for {needle:?}: {error}; output={:?}",
            String::from_utf8_lossy(&bytes)
        )
    })
}
#[tokio::test]
async fn windows_terminal_pwsh_cwd_resize_unicode_reconnect_interrupt_and_exit() {
    let mut host = Host::start().await;
    let mut client = host.client().await;
    let project = host.root.path().join("中文 工作目录");
    std::fs::create_dir(&project).unwrap();
    let added = ask(
        &mut client,
        json!({"method":"add_project","path":project,"name":null}),
    )
    .await;
    let project_id = &added["project"]["id"];
    let id = uuid::Uuid::new_v4().to_string();
    let create = json!({"method":"create_terminal","project_id":project_id,"terminal_id":id,"cols":80,"rows":24});
    for _ in 0..2 {
        let response = ask(&mut client, create.clone()).await;
        assert_eq!(response["kind"], "terminal", "{response}");
        assert_eq!(response["terminal"]["title"], "pwsh");
    }
    assert!(!client.shutdown_if_idle().await.unwrap());
    let mut cursor = 0.0;
    write(
        &mut client,
        &id,
        "Write-Output ('READY_'+'中文🙂'); Write-Output ('CWD_'+(Get-Location).Path)\r".as_bytes(),
    )
    .await;
    let text = output_until(
        &mut client,
        &id,
        &mut cursor,
        &format!("CWD_{}", project.display()),
    )
    .await;
    assert!(text.contains("READY_中文🙂"), "{text}");
    let resized = ask(
        &mut client,
        json!({"method":"resize_terminal","terminal_id":id,"cols":103,"rows":37}),
    )
    .await;
    assert_eq!(resized["kind"], "ok", "{resized}");
    write(&mut client, &id, b"Write-Output ('DIM_'+$Host.UI.RawUI.WindowSize.Width+'x'+$Host.UI.RawUI.WindowSize.Height)\r").await;
    output_until(&mut client, &id, &mut cursor, "DIM_103x37").await;
    drop(client);
    let mut client = host.client().await;
    write(
        &mut client,
        &id,
        "Write-Output ('RECONNECTED_'+'中文🙂')\r".as_bytes(),
    )
    .await;
    output_until(&mut client, &id, &mut cursor, "RECONNECTED_中文🙂").await;
    write(
        &mut client,
        &id,
        b"Write-Output ('SLEEP_'+'STARTED'); Start-Sleep -Seconds 60\r",
    )
    .await;
    output_until(&mut client, &id, &mut cursor, "SLEEP_STARTED").await;
    write(&mut client, &id, b"\x03").await;
    output_until(&mut client, &id, &mut cursor, "目录> ").await;
    write(&mut client, &id, b"Write-Output ('AFTER_'+'INTERRUPT')\r").await;
    output_until(&mut client, &id, &mut cursor, "AFTER_INTERRUPT").await;
    write(&mut client, &id, b"pwsh -NoLogo -NoProfile -Command \"Write-Output ('INNER_'+'DONE'); exit 9\"; Write-Output ('PARENT_'+'ALIVE')\r").await;
    output_until(&mut client, &id, &mut cursor, "PARENT_ALIVE").await;
    let alive = ask(
        &mut client,
        json!({"method":"read_terminal","terminal_id":id,"after":cursor}),
    )
    .await;
    assert_eq!(alive["terminal"]["exited"], false);
    cursor = alive["next"].as_f64().unwrap();
    write(&mut client, &id, b"exit 7\r").await;
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let response = ask(
                &mut client,
                json!({"method":"read_terminal","terminal_id":id,"after":cursor}),
            )
            .await;
            assert_eq!(response["kind"], "terminal_output", "{response}");
            cursor = response["next"].as_f64().unwrap();
            if response["terminal"]["exited"] == true {
                assert_eq!(response["terminal"]["exit_code"], 7);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    for _ in 0..2 {
        assert_eq!(
            ask(
                &mut client,
                json!({"method":"close_terminal","terminal_id":id})
            )
            .await["kind"],
            "ok"
        );
    }
    host.stop(&mut client).await;
}
