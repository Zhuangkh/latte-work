//! Graceful App exit: close every owned Agent session/terminal, then drain an idle local daemon.
use crate::Connections;
use latte_work_protocol::{Request, Response};
use std::{
    collections::{BTreeMap, HashSet},
    future::Future,
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use tauri::{Emitter, Manager, State};

pub(crate) type CloseAck = (String, tokio::sync::oneshot::Sender<Result<(), String>>);

// Close is idempotent: a transport failure may lose its acknowledgement, so only
// this close request can be retried on a fresh bridge. Never replay a Send/Open.
pub(crate) async fn close_resource<F, Fut>(
    state: &Connections,
    host: &str,
    request: Request,
    reconnect: &F,
) -> Result<(), String>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<latte_work_client::Client, String>>,
{
    if !matches!(
        request,
        Request::CloseAgentSession { .. } | Request::CloseTerminal { .. }
    ) {
        return Err("退出清理只允许关闭资源".into());
    }
    let _cleanup = state.cleanup_admission.lock().await;
    let cached = state.clients.lock().await.get(host).cloned();
    if let Some(client) = cached {
        let mut client = client.lock().await;
        if !client.is_broken()
            && let Ok(response) = guarded_close(&mut client, state, host, request.clone()).await
        {
            return close_response(response);
        }
    }
    let client = Arc::new(tokio::sync::Mutex::new(reconnect(host.to_owned()).await?));
    state
        .clients
        .lock()
        .await
        .insert(host.to_owned(), client.clone());
    let response = guarded_close(&mut *client.lock().await, state, host, request).await?;
    close_response(response)
}
async fn guarded_close(
    client: &mut latte_work_client::Client,
    state: &Connections,
    host: &str,
    request: Request,
) -> Result<Response, String> {
    use latte_work_protocol::lifecycle::Resource;
    let resource = match &request {
        Request::CloseAgentSession { session_id, .. } => Resource::AgentSession(session_id.clone()),
        Request::CloseTerminal { terminal_id } => Resource::Terminal(terminal_id.clone()),
        _ => return Err("退出清理只允许关闭资源".into()),
    };
    let server = state
        .owned_instances
        .lock()
        .await
        .get(&(host.to_owned(), resource.clone()))
        .cloned();
    if let Some(server) = server {
        client
            .close_owned_resources(server, state.owner_id().to_owned(), vec![resource])
            .await
            .map_err(|e| format!("{e:#}"))?;
        Ok(Response::Ok)
    } else {
        client.request(request).await.map_err(|e| format!("{e:#}"))
    }
}
fn close_response(response: Response) -> Result<(), String> {
    match response {
        Response::Ok => Ok(()),
        Response::Error { message, .. } => Err(message),
        other => Err(format!("关闭响应格式不匹配：{other:?}")),
    }
}
async fn stop_owned<F, Fut>(state: &Connections, reconnect: &F) -> Result<(), String>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<latte_work_client::Client, String>>,
{
    // Wait for in-flight sends to reconcile before inspecting their sessions.
    let _admission = tokio::time::timeout(Duration::from_secs(1), state.admission.write())
        .await
        .ok();
    let mut failed_hosts = HashSet::new();
    let mut errors = Vec::new();
    let mut batches =
        BTreeMap::<(String, String), Vec<latte_work_protocol::lifecycle::Resource>>::new();
    for ((host, resource), server) in state.owned_instances.lock().await.clone() {
        batches.entry((host, server)).or_default().push(resource);
    }
    let mut batches: Vec<_> = batches.into_iter().collect();
    batches.sort_by_key(|((host, server), _)| (host != "local", host.clone(), server.clone()));
    for ((host, server), resources) in batches {
        let result = async {
            // Use a dedicated bridge: an in-flight reply or UI polling cannot
            // prevent the server from receiving the whole cleanup batch.
            let mut client = reconnect(host.clone()).await?;
            client
                .close_owned_resources(server, state.owner_id().to_owned(), resources.clone())
                .await
                .map_err(|e| format!("{e:#}"))
        }
        .await;
        match result {
            Ok(()) => {
                for resource in resources {
                    state
                        .owned_instances
                        .lock()
                        .await
                        .remove(&(host.clone(), resource.clone()));
                    match resource {
                        latte_work_protocol::lifecycle::Resource::AgentSession(id) => {
                            state.turns.lock().await.remove(&(host.clone(), id));
                        }
                        latte_work_protocol::lifecycle::Resource::Terminal(id) => {
                            state.terminals.lock().await.remove(&(host.clone(), id));
                        }
                    }
                }
            }
            Err(error) => {
                errors.push(format!("{host}：{error}"));
                failed_hosts.insert(host);
            }
        }
    }
    let mut turns: Vec<_> = state.turns.lock().await.keys().cloned().collect();
    // Settle local resources before potentially unavailable remote hosts.
    turns.sort_by_key(|(host, id)| (host != "local", host.clone(), id.clone()));
    for (host, session_id) in turns {
        if failed_hosts.contains(&host) {
            continue;
        }
        match close_resource(
            state,
            &host,
            Request::CloseAgentSession {
                session_id: session_id.clone(),
                only_if_idle: None,
            },
            reconnect,
        )
        .await
        {
            Ok(()) => {
                state.turns.lock().await.remove(&(host, session_id));
            }
            Err(error) => {
                errors.push(format!("{host}：{error}"));
                failed_hosts.insert(host);
            }
        }
    }
    let mut terminals: Vec<_> = state.terminals.lock().await.iter().cloned().collect();
    terminals.sort_by_key(|(host, id)| (host != "local", host.clone(), id.clone()));
    for (host, terminal_id) in terminals {
        if failed_hosts.contains(&host) {
            continue;
        }
        match close_resource(
            state,
            &host,
            Request::CloseTerminal {
                terminal_id: terminal_id.clone(),
            },
            reconnect,
        )
        .await
        {
            Ok(()) => {
                state.terminals.lock().await.remove(&(host, terminal_id));
            }
            Err(error) => {
                errors.push(format!("{host}：{error}"));
                failed_hosts.insert(host);
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}
async fn finish_cleanup(state: &Connections) {
    // Busy shared daemons remain alive. Optional idle shutdown cannot block exit.
    if let Some(local) = state.clients.lock().await.get("local").cloned() {
        let _ = tokio::time::timeout(Duration::from_secs(3), async {
            local.lock().await.shutdown_if_idle().await
        })
        .await;
    }
    state.clients.lock().await.clear();
    state.passwords.lock().await.clear();
}
#[tauri::command]
pub async fn app_close_ready(
    state: State<'_, Connections>,
    request_id: String,
    error: Option<String>,
) -> Result<(), String> {
    finish_close(&state, &request_id, error).await;
    Ok(())
}
async fn finish_close(state: &Connections, request_id: &str, error: Option<String>) {
    let mut pending = state.close_ack.lock().await;
    if pending.as_ref().is_some_and(|(id, _)| id == request_id)
        && let Some((_, reply)) = pending.take()
    {
        let _ = reply.send(error.map_or(Ok(()), Err));
    }
}
async fn close_tabs(app: &tauri::AppHandle, state: &Connections) -> Result<(), String> {
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos()
        .to_string();
    let (reply, result) = tokio::sync::oneshot::channel();
    *state.close_ack.lock().await = Some((id.clone(), reply));
    app.emit_to("main", "app-close-requested", &id)
        .map_err(|e| e.to_string())?;
    result
        .await
        .map_err(|_| "Tab 清理确认通道已关闭".to_string())?
}
fn exit(app: &tauri::AppHandle) {
    app.state::<Connections>()
        .exit_ready
        .store(true, Ordering::SeqCst);
    app.exit(0);
}
pub fn quit(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<Connections>();
        // Persist explicit closing intent before any RPC. Disconnect/lock/sleep
        // never enters this path. Exit remains bounded and never opens a dialog.
        if let Err(error) = crate::pending_close::persist(&app, &state).await {
            eprintln!("保存退出关闭记录失败：{error}");
        }
        let result = tokio::time::timeout(Duration::from_secs(8), async {
            // Tab cleanup is advisory; native ownership is the cleanup authority.
            let _ = tokio::time::timeout(Duration::from_secs(1), close_tabs(&app, &state)).await;
            stop_owned(&state, &|host| {
                let app = app.clone();
                let state = &*state;
                async move { crate::reconnect_for_cleanup(&app, state, &host).await }
            })
            .await?;
            finish_cleanup(&state).await;
            Ok::<_, String>(())
        })
        .await;
        if !matches!(result, Ok(Ok(()))) {
            // Keep only unconfirmed instances; no 'stopped' state is invented.
            if let Err(error) = crate::pending_close::persist(&app, &state).await {
                eprintln!("保留待关闭记录失败：{error}");
            }
        } else {
            let _ = crate::pending_close::persist(&app, &state).await;
        }
        state.close_ack.lock().await.take();
        crate::source_preview::close().await;
        exit(&app);
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    async fn unavailable(_: String) -> Result<latte_work_client::Client, String> {
        Err("未连接，无法确认会话或终端已关闭".into())
    }
    #[cfg(unix)]
    fn bridge(break_ack: bool) -> (std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "latte-quit-reconnect-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).unwrap();
        let binary = directory.join("bridge");
        let source = r#"#!/usr/bin/env python3
import sys,json,pathlib
base=pathlib.Path(sys.argv[sys.argv.index('--state-dir')+1])
for line in sys.stdin:
    request=json.loads(line)
    with (base/'requests').open('a') as f:f.write(request['method']+'\n')
    if request['method']=='hello':
        response={'kind':'hello','version':1,'server_id':'fixture','agents':[]}
    elif request['method']=='projects':
        sys.exit(0)
    elif request['method'] in ['close_agent_session','close_terminal']:
        if BREAK_ACK and not (base/'ack-lost').exists():
            (base/'ack-lost').touch()
            sys.exit(0)
        response={'kind':'ok'}
    else:
        response={'kind':'error','code':'unexpected','message':'no task replay allowed'}
    print(json.dumps(response),flush=True)
"#
        .replace("BREAK_ACK", if break_ack { "True" } else { "False" });
        std::fs::write(&binary, source).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        (directory, binary)
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn lost_close_ack_reconnects_once_without_replaying_tasks_and_settles_terminals() {
        let (directory, binary) = bridge(true);
        let state = Connections::default();
        let client = latte_work_client::Client::local(&binary, Some(&directory))
            .await
            .unwrap();
        state
            .clients
            .lock()
            .await
            .insert("remote".into(), Arc::new(tokio::sync::Mutex::new(client)));
        state
            .turns
            .lock()
            .await
            .insert(("remote".into(), "session".into()), "request".into());
        state
            .terminals
            .lock()
            .await
            .insert(("remote".into(), "terminal".into()));
        let reconnects = std::sync::atomic::AtomicUsize::new(0);
        stop_owned(&state, &|host| {
            assert_eq!(host, "remote");
            reconnects.fetch_add(1, Ordering::SeqCst);
            let (binary, directory) = (binary.clone(), directory.clone());
            async move {
                latte_work_client::Client::local(&binary, Some(&directory))
                    .await
                    .map_err(|e| e.to_string())
            }
        })
        .await
        .unwrap();
        finish_cleanup(&state).await;
        assert_eq!(reconnects.load(Ordering::SeqCst), 1);
        assert!(state.turns.lock().await.is_empty());
        assert!(state.terminals.lock().await.is_empty());
        assert_eq!(
            std::fs::read_to_string(directory.join("requests")).unwrap(),
            "hello\nclose_agent_session\nhello\nclose_agent_session\nclose_terminal\n"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn missing_or_already_broken_bridge_can_be_replaced_during_quit() {
        for cached in [false, true] {
            let (directory, binary) = bridge(false);
            let state = Connections::default();
            if cached {
                let mut client = latte_work_client::Client::local(&binary, Some(&directory))
                    .await
                    .unwrap();
                assert!(client.request(Request::Projects).await.is_err());
                assert!(client.is_broken());
                state
                    .clients
                    .lock()
                    .await
                    .insert("remote".into(), Arc::new(tokio::sync::Mutex::new(client)));
            }
            state
                .turns
                .lock()
                .await
                .insert(("remote".into(), "session".into()), "request".into());
            stop_owned(&state, &|_| {
                let (binary, directory) = (binary.clone(), directory.clone());
                async move {
                    latte_work_client::Client::local(&binary, Some(&directory))
                        .await
                        .map_err(|e| e.to_string())
                }
            })
            .await
            .unwrap();
            finish_cleanup(&state).await;
            assert!(state.turns.lock().await.is_empty());
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn unavailable_host_preserves_unknown_ownership_but_other_hosts_still_close() {
        let (directory, binary) = bridge(false);
        let state = Connections::default();
        for host in ["offline", "reachable"] {
            state
                .turns
                .lock()
                .await
                .insert((host.into(), "session".into()), "request".into());
        }
        let error = stop_owned(&state, &|host| {
            let (binary, directory) = (binary.clone(), directory.clone());
            async move {
                if host == "offline" {
                    return Err("无法连接".into());
                }
                latte_work_client::Client::local(&binary, Some(&directory))
                    .await
                    .map_err(|e| e.to_string())
            }
        })
        .await
        .unwrap_err();
        assert!(error.contains("offline"));
        let remaining = state.turns.lock().await;
        assert_eq!(remaining.len(), 1);
        assert!(remaining.contains_key(&("offline".into(), "session".into())));
        drop(remaining);
        finish_cleanup(&state).await;
        assert!(!state.exit_ready.load(Ordering::SeqCst));
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[tokio::test]
    async fn cleanup_rejects_non_close_requests_without_contacting_a_host() {
        let state = Connections::default();
        assert!(
            close_resource(&state, "remote", Request::Projects, &|_| async {
                panic!("cleanup must never contact a host for a non-close request")
            })
            .await
            .unwrap_err()
            .contains("只允许关闭")
        );
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn quit_closes_completed_agent_sessions_without_polling_away_ownership() {
        use std::os::unix::fs::PermissionsExt;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("latte-quit-{}-{stamp}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let binary = directory.join("bridge");
        std::fs::write(&binary, r#"#!/usr/bin/env python3
import sys,json,pathlib
log=pathlib.Path(sys.argv[sys.argv.index('--state-dir')+1])/'requests'
for line in sys.stdin:
    request=json.loads(line)
    with log.open('a') as f:f.write(request['method']+'\n')
    if request['method']=='hello':
        response={'kind':'hello','version':1,'server_id':'fixture','agents':[]}
    elif request['method']=='close_agent_session' and request['session_id']=='completed-session':
        response={'kind':'ok'}
    else:
        response={'kind':'error','code':'unexpected','message':'completed sessions still require explicit close'}
    print(json.dumps(response),flush=True)
"#).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let client = latte_work_client::Client::local(&binary, Some(&directory))
            .await
            .unwrap();
        let state = Connections::default();
        state.clients.lock().await.insert(
            "remote".into(),
            std::sync::Arc::new(tokio::sync::Mutex::new(client)),
        );
        state.turns.lock().await.insert(
            ("remote".into(), "completed-session".into()),
            "accepted-request".into(),
        );
        stop_owned(&state, &unavailable).await.unwrap();
        finish_cleanup(&state).await;
        assert!(state.turns.lock().await.is_empty());
        assert!(state.clients.lock().await.is_empty());
        assert_eq!(
            std::fs::read_to_string(directory.join("requests")).unwrap(),
            "hello\nclose_agent_session\n"
        );
        std::fs::remove_file(binary).unwrap();
        std::fs::remove_file(directory.join("requests")).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
    #[tokio::test]
    async fn only_current_tab_cleanup_ack_can_finish_exit() {
        let state = Connections::default();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        *state.close_ack.lock().await = Some(("current".into(), tx));
        finish_close(&state, "old", None).await;
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        finish_close(&state, "current", Some("terminal offline".into())).await;
        assert_eq!(rx.await.unwrap(), Err("terminal offline".into()));
    }
    #[tokio::test]
    async fn disconnected_owned_terminal_keeps_unknown_ownership() {
        let state = Connections::default();
        state
            .terminals
            .lock()
            .await
            .insert(("remote".into(), "terminal".into()));
        assert!(
            stop_owned(&state, &unavailable)
                .await
                .unwrap_err()
                .contains("终端")
        );
        assert_eq!(state.terminals.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn disconnected_owned_turn_is_not_marked_stopped() {
        let state = Connections::default();
        state
            .turns
            .lock()
            .await
            .insert(("remote".into(), "session".into()), "request".into());
        assert!(
            stop_owned(&state, &unavailable)
                .await
                .unwrap_err()
                .contains("未连接")
        );
        assert_eq!(state.turns.lock().await.len(), 1);
        assert!(!state.exit_ready.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn quit_waits_for_inflight_admission_before_completing() {
        let state = Connections::default();
        let pending_send = state.admission.read().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(20), stop_owned(&state, &unavailable))
                .await
                .is_err()
        );
        drop(pending_send);
        assert!(stop_owned(&state, &unavailable).await.is_ok());
    }
}
