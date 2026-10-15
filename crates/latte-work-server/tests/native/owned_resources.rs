use super::*;
use latte_work_protocol::lifecycle::Resource;

async fn open_session(client: &mut Client, project: &Path) -> String {
    let session = create_session(client, project).await;
    let opened = ask(
        client,
        json!({"method":"open_agent_session","session_id":session}),
    )
    .await;
    assert_eq!(opened["kind"], "ok", "{opened}");
    assert_session_open(client, &session, true).await;
    session
}

async fn assert_session_open(client: &mut Client, session: &str, open: bool) {
    let state = ask(
        client,
        json!({"method":"poll","session_id":session,"after":0}),
    )
    .await;
    assert_eq!(state["session"]["agent_session_open"], open, "{state}");
}

#[tokio::test]
async fn duplicate_live_agent_close_succeeds_and_remains_idempotent() {
    let mut host = Host::start().await;
    let mut client = host.client().await;
    let session = open_session(&mut client, host.root.path()).await;
    let resource = Resource::AgentSession(session.clone());
    let duplicates = vec![resource.clone(), resource.clone()];
    let server_id = client.server_id().to_owned();
    client
        .claim_resources("app-current".into(), duplicates.clone())
        .await
        .unwrap();

    // Deduplication must not relax the input bound or malformed-ID validation.
    for invalid in [
        vec![resource.clone(); 129],
        vec![resource.clone(), Resource::AgentSession(String::new())],
    ] {
        assert!(
            client
                .close_owned_resources(server_id.clone(), "app-current".into(), invalid)
                .await
                .is_err()
        );
        assert_session_open(&mut client, &session, true).await;
    }

    client
        .close_owned_resources(server_id.clone(), "app-current".into(), duplicates.clone())
        .await
        .unwrap();
    assert_session_open(&mut client, &session, false).await;
    for resources in [duplicates, vec![resource]] {
        client
            .close_owned_resources(server_id.clone(), "app-current".into(), resources)
            .await
            .unwrap();
    }
    assert!(!host.root.path().join("prompts.log").exists());
    host.stop(&mut client).await;
}

#[cfg(windows)]
#[tokio::test]
async fn duplicate_mixed_close_preserves_resource_kind_owner_and_server_fences() {
    let mut host = Host::start().await;
    let mut client = host.client().await;
    let first = open_session(&mut client, host.root.path()).await;
    let second = open_session(&mut client, host.root.path()).await;
    let foreign = open_session(&mut client, host.root.path()).await;
    let added = ask(
        &mut client,
        json!({"method":"add_project","path":host.root.path(),"name":null}),
    )
    .await;
    // Equal IDs across resource kinds must still close both resources.
    let terminal = ask(
        &mut client,
        json!({"method":"create_terminal","project_id":added["project"]["id"],
        "terminal_id":first,"cols":80,"rows":24}),
    )
    .await;
    assert_eq!(terminal["kind"], "terminal", "{terminal}");
    let resources = vec![
        Resource::AgentSession(first.clone()),
        Resource::Terminal(first.clone()),
        Resource::AgentSession(second.clone()),
        Resource::AgentSession(first.clone()),
        Resource::AgentSession(second.clone()),
        Resource::Terminal(first.clone()),
    ];
    client
        .claim_resources("app-current".into(), resources.clone())
        .await
        .unwrap();
    let foreign_resource = Resource::AgentSession(foreign.clone());
    client
        .claim_resources("app-foreign".into(), vec![foreign_resource.clone()])
        .await
        .unwrap();
    let server_id = client.server_id().to_owned();
    for (server, owner) in [
        ("stale-server".to_owned(), "app-current"),
        (server_id.clone(), "stale-owner"),
    ] {
        client
            .close_owned_resources(server, owner.into(), resources.clone())
            .await
            .unwrap();
        assert_session_open(&mut client, &first, true).await;
        assert_session_open(&mut client, &second, true).await;
        let listed = ask(
            &mut client,
            json!({"method":"terminals","project_id":added["project"]["id"]}),
        )
        .await;
        assert_eq!(listed["terminals"].as_array().unwrap().len(), 1);
    }
    let mut mixed = resources.clone();
    mixed.extend([foreign_resource.clone(), foreign_resource.clone()]);
    client
        .close_owned_resources(server_id.clone(), "app-current".into(), mixed)
        .await
        .unwrap();
    assert_session_open(&mut client, &first, false).await;
    assert_session_open(&mut client, &second, false).await;
    assert_session_open(&mut client, &foreign, true).await;
    let listed = ask(
        &mut client,
        json!({"method":"terminals","project_id":added["project"]["id"]}),
    )
    .await;
    assert_eq!(listed["terminals"], json!([]), "{listed}");
    client
        .close_owned_resources(server_id.clone(), "app-current".into(), resources)
        .await
        .unwrap();
    client
        .close_owned_resources(server_id, "app-foreign".into(), vec![foreign_resource])
        .await
        .unwrap();
    assert_session_open(&mut client, &foreign, false).await;
    assert!(!host.root.path().join("prompts.log").exists());
    host.stop(&mut client).await;
}
