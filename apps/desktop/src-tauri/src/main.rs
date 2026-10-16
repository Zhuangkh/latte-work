#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod askpass;
mod browser;
mod browser_downloads;
mod browser_native;
mod lifecycle;
mod paste;
mod pending_close;
mod project_folder;
mod providers;
mod request_channels;
mod source_preview;
use latte_work_client::{Client, SshAuthentication};
use latte_work_protocol::{Request, Response};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::{Mutex, RwLock};
#[derive(Default)]
struct Connections {
    clients: Mutex<HashMap<String, Arc<Mutex<Client>>>>,
    host_targets: Mutex<HashMap<String, Host>>,
    request_channels: Mutex<HashMap<String, Arc<request_channels::Channels<Client>>>>,
    cleanup_admission: Mutex<()>,
    owner: std::sync::OnceLock<String>,
    server_ids: Mutex<HashMap<String, String>>,
    owned_instances: Mutex<HashMap<(String, latte_work_protocol::lifecycle::Resource), String>>,
    passwords: Mutex<HashMap<String, CachedPassword>>,
    turns: Mutex<HashMap<(String, String), String>>,
    terminals: Mutex<HashSet<(String, String)>>,
    admission: RwLock<()>,
    agent_admission: Mutex<()>,
    quitting: AtomicBool,
    exit_ready: AtomicBool,
    close_ack: Mutex<Option<lifecycle::CloseAck>>,
}
impl Connections {
    fn owner_id(&self) -> &str {
        self.owner.get_or_init(|| uuid::Uuid::new_v4().to_string())
    }
}
struct CachedPassword {
    ssh: String,
    port: Option<u16>,
    value: String,
}
impl CachedPassword {
    fn for_target(&self, ssh: &str, port: Option<u16>) -> Option<&str> {
        (self.ssh == ssh && self.port == port).then_some(self.value.as_str())
    }
}
#[cfg(unix)]
fn askpass_helper(path: PathBuf) -> Result<(), String> {
    use std::io::Write;
    let stream = std::os::unix::net::UnixStream::connect(path)
        .map_err(|_| "askpass socket unavailable".to_string())?;
    let mut password = Vec::new();
    stream
        .take(4097)
        .read_to_end(&mut password)
        .map_err(|_| "askpass read failed".to_string())?;
    if password.len() > 4096 {
        return Err("askpass response too long".into());
    }
    std::io::stdout()
        .write_all(&password)
        .map_err(|_| "askpass write failed".to_string())
}

#[cfg(windows)]
fn askpass_helper(path: PathBuf) -> Result<(), String> {
    use std::io::Write;
    let name = path
        .to_str()
        .ok_or_else(|| "askpass pipe path is not UTF-8".to_string())?;
    let mut stream = latte_work_platform::pipe::connect_file_once(name)
        .map_err(|_| "askpass pipe unavailable".to_string())?;
    let mut password = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) if byte[0] == 0 => break,
            Ok(_) => {
                password.push(byte[0]);
                if password.len() > 4096 {
                    return Err("askpass response too long".into());
                }
            }
            Err(_) => return Err("askpass read failed".into()),
        }
    }
    std::io::stdout()
        .write_all(&password)
        .map_err(|_| "askpass write failed".to_string())
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SshAuthMode {
    #[default]
    None,
    IdentityFile,
    Password,
}
#[derive(Clone, Serialize, Deserialize)]
struct Host {
    id: String,
    name: String,
    ssh: Option<String>,
    server_path: Option<String>,
    #[serde(default)]
    platform: latte_work_client::SshPlatform,
    #[serde(default)]
    auth: SshAuthMode,
    #[serde(default)]
    identity_file: Option<String>,
    #[serde(default)]
    port: Option<u16>,
}
#[tauri::command]
async fn connect_host(
    app: tauri::AppHandle,
    state: State<'_, Connections>,
    host: Host,
    password: Option<String>,
) -> Result<Response, String> {
    let _admission = state.admission.read().await;
    if state.quitting.load(Ordering::SeqCst) {
        return Err("正在停止任务并退出，请稍候".into());
    }
    let client = open_host_client(&app, &state, &host, password.as_deref()).await?;
    let mut client = client;
    state
        .host_targets
        .lock()
        .await
        .insert(host.id.clone(), host.clone());
    let adopted = pending_close::drain(&app, &state, &host, &mut client).await?;
    state
        .server_ids
        .lock()
        .await
        .insert(host.id.clone(), client.server_id().to_owned());
    let response = client
        .request(Request::Hello {
            version: latte_work_protocol::VERSION,
        })
        .await
        .map_err(|e| e.to_string())?;
    state
        .host_targets
        .lock()
        .await
        .insert(host.id.clone(), host.clone());
    let client = Arc::new(Mutex::new(client));
    state.request_channels.lock().await.remove(&host.id);
    state
        .clients
        .lock()
        .await
        .insert(host.id.clone(), client.clone());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<Connections>();
        let sessions: Vec<_> = state
            .turns
            .lock()
            .await
            .keys()
            .filter(|(id, _)| id == &host.id)
            .cloned()
            .collect();
        let primary = client.clone();
        let (control, _) = tokio::join!(
            request_client(
                &app,
                &state,
                &host.id,
                client.clone(),
                request_channels::Lane::Control
            ),
            request_client(
                &app,
                &state,
                &host.id,
                client,
                request_channels::Lane::Metadata
            )
        );
        if sessions.is_empty() {
            return;
        }
        let client = match control {
            Ok(client) => client,
            Err(_) => return,
        };
        for key in sessions {
            if adopted.contains(&latte_work_protocol::lifecycle::Resource::AgentSession(
                key.1.clone(),
            )) {
                continue;
            }
            let _admission = state.admission.read().await;
            if state.quitting.load(Ordering::SeqCst) {
                break;
            }
            let _agents = state.agent_admission.lock().await;
            if !state
                .clients
                .lock()
                .await
                .get(&host.id)
                .is_some_and(|current| Arc::ptr_eq(current, &primary))
            {
                break;
            }
            if !state.turns.lock().await.contains_key(&key) {
                continue;
            }
            if claim_resource(
                &state,
                &host.id,
                &client,
                latte_work_protocol::lifecycle::Resource::AgentSession(key.1.clone()),
            )
            .await
            .is_err()
            {
                continue;
            }
            let target = host.id.clone();
            // Open is idempotent and contains no prompt. A surviving daemon keeps
            // the exact process; a restarted daemon resumes the persisted native ID.
            let _ = latte_work_client::request_with_app_provider(
                &client,
                Request::OpenAgentSession {
                    session_id: key.1,
                    provider: None,
                },
                |agent| async {
                    providers::with_store(app.clone(), move |store| {
                        store.snapshot_for_host(&target, &agent)
                    })
                    .await
                    .map_err(anyhow::Error::msg)
                },
            )
            .await;
        }
    });
    Ok(response)
}
// Establish transport only. Exit cleanup must never restore an Agent or replay input.
async fn open_host_client(
    app: &tauri::AppHandle,
    state: &Connections,
    host: &Host,
    password: Option<&str>,
) -> Result<Client, String> {
    let client = if let Some(ssh) = &host.ssh {
        let secret = if matches!(host.auth, SshAuthMode::Password) {
            Some(match password {
                Some(value) if !value.is_empty() => value.to_owned(),
                _ => state
                    .passwords
                    .lock()
                    .await
                    .get(&host.id)
                    .and_then(|saved| saved.for_target(ssh, host.port))
                    .map(str::to_owned)
                    .ok_or("SSH_PASSWORD_REQUIRED")?,
            })
        } else {
            None
        };
        let identity = host.identity_file.as_deref().map(PathBuf::from);
        let askpass = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut challenge = None;
        let auth = match host.auth {
            SshAuthMode::None => SshAuthentication::OpenSsh,
            SshAuthMode::IdentityFile => {
                SshAuthentication::IdentityFile(identity.as_deref().ok_or("请选择身份文件")?)
            }
            SshAuthMode::Password => {
                challenge = Some(askpass::Askpass::start(
                    secret.clone().ok_or("请输入 SSH 密码")?,
                )?);
                SshAuthentication::Password {
                    askpass: &askpass,
                    socket: challenge.as_ref().expect("askpass challenge").target(),
                }
            }
        };
        let result = Client::ssh_with_platform(
            ssh,
            host.server_path.as_deref().unwrap_or(""),
            host.port,
            auth,
            host.platform,
        )
        .await;
        drop(challenge);
        if result.is_ok() {
            let mut passwords = state.passwords.lock().await;
            if let Some(secret) = secret {
                passwords.insert(
                    host.id.clone(),
                    CachedPassword {
                        ssh: ssh.clone(),
                        port: host.port,
                        value: secret,
                    },
                );
            } else {
                passwords.remove(&host.id);
            }
        }
        result.map_err(|e| format!("{e:#}"))
    } else {
        connect_local(app).await
    }?;
    Ok(client)
}
async fn reconnect_for_cleanup(
    app: &tauri::AppHandle,
    state: &Connections,
    host_id: &str,
) -> Result<Client, String> {
    tokio::time::timeout(std::time::Duration::from_secs(8), async {
        if host_id == "local" {
            return Client::local_cleanup(&local_binary(app)?, None)
                .await
                .map_err(|e| format!("{e:#}"));
        }
        let host = state
            .host_targets
            .lock()
            .await
            .get(host_id)
            .cloned()
            .ok_or_else(|| format!("{host_id} 的连接信息不可用"))?;
        open_host_client(app, state, &host, None).await
    })
    .await
    .map_err(|_| format!("{host_id} 重连超时"))?
}
fn local_binary(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if cfg!(debug_assertions)
        && let Ok(path) = std::env::var("LATTE_WORK_SERVER")
    {
        return Ok(PathBuf::from(path));
    }
    let name = format!("latte-work-server{}", std::env::consts::EXE_SUFFIX);
    let mut binary = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name(&name);
    if !binary.is_file() {
        let resource = app.path().resource_dir().map_err(|e| e.to_string())?;
        binary = resource.join(&name);
    }
    if !binary.is_file() && cfg!(debug_assertions) {
        // `cargo run` does not run Tauri's sidecar copy; fall back to the
        // prepared external-binaries directory so dev builds stay usable.
        let bundled = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!(
                "latte-work-server-{}{}",
                env!("TAURI_ENV_TARGET_TRIPLE"),
                std::env::consts::EXE_SUFFIX
            ));
        if bundled.is_file() {
            binary = bundled;
        }
    }
    Ok(binary)
}
async fn connect_local(app: &tauri::AppHandle) -> Result<Client, String> {
    Client::local(&local_binary(app)?, None)
        .await
        .map_err(|e| format!("{e:#}"))
}
async fn local_client(
    app: &tauri::AppHandle,
    state: &Connections,
) -> Result<Arc<Mutex<Client>>, String> {
    if let Some(client) = state.clients.lock().await.get("local").cloned() {
        return Ok(client);
    }
    let client = Arc::new(Mutex::new(connect_local(app).await?));
    Ok(state
        .clients
        .lock()
        .await
        .entry("local".into())
        .or_insert(client)
        .clone())
}
async fn claim_resource(
    state: &Connections,
    host: &str,
    client: &Arc<Mutex<Client>>,
    resource: latte_work_protocol::lifecycle::Resource,
) -> Result<(), String> {
    let server_id = state
        .server_ids
        .lock()
        .await
        .get(host)
        .cloned()
        .ok_or("缺少 Server 实例信息")?;
    state
        .owned_instances
        .lock()
        .await
        .insert((host.to_owned(), resource.clone()), server_id);
    let mut client = client.lock().await;
    client
        .claim_resources(state.owner_id().to_owned(), vec![resource])
        .await
        .map_err(|e| format!("{e:#}"))
}
// Extra lanes connect to the existing daemon only. They do not initialize projects,
// restore sessions or upgrade a local daemon, and survive ordinary navigation.
async fn request_client(
    app: &tauri::AppHandle,
    state: &Connections,
    host_id: &str,
    primary: Arc<Mutex<Client>>,
    lane: request_channels::Lane,
) -> Result<Arc<Mutex<Client>>, String> {
    if !state
        .clients
        .lock()
        .await
        .get(host_id)
        .is_some_and(|current| Arc::ptr_eq(current, &primary))
    {
        return Err("HOST_CONNECTION_LOST:连接已更新，请重试".into());
    }
    let channels = {
        let mut hosts = state.request_channels.lock().await;
        if !hosts
            .get(host_id)
            .is_some_and(|c| Arc::ptr_eq(&c.primary, &primary))
        {
            hosts.insert(
                host_id.to_owned(),
                Arc::new(request_channels::Channels::new(primary.clone())),
            );
        }
        hosts.get(host_id).unwrap().clone()
    };
    let expected = state.server_ids.lock().await.get(host_id).cloned();
    let client = channels
        .client(lane, || async {
            let client = reconnect_for_cleanup(app, state, host_id).await?;
            if expected.as_deref() != Some(client.server_id()) {
                return Err("HOST_CONNECTION_LOST:后台实例已变化，请重新连接".to_owned());
            }
            Ok(client)
        })
        .await?;
    if !state
        .clients
        .lock()
        .await
        .get(host_id)
        .is_some_and(|current| Arc::ptr_eq(current, &primary))
    {
        return Err("HOST_CONNECTION_LOST:连接已更新，请重试".into());
    }
    Ok(client)
}
#[tauri::command]
async fn host_request(
    app: tauri::AppHandle,
    state: State<'_, Connections>,
    host_id: String,
    request: Request,
) -> Result<Response, String> {
    let _admission = state.admission.read().await;
    if state.quitting.load(Ordering::SeqCst) && !matches!(request, Request::CloseTerminal { .. }) {
        return Err("正在停止任务并退出，请稍候".into());
    }
    if state.quitting.load(Ordering::SeqCst)
        && let Request::CloseTerminal { terminal_id } = &request
    {
        let terminal_id = terminal_id.clone();
        lifecycle::close_resource(&state, &host_id, request, &|host| {
            let app = app.clone();
            let state = &*state;
            async move { reconnect_for_cleanup(&app, state, &host).await }
        })
        .await?;
        state
            .terminals
            .lock()
            .await
            .remove(&(host_id.clone(), terminal_id.clone()));
        state.owned_instances.lock().await.remove(&(
            host_id,
            latte_work_protocol::lifecycle::Resource::Terminal(terminal_id),
        ));
        return Ok(Response::Ok);
    }
    if matches!(
        request,
        Request::Providers
            | Request::ProvidersForHost { .. }
            | Request::SaveProvider { .. }
            | Request::DeleteProvider { .. }
            | Request::BindAgentProvider { .. }
            | Request::BindHostAgentProvider { .. }
            | Request::ForgetHostProviders { .. }
            | Request::ExportHostAgentProvider { .. }
            | Request::SyncAgentProvider { .. }
            | Request::ModelsForProvider { .. }
    ) {
        return Err("Provider 配置由 App 管理，请使用 App 设置入口".into());
    }
    let client = state
        .clients
        .lock()
        .await
        .get(&host_id)
        .cloned()
        .ok_or("HOST_CONNECTION_LOST:Host 未连接")?;
    let primary = client.clone();
    let client = request_client(
        &app,
        &state,
        &host_id,
        client,
        request_channels::lane(&request),
    )
    .await?;
    let _agents = if matches!(
        request,
        Request::Send { .. }
            | Request::SendQueued { .. }
            | Request::DispatchQueuedMessage { .. }
            | Request::ResumeMessageQueue { .. }
            | Request::OpenAgentSession { .. }
            | Request::CloseAgentSession { .. }
    ) {
        Some(state.agent_admission.lock().await)
    } else {
        None
    };
    if state.quitting.load(Ordering::SeqCst) {
        return Err("正在退出，任务未发送".into());
    }
    if let Request::OpenAgentSession { session_id, .. }
    | Request::DispatchQueuedMessage { session_id, .. }
    | Request::ResumeMessageQueue { session_id, .. } = &request
    {
        state
            .turns
            .lock()
            .await
            .entry((host_id.clone(), session_id.clone()))
            .or_insert_with(|| "open".into());
    }
    let closed_agent = if let Request::CloseAgentSession { session_id, .. } = &request {
        Some((host_id.clone(), session_id.clone()))
    } else {
        None
    };
    if let Request::Send {
        session_id,
        request_id,
        ..
    }
    | Request::SendQueued {
        session_id,
        request_id,
        ..
    } = &request
    {
        // Track before transport: an ambiguous send response may still mean it ran.
        state
            .turns
            .lock()
            .await
            .insert((host_id.clone(), session_id.clone()), request_id.clone());
    }
    // Keep ownership even if creation's transport response is lost.
    let created_terminal = if let Request::CreateTerminal { terminal_id, .. } = &request {
        let key = (host_id.clone(), terminal_id.clone());
        let fresh = state.terminals.lock().await.insert(key.clone());
        Some((key, fresh))
    } else {
        None
    };
    let closed_terminal = if let Request::CloseTerminal { terminal_id } = &request {
        Some((host_id.clone(), terminal_id.clone()))
    } else {
        None
    };
    let resource = match &request {
        Request::OpenAgentSession { session_id, .. }
        | Request::Send { session_id, .. }
        | Request::SendQueued { session_id, .. }
        | Request::DispatchQueuedMessage { session_id, .. }
        | Request::ResumeMessageQueue { session_id, .. } => Some(
            latte_work_protocol::lifecycle::Resource::AgentSession(session_id.clone()),
        ),
        Request::CreateTerminal { terminal_id, .. } => Some(
            latte_work_protocol::lifecycle::Resource::Terminal(terminal_id.clone()),
        ),
        _ => None,
    };
    if let Some(resource) = resource {
        claim_resource(&state, &host_id, &client, resource).await?;
        if state.quitting.load(Ordering::SeqCst) {
            return Err("正在退出，任务未发送".into());
        }
    }
    let response = if matches!(
        request,
        Request::Models { .. }
            | Request::Send { .. }
            | Request::SendQueued { .. }
            | Request::DispatchQueuedMessage { .. }
            | Request::ResumeMessageQueue { .. }
            | Request::OpenAgentSession { .. }
            | Request::SubagentHistory { .. }
    ) {
        let provider_host = host_id.clone();
        latte_work_client::request_with_app_provider(&client, request, |agent| async move {
            providers::with_store(app, move |store| {
                store.snapshot_for_host(&provider_host, &agent)
            })
            .await
            .map_err(anyhow::Error::msg)
        })
        .await
    } else {
        client.lock().await.request(request).await
    };
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            let detail = format!("{error:#}");
            return Err(if client.lock().await.is_broken() {
                format!("HOST_CONNECTION_LOST:{detail}")
            } else {
                detail
            });
        }
    };
    if !state
        .clients
        .lock()
        .await
        .get(&host_id)
        .is_some_and(|current| Arc::ptr_eq(current, &primary))
    {
        return Err("HOST_CONNECTION_LOST:连接已更新；请求结果待确认，任务不会自动重发".into());
    }
    if matches!(response, Response::Error { .. })
        && let Some((key, true)) = created_terminal
    {
        state.terminals.lock().await.remove(&key);
        state.owned_instances.lock().await.remove(&(
            key.0,
            latte_work_protocol::lifecycle::Resource::Terminal(key.1),
        ));
    }
    if matches!(response, Response::Ok)
        && let Some(key) = closed_terminal
    {
        state.terminals.lock().await.remove(&key);
        state.owned_instances.lock().await.remove(&(
            key.0,
            latte_work_protocol::lifecycle::Resource::Terminal(key.1),
        ));
    }
    if matches!(response, Response::Ok)
        && let Some(key) = closed_agent
    {
        state.turns.lock().await.remove(&key);
        state.owned_instances.lock().await.remove(&(
            key.0,
            latte_work_protocol::lifecycle::Resource::AgentSession(key.1),
        ));
    }
    ui_response(response)
}
fn ui_response(response: Response) -> Result<Response, String> {
    if matches!(response, Response::ProviderSnapshot { .. }) {
        return Err("Provider 凭据不能返回界面".into());
    }
    Ok(response)
}
#[tauri::command]
async fn disconnect_host(state: State<'_, Connections>, host_id: String) -> Result<(), String> {
    state.clients.lock().await.remove(&host_id);
    state.request_channels.lock().await.remove(&host_id);
    state.passwords.lock().await.remove(&host_id);
    Ok(())
}
fn host_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("hosts.json"))
}
#[tauri::command]
fn load_hosts(app: tauri::AppHandle) -> Result<Vec<Host>, String> {
    let path = host_file(&app)?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice(&data).map_err(|e| e.to_string())
}
#[tauri::command]
async fn save_hosts(app: tauri::AppHandle, hosts: Vec<Host>) -> Result<(), String> {
    let removed: Vec<_> = load_hosts(app.clone())?
        .into_iter()
        .filter(|old| !hosts.iter().any(|host| host.id == old.id))
        .collect();
    if !removed.is_empty() {
        providers::with_store(app.clone(), move |store| {
            for host in removed {
                store.forget_host(&host.id)?;
            }
            Ok(())
        })
        .await?;
    }
    let path = host_file(&app)?;
    std::fs::create_dir_all(path.parent().ok_or("无效配置路径")?).map_err(|e| e.to_string())?;
    let temp = path.with_extension("tmp");
    std::fs::write(
        &temp,
        serde_json::to_vec_pretty(&hosts).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(temp, path).map_err(|e| e.to_string())
}
#[tauri::command]
async fn choose_reference_path(
    app: tauri::AppHandle,
    directory: bool,
) -> Result<Option<String>, String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let dialog = app.dialog().file().set_title(if directory {
        "引用文件夹"
    } else {
        "引用文件"
    });
    let callback = move |file: Option<tauri_plugin_dialog::FilePath>| {
        let result = file
            .map(|p| {
                p.into_path()
                    .map(|p| p.to_string_lossy().into_owned())
                    .map_err(|e| e.to_string())
            })
            .transpose();
        let _ = send.send(result);
    };
    if directory {
        dialog.pick_folder(callback);
    } else {
        dialog.pick_file(callback);
    }
    receive.await.map_err(|e| e.to_string())?
}
#[tauri::command]
async fn choose_project_folder(
    app: tauri::AppHandle,
) -> Result<Option<project_folder::ProjectFolder>, String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择项目文件夹")
        .pick_folder(move |folder| {
            let result = folder
                .map(|p| {
                    p.into_path()
                        .map_err(|e| e.to_string())
                        .and_then(project_folder::ProjectFolder::try_from)
                })
                .transpose();
            let _ = send.send(result);
        });
    receive.await.map_err(|e| e.to_string())?
}
#[tauri::command]
async fn choose_identity_file(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择 SSH 身份文件")
        .pick_file(move |file| {
            let result = file
                .map(|p| {
                    p.into_path()
                        .map(|p| p.to_string_lossy().into_owned())
                        .map_err(|e| e.to_string())
                })
                .transpose();
            let _ = send.send(result);
        });
    receive.await.map_err(|e| e.to_string())?
}
#[tauri::command]
async fn reveal_project(
    app: tauri::AppHandle,
    state: State<'_, Connections>,
    host_id: String,
    project_id: String,
) -> Result<(), String> {
    if host_id != "local" {
        return Err("仅本机项目可在文件管理器中显示".into());
    }
    let client = state
        .clients
        .lock()
        .await
        .get(&host_id)
        .cloned()
        .ok_or("Host 未连接")?;
    let response = client
        .lock()
        .await
        .request(Request::Projects)
        .await
        .map_err(|e| e.to_string())?;
    let Response::Projects { projects } = response else {
        return Err("项目列表不可用".into());
    };
    let project = projects
        .into_iter()
        .find(|p| p.id == project_id)
        .ok_or("项目不存在")?;
    let path = PathBuf::from(project.path)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if !path.is_dir() {
        return Err("项目目录不存在".into());
    }
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| format!("无法在文件管理器中显示项目：{e}"))
}
#[tauri::command]
fn desktop_platform() -> &'static str {
    std::env::consts::OS
}
fn main() {
    if std::env::var("LATTE_WORK_SSH_ASKPASS").as_deref() == Ok("1") {
        let result = std::env::var_os("LATTE_WORK_SSH_ASKPASS_SOCKET")
            .ok_or_else(|| "missing askpass socket".to_string())
            .and_then(|path| askpass_helper(PathBuf::from(path)));
        if let Err(error) = &result {
            eprintln!("latte-work-desktop askpass: {error}");
        }
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(Connections::default())
        .manage(browser::Tools::default())
        .manage(browser_downloads::Downloads::default())
        .menu(|app| {
            let menu = tauri::menu::Menu::default(app)?;
            #[cfg(target_os = "macos")]
            if let Some(tauri::menu::MenuItemKind::Submenu(application)) = menu.items()?.first() {
                // Tauri's default macOS app submenu ends in native terminate:.
                // Replace it with an event-backed item also invoked by macos_quit.m.
                let position = application.items()?.len() - 1;
                application.remove_at(position)?;
                let quit = tauri::menu::MenuItem::with_id(
                    app,
                    "latte-quit",
                    "退出 Latte Work",
                    true,
                    Some("CmdOrCtrl+Q"),
                )?;
                application.insert(&quit, position)?;
            }
            let tools = tauri::menu::Submenu::with_id(app, "browser-menu", "浏览器", true)?;
            for (id, title, shortcut) in [
                ("find", "在页面中查找", "CmdOrCtrl+F"),
                ("print", "打印…", "CmdOrCtrl+P"),
                ("zoom-in", "放大", "CmdOrCtrl+Plus"),
                ("zoom-out", "缩小", "CmdOrCtrl+-"),
                ("zoom-reset", "实际大小", "CmdOrCtrl+0"),
                ("history", "历史记录", "CmdOrCtrl+Y"),
            ] {
                tools.append(&tauri::menu::MenuItem::with_id(
                    app,
                    format!("browser-{id}"),
                    title,
                    true,
                    Some(shortcut),
                )?)?;
            }
            menu.append(&tools)?;
            Ok(menu)
        })
        .on_menu_event(|app, event| {
            if event.id().as_ref() == "latte-quit" {
                app.exit(0);
            } else if let Some(action) = event.id().as_ref().strip_prefix("browser-") {
                let _ = app.emit_to("main", "browser-shortcut", action);
            }
        })
        .on_window_event(|window, event| {
            #[cfg(target_os = "macos")]
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Closing the window must preserve its WebView and in-memory drafts.
                // Explicit application Quit is deliberately not intercepted.
                if window.hide().is_ok() {
                    api.prevent_close();
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (window, event);
        })
        .invoke_handler(tauri::generate_handler![
            lifecycle::app_close_ready,
            browser::browser_create,
            browser::browser_sync,
            browser::browser_site_info,
            browser::browser_site_data,
            browser::browser_action,
            browser::browser_close,
            browser::browser_find,
            browser::browser_capture,
            browser::browser_preview,
            browser::browser_clear_data,
            browser_downloads::browser_downloads,
            browser_downloads::browser_clear_downloads,
            browser_downloads::browser_reveal_download,
            browser::open_external_url,
            connect_host,
            disconnect_host,
            host_request,
            providers::provider_request,
            providers::fetch_provider_models,
            providers::bind_agent_provider,
            providers::agent_providers,
            load_hosts,
            save_hosts,
            choose_project_folder,
            choose_reference_path,
            source_preview::system_preview,
            source_preview::preview_pdf,
            paste::clipboard_file_paths,
            paste::paste_stage,
            paste::finish_paste_upload,
            paste::import_clipboard_file,
            choose_identity_file,
            reveal_project,
            desktop_platform
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Latte Work desktop")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = &event {
                let state = app.state::<Connections>();
                if !state.exit_ready.load(Ordering::SeqCst) {
                    api.prevent_exit();
                    if !state.quitting.swap(true, Ordering::SeqCst) {
                        lifecycle::quit(app.clone());
                    }
                }
            }
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event
                && let Some(window) = app.get_window("main")
            {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_provider_snapshots_cannot_reach_the_webview() {
        let response: Response = serde_json::from_value(serde_json::json!({
            "kind": "provider_snapshot",
            "snapshot": {
                "provider": {
                    "id": "provider", "name": "Fixture", "protocol": "anthropic_messages",
                    "base_url": "https://example.test", "model": "model", "auth": "api_key",
                    "has_credential": true
                },
                "credential": "native-only-secret"
            }
        }))
        .unwrap();
        assert!(!format!("{response:?}").contains("native-only-secret"));
        let error = ui_response(response).unwrap_err();
        assert!(!error.contains("native-only-secret"));
        assert!(
            ui_response(Response::Providers {
                providers: vec![],
                bindings: vec![]
            })
            .is_ok()
        );
    }

    #[test]
    fn old_host_records_default_to_openssh_and_password_is_not_serialized() {
        let host: Host = serde_json::from_str(
            r#"{"id":"remote","name":"devbox","ssh":"devbox","server_path":"/server","password":"secret"}"#,
        )
        .unwrap();
        assert!(matches!(host.auth, SshAuthMode::None));
        assert_eq!(host.platform, latte_work_client::SshPlatform::Unix);
        let json = serde_json::to_string(&Host {
            auth: SshAuthMode::Password,
            platform: latte_work_client::SshPlatform::Windows,
            ..host
        })
        .unwrap();
        assert!(json.contains("\"auth\":\"password\""));
        let restored: Host = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.platform, latte_work_client::SshPlatform::Windows);
        assert!(!json.contains("password\":\"secret"));
    }

    #[test]
    fn cached_password_is_only_reused_for_the_same_ssh_target() {
        let saved = CachedPassword {
            ssh: "user@old-host".into(),
            port: Some(2222),
            value: "test-secret".into(),
        };
        assert_eq!(
            saved.for_target("user@old-host", Some(2222)),
            Some("test-secret")
        );
        assert_eq!(saved.for_target("user@new-host", Some(2222)), None);
        assert_eq!(saved.for_target("user@old-host", Some(22)), None);
    }
}
