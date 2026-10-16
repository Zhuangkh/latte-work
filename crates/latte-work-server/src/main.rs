// A single host daemon; `connect` is a disposable local/SSH byte bridge.
mod agent_environment;
mod agents;
mod attachments;
mod files;
mod git_review;
mod host_paths;
mod managed_recovery;
mod message_queue;
mod providers;
mod questions;
mod runtime;
mod sources;
mod store;
mod task_changes;
mod terminal;
mod upgrade;
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use latte_work_platform::{
    fs::prepare_state_dir,
    ipc::{self, Endpoint, Stream},
    process::{self, Shutdown},
};
use latte_work_protocol::{AgentInfo, MAX_FRAME, Request, Response, VERSION};
use runtime::{Control, Database, Runs, db};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{io::AsyncWriteExt, sync::oneshot};
use tokio_util::codec::{FramedRead, LinesCodec};

#[derive(Clone)]
struct Service {
    attachments: Arc<Mutex<attachments::Attachments>>,
    lifecycle: Arc<upgrade::Lifecycle>,
    terminals: terminal::SharedTerminals,
    database: Database,
    runs: Runs,
    title_refresh: Arc<Mutex<runtime::TitleRefreshCache>>,
    command_probes: Arc<tokio::sync::Semaphore>,
    file_admission: Arc<tokio::sync::RwLock<()>>,
    agent_binary: Option<PathBuf>,
    agent: AgentInfo,
    server_id: String,
    providers: Arc<Mutex<providers::ProviderStore>>,
}
impl Service {
    fn executable(&self) -> Result<&Path> {
        self.agent_binary
            .as_deref()
            .context("Agent 原生 CLI 路径未解析")
    }
}
fn main() -> std::process::ExitCode {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "__terminal")
    {
        return match terminal::helper() {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("terminal helper: {error:#}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    let bridge = std::env::args_os().nth(1).is_some_and(|command| {
        command == "connect" || command == "connect-local" || command == "connect-existing"
    });
    let runtime = if bridge {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
    } else {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
    };
    let runtime = match runtime {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("latte-work-server: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(entry());
    if bridge {
        // Tokio stdin's blocking read cannot be cancelled when IPC reaches EOF.
        // All bridge futures/streams are already dropped; bound runtime shutdown
        // before immediately returning from this disposable process. At most one
        // pending stdin read remains until OS process exit, with no runtime reuse.
        runtime.shutdown_timeout(Duration::from_millis(250));
    } else {
        // The daemon retains normal runtime shutdown after its owned cleanup.
        drop(runtime);
    }
    if let Err(error) = result {
        eprintln!("latte-work-server: {error:#}");
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}
async fn entry() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.is_empty() || args[0] == "--help" {
        println!(
            "latte-work-server 0.1.0\nUsage: latte-work-server <serve|connect|connect-local|connect-existing> [--state-dir PATH]\nOne daemon per host user; connect starts it if absent; connect-existing never starts it.\nLATTE_WORK_CLAUDE: absolute CLI path override (default auto-detected claude)."
        );
        return Ok(());
    }
    let dir = if args.len() == 3 && args[1] == "--state-dir" {
        PathBuf::from(&args[2])
    } else if args.len() == 1 {
        host_paths::default_state_dir().context("无法确定状态目录")?
    } else {
        bail!("无效参数；使用 --help 查看用法");
    };
    let dir = prepare_dir(&dir)?;
    let endpoint = Endpoint::prepare(&dir)?;
    match args[0].to_str().context("命令名称不是 UTF-8")? {
        "serve" => serve(&endpoint).await,
        "connect" => bridge(&endpoint).await,
        "connect-existing" => match endpoint.connect(Duration::from_secs(3)).await {
            Ok(stream) => forward(stream).await,
            Err(error) => {
                let response = Response::Error {
                    code: "server_unavailable".into(),
                    message: format!(
                        "无法连接独立运行的 Server；请先在远端同一用户的本机终端运行 latte-work-server.exe serve 并保持运行（不要在 SSH 会话内启动）：{error}"
                    ),
                };
                let mut bytes = serde_json::to_vec(&response)?;
                bytes.push(b'\n');
                tokio::io::stdout().write_all(&bytes).await?;
                Ok(())
            }
        },
        "connect-local" => match upgrade::connect_local(&endpoint).await {
            Ok(stream) => forward(stream).await,
            Err(error) => {
                let response = Response::Error {
                    code: "local_upgrade_required".into(),
                    message: error.to_string(),
                };
                let mut bytes = serde_json::to_vec(&response)?;
                bytes.push(b'\n');
                let mut output = tokio::io::stdout();
                output.write_all(&bytes).await?;
                output.flush().await?;
                Ok(())
            }
        },
        _ => bail!("未知命令"),
    }
}
fn prepare_dir(dir: &Path) -> Result<PathBuf> {
    Ok(prepare_state_dir(dir)?)
}
async fn bridge(endpoint: &Endpoint) -> Result<()> {
    forward(connect_server(endpoint).await?).await
}
async fn connect_server(endpoint: &Endpoint) -> Result<Stream> {
    match endpoint.connect(Duration::from_secs(3)).await {
        Ok(stream) => return Ok(stream),
        Err(error) if ipc::daemon_absent(&error) => {}
        Err(error) => return Err(error).context("无法验证或连接 Host Server"),
    }
    let mut command = tokio::process::Command::new(std::env::current_exe()?);
    command
        .arg("serve")
        .arg("--state-dir")
        .arg(endpoint.state_dir());
    let mut daemon = process::spawn_daemon(&mut command).context("无法启动 Host Server")?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            bail!("无法连接 Server；请运行 latte-work-server serve 查看诊断");
        }
        match endpoint
            .connect(remaining.min(Duration::from_secs(3)))
            .await
        {
            Ok(stream) => return Ok(stream),
            Err(error)
                if ipc::daemon_absent(&error) || error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("无法验证或连接 Host Server"),
        }
        // A concurrent bridge may win the singleton lock; wait for its endpoint.
        let _ = daemon.try_wait();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
async fn forward(stream: Stream) -> Result<()> {
    let (mut reader, mut writer) = stream.into_split();
    let mut input = tokio::io::stdin();
    let mut output = tokio::io::stdout();
    let result = tokio::select! {
        result = tokio::io::copy(&mut input, &mut writer) => result,
        result = tokio::io::copy(&mut reader, &mut output) => result,
    };
    // Close both IPC halves before the binary bounds its stdio runtime lifetime.
    drop(reader);
    drop(writer);
    drop(input);
    result?;
    tokio::time::timeout(Duration::from_secs(1), output.flush())
        .await
        .context("bridge stdout flush timed out")??;
    Ok(())
}
async fn serve(endpoint: &Endpoint) -> Result<()> {
    let mut shutdown = Shutdown::new().context("无法注册 Host 退出通知")?;
    // Binding owns the singleton lease and validates transport before recovery/probes.
    let listener = endpoint
        .bind()
        .await
        .context("无法绑定 Host Server 控制通道")?;
    let dir = endpoint.state_dir();
    let database = Arc::new(Mutex::new(store::Store::open(&dir.join("state.sqlite"))?));
    let environment_warning = agent_environment::initialize().await;
    let (agent_binary, mut agent) = agents::discover().await;
    if let Some(warning) = environment_warning {
        agent.detail = format!("{} · {warning}", agent.detail);
    }
    let service = Service {
        attachments: Arc::new(Mutex::new(attachments::Attachments::new(dir)?)),
        lifecycle: Arc::new(upgrade::Lifecycle::new()?),
        terminals: Arc::new(Mutex::new(terminal::Terminals::default())),
        providers: Arc::new(Mutex::new(providers::ProviderStore::open(dir)?)),
        database,
        runs: Arc::new(Mutex::new(HashMap::new())),
        title_refresh: Arc::new(Mutex::new(runtime::TitleRefreshCache::default())),
        command_probes: Arc::new(tokio::sync::Semaphore::new(2)),
        file_admission: Arc::new(tokio::sync::RwLock::new(())),
        agent_binary,
        agent,
        server_id: uuid::Uuid::new_v4().to_string(),
    };
    eprintln!(
        "Latte Work server {} listening at {}",
        service.server_id,
        endpoint.address()
    );
    let mut clients = tokio::task::JoinSet::new();
    let accept_result: Result<()> = loop {
        tokio::select! {
            result=listener.accept()=>{
                match result {
                    Ok(stream) => {
                        let service=service.clone();
                        clients.spawn(async move {if let Err(e)=connection(stream,service).await {eprintln!("client disconnected: {e}");}});
                    }
                    Err(error) if matches!(error.kind(), std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::ConnectionReset) => {
                        eprintln!("IPC connection rejected: {error}");
                    }
                    Err(error) => break Err(error.into()),
                }
            },
            _=clients.join_next(), if !clients.is_empty()=>{},
            _=shutdown.recv()=>break Ok(()),
            _=service.lifecycle.shutdown.notified()=>break Ok(()),
        }
    };
    // Fence requests on existing connections as well as new accepts. If an
    // in-flight dispatch will not yield, cancel its connection before fencing.
    if let Ok(mut admission) =
        tokio::time::timeout(Duration::from_secs(2), service.lifecycle.draining.write()).await
    {
        *admission = true;
    } else {
        clients.shutdown().await;
        *service.lifecycle.draining.write().await = true;
    }
    let (cleanup, terminals) = tokio::join!(
        runtime::shutdown(&service.runs),
        terminal::close_all(&service.terminals)
    );
    clients.shutdown().await;
    drop(listener);
    cleanup?;
    terminals?;
    accept_result
}
async fn connection(stream: Stream, service: Service) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = FramedRead::new(reader, LinesCodec::new_with_max_length(MAX_FRAME));
    let mut ready = false;
    while let Some(line) = lines.next().await {
        let line = line?;
        if let Ok(request) = serde_json::from_str::<upgrade::Request>(&line) {
            let response = upgrade::handle(&service, request).await;
            let mut bytes = serde_json::to_vec(&response)?;
            bytes.push(b'\n');
            let written =
                tokio::time::timeout(Duration::from_secs(15), writer.write_all(&bytes)).await;
            if matches!(response, upgrade::Response::UpgradeReady) {
                service.lifecycle.shutdown.notify_one();
            }
            written??;
            continue;
        }
        // Hold admission through dispatch so a Send/CreateTerminal cannot race an idle check.
        let draining = service.lifecycle.draining.read().await;
        let response = if *draining {
            Response::Error {
                code: "server_upgrading".into(),
                message: "本机后台正在更新，请重新连接；任务不会自动重发".into(),
            }
        } else {
            match serde_json::from_str::<Request>(&line) {
                Ok(Request::Hello { version }) => {
                    if version == VERSION {
                        ready = true;
                        Response::Hello {
                            version: VERSION,
                            server_id: service.server_id.clone(),
                            path_syntax: Some(host_paths::PATH_SYNTAX),
                            agents: vec![service.agent.clone()],
                            permission_settings: true,
                            history_window: true,
                            terminal: terminal::SUPPORTED,
                            undo: task_changes::UNDO_SUPPORTED,
                        }
                    } else {
                        Response::Error {
                            code: "version_mismatch".into(),
                            message: format!("Server 协议为 {VERSION}，客户端为 {version}"),
                        }
                    }
                }
                Ok(request) if ready => match dispatch(&service, request).await {
                    Ok(response) => response,
                    Err(error) => Response::Error {
                        code: "request_failed".into(),
                        message: error.to_string(),
                    },
                },
                Ok(_) => Response::Error {
                    code: "handshake_required".into(),
                    message: "请先发送 hello".into(),
                },
                Err(error) => Response::Error {
                    code: "invalid_request".into(),
                    message: error.to_string(),
                },
            }
        };
        let mut bytes = serde_json::to_vec(&response)?;
        if bytes.len() > MAX_FRAME {
            bytes = serde_json::to_vec(&Response::Error {
                code: "response_too_large".into(),
                message: "响应超出限制".into(),
            })?;
        }
        bytes.push(b'\n');
        tokio::time::timeout(Duration::from_secs(15), writer.write_all(&bytes)).await??;
    }
    Ok(())
}
async fn dispatch(s: &Service, request: Request) -> Result<Response> {
    let allow_queue = matches!(&request, Request::SendQueued { .. });
    let request = match request {
        Request::SendQueued {
            provider,
            session_id,
            request_id,
            text,
            model,
            effort,
            permission_mode,
        } => Request::Send {
            provider,
            session_id,
            request_id,
            text,
            model,
            effort,
            permission_mode,
        },
        other => other,
    };
    // Undo and archive hold exclusive admission through their durable result.
    // Archive must not race a new open/send between closing and saving metadata.
    // Ordinary turns retain concurrent admission; no request is replayed.
    let exclusive = matches!(
        &request,
        Request::UndoTurnChanges { .. } | Request::ArchiveSession { archived: true, .. }
    );
    let _exclusive_guard = if exclusive {
        Some(s.file_admission.clone().write_owned().await)
    } else {
        None
    };
    let _write_guard = if matches!(
        &request,
        Request::Send { .. }
            | Request::OpenAgentSession { .. }
            | Request::CreateTerminal { .. }
            | Request::WriteTerminal { .. }
            | Request::Approve { .. }
            | Request::AnswerQuestion { .. }
            | Request::ResumeMessageQueue { .. }
            | Request::DispatchQueuedMessage { .. }
    ) {
        Some(s.file_admission.read().await)
    } else {
        None
    };
    let closed_resource = match &request {
        Request::CloseAgentSession { session_id, .. }
        | Request::ArchiveSession {
            session_id,
            archived: true,
        } => Some(upgrade::Resource::AgentSession(session_id.clone())),
        Request::CloseTerminal { terminal_id } => {
            Some(upgrade::Resource::Terminal(terminal_id.clone()))
        }
        _ => None,
    };
    let mut response = match request {
        Request::Hello { .. } => unreachable!(),
        Request::Providers => s
            .providers
            .lock()
            .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?
            .list(),
        Request::AgentPermissions { agent } => {
            if agent != s.agent.id || !s.agent.available {
                bail!("此 Agent 不可用或尚未实现：{agent}");
            }
            let _permit = s
                .command_probes
                .try_acquire()
                .context("权限选项正在加载，请稍后重试")?;
            Response::AgentPermissions {
                modes: agents::permission_modes(&agent, s.executable()?).await?,
            }
        }
        Request::AgentCommands { agent, project_id } => {
            if agent != s.agent.id || !s.agent.available {
                bail!("此 Agent 不可用或尚未实现：{agent}");
            }
            let project = db(&s.database, |d| d.project(&project_id))?;
            let _permit = s
                .command_probes
                .try_acquire()
                .context("命令列表正在加载，请稍后重试")?;
            let config = s
                .providers
                .lock()
                .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?
                .selected_config(&agent, None)?;
            Response::AgentCommands {
                commands: runtime::commands(s.executable()?, &agent, &project.path, &config)
                    .await?,
            }
        }
        Request::Models {
            agent,
            model,
            project_id,
        } => {
            let project = project_id
                .map(|id| db(&s.database, |d| d.project(&id)))
                .transpose()?;
            let mut response = s
                .providers
                .lock()
                .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?
                .models(&agent, model.as_deref())?;
            if let Response::Models {
                provider: None,
                model_labels,
                ..
            } = &mut response
            {
                *model_labels = agents::model_labels(
                    &agent,
                    project.as_ref().map(|p| std::path::Path::new(&p.path)),
                );
            }
            response
        }
        Request::SaveProvider { provider } => s
            .providers
            .lock()
            .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?
            .save(provider)?,
        Request::DeleteProvider { id } => s
            .providers
            .lock()
            .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?
            .delete(&id)?,
        Request::BindAgentProvider {
            agent,
            provider_id,
            target,
        } => {
            if let Some(target) = target {
                let snapshot = provider_id
                    .as_deref()
                    .map(|id| {
                        s.providers
                            .lock()
                            .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?
                            .snapshot(&agent, id)
                    })
                    .transpose()?;
                let mut client =
                    latte_work_client::Client::ssh(&target.ssh, &target.server_path).await?;
                let response = client
                    .request(Request::SyncAgentProvider { agent, snapshot })
                    .await?;
                match response {
                    Response::Providers { .. } => response,
                    Response::Error { message, .. } => bail!("远程同步失败：{message}"),
                    _ => bail!("远程 Provider 响应格式不匹配"),
                }
            } else {
                s.providers
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?
                    .bind(&agent, provider_id)?
            }
        }
        Request::ProvidersForHost { host_id } => s
            .providers
            .lock()
            .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?
            .list_for_host(&host_id)?,
        Request::BindHostAgentProvider {
            host_id,
            agent,
            provider_id,
        } => s
            .providers
            .lock()
            .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?
            .bind_host(&host_id, &agent, provider_id)?,
        Request::ForgetHostProviders { host_id } => s
            .providers
            .lock()
            .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?
            .forget_host(&host_id)?,
        Request::ExportHostAgentProvider { host_id, agent } => {
            let store = s
                .providers
                .lock()
                .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?;
            Response::ProviderSnapshot {
                snapshot: store.snapshot_for_host(&host_id, &agent)?,
                configured: store.host_configured(&host_id),
            }
        }
        Request::ModelsForProvider {
            agent,
            model,
            project_id,
            provider,
        } => {
            let project = project_id
                .map(|id| db(&s.database, |d| d.project(&id)))
                .transpose()?;
            let mut response =
                providers::ProviderStore::models_for_provider(&agent, model.as_deref(), provider)?;
            if let Response::Models {
                provider: None,
                model_labels,
                ..
            } = &mut response
            {
                *model_labels = agents::model_labels(
                    &agent,
                    project.as_ref().map(|p| std::path::Path::new(&p.path)),
                );
            }
            response
        }
        Request::Session { session_id } => Response::Session {
            session: db(&s.database, |d| d.session(&session_id))?,
        },
        Request::SyncAgentProvider { agent, snapshot } => s
            .providers
            .lock()
            .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?
            .sync(&agent, snapshot)?,
        Request::WriteTerminal { terminal_id, data } => {
            terminal::write(&s.terminals, &terminal_id, data).await?
        }
        Request::CreateTerminal {
            project_id,
            terminal_id,
            cols,
            rows,
        } => {
            let project = db(&s.database, |d| d.project(&project_id))?;
            terminal::create(
                &s.terminals,
                project_id,
                Path::new(&project.path),
                terminal_id,
                cols,
                rows,
            )
            .await?
        }
        Request::Terminals { project_id } => {
            db(&s.database, |d| d.project(&project_id))?;
            s.terminals
                .lock()
                .map_err(|_| anyhow::anyhow!("终端锁异常"))?
                .list(&project_id)?
        }
        Request::ReadTerminal { terminal_id, after } => s
            .terminals
            .lock()
            .map_err(|_| anyhow::anyhow!("终端锁异常"))?
            .read(&terminal_id, after)?,
        Request::ResizeTerminal {
            terminal_id,
            cols,
            rows,
        } => terminal::resize(&s.terminals, &terminal_id, cols, rows).await?,
        Request::CloseTerminal { terminal_id } => {
            terminal::close(&s.terminals, &terminal_id).await?
        }
        Request::Projects => Response::Projects {
            projects: db(&s.database, |d| d.projects())?,
        },
        Request::AddProject { path, name } => Response::Project {
            project: db(&s.database, |d| {
                d.add_named_project(Path::new(&path), name.as_deref())
            })?,
        },
        Request::RenameProject { project_id, name } => Response::Project {
            project: db(&s.database, |d| d.rename_project(&project_id, &name))?,
        },
        Request::RemoveProject { project_id } => {
            db(&s.database, |d| d.remove_project(&project_id))?;
            Response::Projects {
                projects: db(&s.database, |d| d.projects())?,
            }
        }
        Request::BrowseDirectories { path } => {
            tokio::task::spawn_blocking(move || files::browse_directories(path.as_deref()))
                .await??
        }
        Request::Sessions { project_id } => Response::Sessions {
            sessions: db(&s.database, |d| d.sessions(&project_id))?,
        },
        Request::PinnedSessions => Response::Sessions {
            sessions: db(&s.database, |d| d.pinned_sessions())?,
        },
        Request::RecentSessions { before } => {
            let (sessions, next) = db(&s.database, |d| d.recent_sessions(before.as_ref()))?;
            Response::RecentSessions { sessions, next }
        }
        Request::RenameSession { session_id, title } => {
            let session = db(&s.database, |d| d.session(&session_id))?;
            let project = db(&s.database, |d| d.project(&session.project_id))?;
            Response::Session {
                session: runtime::rename_title(&s.database, &s.runs, session, project.path, title)
                    .await?,
            }
        }
        Request::PinSession { session_id, pinned } => Response::Session {
            session: db(&s.database, |d| d.pin_session(&session_id, pinned))?,
        },
        Request::MarkSessionUnread { session_id, unread } => Response::Session {
            session: db(&s.database, |d| d.mark_session_unread(&session_id, unread))?,
        },
        Request::ArchiveSession {
            session_id,
            archived,
        } => Response::Session {
            session: runtime::archive(&s.database, &s.runs, &session_id, archived).await?,
        },
        Request::CreateSession { project_id, agent } => {
            if agent != s.agent.id {
                bail!("此 Agent 尚未实现");
            }
            Response::Session {
                session: db(&s.database, |d| d.create_session(project_id, agent))?,
            }
        }
        Request::OpenAgentSession {
            session_id,
            provider,
        } => {
            if !s.agent.available {
                bail!("此 Host 的 Agent CLI 不可用");
            }
            let (session, path) = db(&s.database, |d| {
                let session = d.session(&session_id)?;
                let path = d.project(&session.project_id)?.path;
                Ok((session, path))
            })?;
            if session.archived {
                bail!("请先取消归档再打开 Agent 会话");
            }
            let mut config = s
                .providers
                .lock()
                .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?
                .open_config(&session.agent, session.model.as_deref(), provider)?;
            config.effort = session.effort.filter(|level| {
                agents::effort_levels(&session.agent, config.model.as_deref()).contains(level)
            });
            config.permission_mode = session.permission_mode.clone();
            runtime::open(
                s.database.clone(),
                s.runs.clone(),
                s.executable()?.to_owned(),
                session,
                path,
                config,
            )
            .await?;
            Response::Ok
        }
        Request::CloseAgentSession {
            session_id,
            only_if_idle,
        } => {
            runtime::close(
                &s.database,
                &s.runs,
                &session_id,
                only_if_idle.unwrap_or(false),
            )
            .await?;
            Response::Ok
        }
        Request::SendQueued { .. } => unreachable!(),
        Request::Send {
            provider,
            session_id,
            request_id,
            text,
            model,
            effort,
            permission_mode,
        } => {
            if allow_queue && provider.is_none() {
                bail!("消息排队需要明确的执行配置，请更新客户端");
            }
            let fingerprint = providers::turn_fingerprint(provider.as_ref())?;
            if !allow_queue
                && db(&s.database, |d| {
                    d.already_accepted(
                        &session_id,
                        &request_id,
                        &text,
                        model.as_deref(),
                        effort,
                        fingerprint.as_deref(),
                        permission_mode.as_deref(),
                    )
                })?
            {
                return Ok(Response::Accepted { duplicate: true });
            }
            if !s.agent.available {
                bail!(
                    "{} CLI 不可用；请在此 Host 安装并登录后重启 Server",
                    s.agent.name
                );
            }
            let agent_id = db(&s.database, |d| Ok(d.session(&session_id)?.agent))?;
            if agent_id != s.agent.id {
                bail!("此 Agent 尚未实现：{agent_id}");
            }
            let mut launch_config = s
                .providers
                .lock()
                .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?
                .turn_config(&agent_id, model.as_deref(), provider)?;
            let selected_model = launch_config.model.as_deref().or_else(|| {
                launch_config
                    .provider
                    .as_ref()
                    .map(|p| p.metadata.model.as_str())
            });
            if effort.is_some_and(|level| {
                !agents::effort_levels(&agent_id, selected_model).contains(&level)
            }) {
                bail!("此 Agent 不支持所选思考强度");
            }
            if let Some(mode) = &permission_mode {
                let _permit = s
                    .command_probes
                    .try_acquire()
                    .context("权限能力正在加载，请稍后重试")?;
                if !agents::permission_modes(&agent_id, s.executable()?)
                    .await?
                    .iter()
                    .any(|item| &item.id == mode)
                {
                    bail!("此 Agent 不支持所选权限模式：{mode}");
                }
            }
            launch_config.permission_mode = permission_mode.clone();
            launch_config.effort = effort;
            let (session, path) = db(&s.database, |d| {
                let session = d.session(&session_id)?;
                let path = d.project(&session.project_id)?.path;
                Ok((session, path))
            })?;
            runtime::submit(
                s.database.clone(),
                s.runs.clone(),
                s.executable()?.to_owned(),
                session,
                path,
                runtime::SendTurn {
                    request_id,
                    text,
                    model,
                    fingerprint,
                    config: launch_config,
                },
                allow_queue,
            )
            .await?
        }
        Request::MessageQueue { session_id } => db(&s.database, |d| d.queue_snapshot(&session_id))?,
        request @ (Request::TakeQueuedMessage { .. }
        | Request::DispatchQueuedMessage { .. }
        | Request::ReorderMessageQueue { .. }
        | Request::EditQueuedMessage { .. }
        | Request::CancelQueuedMessage { .. }
        | Request::ClearMessageQueue { .. }
        | Request::ResumeMessageQueue { .. }) => {
            let id = match &request {
                Request::TakeQueuedMessage { session_id, .. }
                | Request::DispatchQueuedMessage { session_id, .. }
                | Request::ReorderMessageQueue { session_id, .. }
                | Request::EditQueuedMessage { session_id, .. }
                | Request::CancelQueuedMessage { session_id, .. }
                | Request::ClearMessageQueue { session_id }
                | Request::ResumeMessageQueue { session_id, .. } => session_id.clone(),
                _ => unreachable!(),
            };
            let (session, path) = db(&s.database, |d| {
                let session = d.session(&id)?;
                let path = d.project(&session.project_id)?.path;
                Ok((session, path))
            })?;
            let action = match request {
                Request::TakeQueuedMessage {
                    request_id,
                    revision,
                    ..
                } => runtime::QueueAction::Take {
                    request_id,
                    revision,
                },
                Request::DispatchQueuedMessage {
                    interrupt,
                    request_id,
                    revision,
                    provider,
                    ..
                } => {
                    if provider.is_none() || !s.agent.available || session.archived {
                        bail!("Agent 不可用、会话已归档或执行配置未确认");
                    }
                    let fingerprint = providers::turn_fingerprint(provider.as_ref())?;
                    let (entry, original) = db(&s.database, |d| d.queued(&id))?
                        .into_iter()
                        .find(|(e, _)| e.request_id == request_id && e.revision == revision)
                        .context("消息已开始执行、移除或更新，请刷新队列")?;
                    if fingerprint != original {
                        bail!("排队时的 Provider 配置已改变，请取消后重新发送");
                    }
                    let mut config = s
                        .providers
                        .lock()
                        .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?
                        .turn_config(&session.agent, entry.model.as_deref(), provider)?;
                    config.effort = entry.effort;
                    config.permission_mode = entry.permission_mode;
                    runtime::QueueAction::Dispatch {
                        interrupt,
                        request_id,
                        revision,
                        config: Box::new(config),
                    }
                }
                Request::ReorderMessageQueue {
                    expected, order, ..
                } => runtime::QueueAction::Reorder { expected, order },
                Request::EditQueuedMessage {
                    request_id,
                    revision,
                    text,
                    ..
                } => runtime::QueueAction::Edit {
                    request_id,
                    revision,
                    text: Some(text),
                },
                Request::CancelQueuedMessage {
                    request_id,
                    revision,
                    ..
                } => runtime::QueueAction::Edit {
                    request_id,
                    revision,
                    text: None,
                },
                Request::ClearMessageQueue { .. } => runtime::QueueAction::Clear,
                Request::ResumeMessageQueue { provider, .. } => {
                    if provider.is_none() {
                        bail!("继续排队需要重新确认执行配置，请更新客户端");
                    }
                    if !s.agent.available {
                        bail!("Agent CLI 不可用，请检查主机");
                    }
                    if session.archived {
                        bail!("请先取消归档再继续排队");
                    }
                    let fingerprint = providers::turn_fingerprint(provider.as_ref())?;
                    let entries = db(&s.database, |d| d.queued(&id))?;
                    let mut configs = HashMap::new();
                    let providers = s
                        .providers
                        .lock()
                        .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?;
                    for (entry, original) in entries {
                        if fingerprint != original {
                            bail!("排队时的 Provider 配置已改变，请核对后取消并重新发送消息");
                        }
                        let mut config = providers.turn_config(
                            &session.agent,
                            entry.model.as_deref(),
                            provider.clone(),
                        )?;
                        config.effort = entry.effort;
                        config.permission_mode = entry.permission_mode;
                        configs.insert(entry.request_id, config);
                    }
                    runtime::QueueAction::Resume { configs }
                }
                _ => unreachable!(),
            };
            runtime::queue_action(
                s.database.clone(),
                s.runs.clone(),
                s.agent_binary
                    .clone()
                    .context("Agent CLI 不可用，请检查主机")?,
                session,
                path,
                action,
            )
            .await?
        }
        Request::History { session_id, before } => db(&s.database, |d| {
            let session = d.session(&session_id)?;
            let (events, has_more, needs_earlier, before) = d.history(&session_id, before)?;
            Ok(Response::History {
                session,
                events,
                has_more,
                needs_earlier,
                before,
            })
        })?,
        Request::Subagents { session_id } => db(&s.database, |d| {
            d.session(&session_id)?;
            let (tasks, truncated) = d.subagents(&session_id)?;
            Ok(Response::Subagents { tasks, truncated })
        })?,
        Request::SubagentHistory {
            provider,
            session_id,
            task_id,
            after,
            source,
        } => {
            db(&s.database, |d| d.subagent(&session_id, &task_id))?;
            let control = s
                .runs
                .lock()
                .map_err(|_| anyhow::anyhow!("run lock poisoned"))?
                .get(&session_id)
                .map(|run| run.control.clone());
            let mut source_has_more = false;
            if let Some(control) = control {
                let permit = agents::child_read_permit()?;
                let (reply, receive) = oneshot::channel();
                if control
                    .send(Control::SubagentHistory {
                        task_id: task_id.clone(),
                        permit,
                        reply,
                    })
                    .await
                    .is_ok()
                {
                    source_has_more = tokio::time::timeout(Duration::from_secs(10), receive)
                        .await??
                        .map_err(anyhow::Error::msg)?;
                }
            }
            let cursor = db(&s.database, |d| {
                d.child_history_cursor(&session_id, &task_id)
            })?;
            if !source_has_more && (cursor.identity.is_empty() || cursor.has_more) {
                let fingerprint = providers::turn_fingerprint(provider.as_ref())?;
                let accepted = db(&s.database, |d| {
                    d.child_provider_fingerprint(&session_id, &task_id)
                })?;
                let session = db(&s.database, |d| d.session(&session_id))?;
                let config = s
                    .providers
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Provider 配置锁异常"))?
                    .turn_config(&session.agent, None, provider)?;
                // A legacy request with no snapshot is only safe when it also
                // used native defaults. A changed/unknown context serves cache.
                if accepted == Some(fingerprint.clone())
                    && (fingerprint.is_some() || config.provider.is_none())
                    && let Some(parent) = session.native_id
                {
                    let (child, cwd) = db(&s.database, |d| {
                        Ok((
                            d.subagent(&session_id, &task_id)?,
                            d.project(&session.project_id)?.path,
                        ))
                    })?;
                    let permit = agents::child_read_permit()?;
                    if let Some(page) = agents::child_history(
                        std::path::PathBuf::from(cwd),
                        parent,
                        child.native_id,
                        session_id.clone(),
                        config,
                        cursor,
                        permit,
                    )
                    .await?
                    {
                        source_has_more = page.has_more;
                        db(&s.database, |d| {
                            d.cache_child_history(&session_id, &task_id, &page)
                        })?;
                    }
                }
            }
            db(&s.database, |d| {
                d.subagent_history(
                    &session_id,
                    &task_id,
                    after,
                    source.as_deref(),
                    source_has_more,
                )
            })?
        }
        Request::SubagentTranscript {
            session_id,
            task_id,
        } => {
            db(&s.database, |d| d.subagent(&session_id, &task_id))?;
            let control = s
                .runs
                .lock()
                .map_err(|_| anyhow::anyhow!("run lock poisoned"))?
                .get(&session_id)
                .map(|run| run.control.clone());
            if let Some(control) = control {
                let permit = agents::child_read_permit()?;
                let (reply, receive) = oneshot::channel();
                if control
                    .send(Control::SubagentTranscript {
                        task_id: task_id.clone(),
                        permit,
                        reply,
                    })
                    .await
                    .is_ok()
                {
                    tokio::time::timeout(Duration::from_secs(10), receive)
                        .await??
                        .map_err(anyhow::Error::msg)?;
                }
            }
            let (events, truncated) = db(&s.database, |d| {
                let snapshot = d.subagent_transcript(&session_id, &task_id)?;
                agents::normalize_child_transcript(&d.session(&session_id)?.agent, snapshot)
            })?;
            Response::SubagentTranscript { events, truncated }
        }
        Request::Poll { session_id, after } => db(&s.database, |d| {
            let session = d.session(&session_id)?;
            let (events, has_more) = d.events(&session_id, after)?;
            Ok(Response::Events {
                session,
                events,
                has_more,
            })
        })?,
        Request::AnswerQuestion {
            session_id,
            request_id,
            answers,
        } => {
            let control = s
                .runs
                .lock()
                .map_err(|_| anyhow::anyhow!("run lock poisoned"))?
                .get(&session_id)
                .map(|run| run.control.clone())
                .context("此会话没有运行中的任务")?;
            let (reply, receive) = oneshot::channel();
            control
                .send(Control::AnswerQuestion {
                    id: request_id,
                    answers,
                    reply,
                })
                .await?;
            tokio::time::timeout(Duration::from_secs(10), receive)
                .await??
                .map_err(anyhow::Error::msg)?;
            Response::Ok
        }
        Request::Approve {
            session_id,
            request_id,
            allow,
        } => {
            let control = s
                .runs
                .lock()
                .map_err(|_| anyhow::anyhow!("run lock poisoned"))?
                .get(&session_id)
                .map(|run| run.control.clone())
                .context("此会话没有运行中的任务")?;
            let (reply, receive) = oneshot::channel();
            control
                .send(Control::Approval {
                    id: request_id,
                    allow,
                    reply,
                })
                .await?;
            tokio::time::timeout(Duration::from_secs(10), receive)
                .await??
                .map_err(anyhow::Error::msg)?;
            Response::Ok
        }
        Request::Cancel { session_id } => {
            let control = s
                .runs
                .lock()
                .map_err(|_| anyhow::anyhow!("run lock poisoned"))?
                .get(&session_id)
                .map(|run| run.control.clone())
                .context("此会话没有运行中的任务")?;
            let (reply, receive) = oneshot::channel();
            control.send(Control::Interrupt { reply }).await?;
            tokio::time::timeout(Duration::from_secs(15), receive)
                .await??
                .map_err(anyhow::Error::msg)?;
            Response::Ok
        }
        Request::Files { project_id, path } => {
            let p = db(&s.database, |d| d.project(&project_id))?;
            let exclusions = managed_recovery::ManagedRecoveryExclusions::load_live(
                &s.database,
                Path::new(&p.path),
            )
            .await?;
            Response::Files {
                entries: files::list_excluding(Path::new(&p.path), &path, &exclusions)?,
            }
        }
        Request::BeginAttachment { name, size } => Response::AttachmentUpload {
            id: s
                .attachments
                .lock()
                .map_err(|_| anyhow::anyhow!("附件存储锁异常"))?
                .begin(name, size)?,
        },
        Request::AttachmentChunk { id, offset, data } => {
            s.attachments
                .lock()
                .map_err(|_| anyhow::anyhow!("附件存储锁异常"))?
                .chunk(&id, offset, &data)?;
            Response::Ok
        }
        Request::FinishAttachment { id } => Response::FileReference {
            entry: s
                .attachments
                .lock()
                .map_err(|_| anyhow::anyhow!("附件存储锁异常"))?
                .finish(&id)?,
        },
        Request::AbortAttachment { id } => {
            s.attachments
                .lock()
                .map_err(|_| anyhow::anyhow!("附件存储锁异常"))?
                .abort(&id);
            Response::Ok
        }
        Request::ResolveReference { project_id, path } => {
            let p = db(&s.database, |d| d.project(&project_id))?;
            let entry = files::reference(Path::new(&p.path), &path).await?;
            if Path::new(&entry.path).is_absolute() {
                db(&s.database, |d| {
                    d.grant_reference(&project_id, &entry.path, entry.directory)
                })?;
            }
            Response::FileReference { entry }
        }
        Request::ReferenceAccess { project_id } => {
            let (entries, truncated) = db(&s.database, |d| d.reference_access(&project_id))?;
            Response::ReferenceAccess { entries, truncated }
        }
        Request::RevokeReferenceAccess { project_id, path } => {
            if path.len() > 4096 || path.contains('\0') || !Path::new(&path).is_absolute() {
                bail!("来源路径无效");
            }
            let removed = db(&s.database, |d| {
                d.revoke_reference_access(&project_id, &path)
            })?;
            // Preserve deletion of a recorded key after the file has disappeared;
            // otherwise resolve existing aliases without blocking the database lock.
            if !removed {
                let canonical =
                    tokio::time::timeout(Duration::from_secs(5), tokio::fs::canonicalize(&path))
                        .await
                        .context("读取路径超时")?;
                if let Ok(canonical) = canonical {
                    let canonical = canonical.to_str().context("路径不是 UTF-8")?;
                    db(&s.database, |d| {
                        d.revoke_reference_access(&project_id, canonical)
                    })?;
                }
            }
            Response::Ok
        }
        Request::SkillInfo { session_id, name } => {
            let session = db(&s.database, |d| d.session(&session_id))?;
            let project = db(&s.database, |d| d.project(&session.project_id))?;
            Response::SkillInfo {
                skill: agents::skill_info(&session.agent, project.path.into(), name).await?,
            }
        }
        Request::Sources { session_id } => {
            let (entries, truncated) = db(&s.database, |d| d.sources(&session_id))?;
            Response::Sources { entries, truncated }
        }
        Request::ChangeSummary {
            project_id,
            session_id,
        } => {
            let p = db(&s.database, |d| d.project(&project_id))?;
            let summary = if let Some(id) = session_id {
                let session = db(&s.database, |d| d.session(&id))?;
                if session.project_id != project_id {
                    bail!("任务不属于当前项目");
                }
                let baseline = db(&s.database, |d| d.baseline(&id))?.unwrap_or_else(|| {
                    task_changes::Snapshot::unavailable(
                        "此任务没有记录文件基线，请查看工作区改动".into(),
                    )
                });
                let exclusions = managed_recovery::ManagedRecoveryExclusions::load_live(
                    &s.database,
                    Path::new(&p.path),
                )
                .await?;
                baseline.summary(Path::new(&p.path), &exclusions).await?
            } else {
                let exclusions = managed_recovery::ManagedRecoveryExclusions::load_live(
                    &s.database,
                    Path::new(&p.path),
                )
                .await?;
                task_changes::workspace_summary(Path::new(&p.path), &exclusions).await?
            };
            Response::ChangeSummary { summary }
        }
        Request::UndoTurnChanges {
            session_id,
            request_id,
        } => {
            use latte_work_protocol::TurnUndoStatus;
            let mut turn = db(&s.database, |d| d.turn_changes(&session_id, &request_id))?;
            if turn.changes.undo == TurnUndoStatus::Reverted {
                return Ok(Response::TurnChanges {
                    changes: turn.changes,
                });
            }
            if db(&s.database, |d| d.latest_request(&session_id))? != request_id {
                bail!("只能撤销此会话最近一轮的修改，请查看历史 diff 后手动调整");
            }
            if db(&s.database, |d| d.has_active_sessions())?
                || s.runs
                    .lock()
                    .map_err(|_| anyhow::anyhow!("执行状态锁异常"))?
                    .values()
                    .any(|r| !r.safe_to_close.load(std::sync::atomic::Ordering::SeqCst))
                || !s
                    .terminals
                    .lock()
                    .map_err(|_| anyhow::anyhow!("终端锁异常"))?
                    .is_empty()
            {
                bail!("此主机仍有运行任务、后台 Agent 或终端；请结束后再撤销");
            }
            let session = db(&s.database, |d| d.session(&session_id))?;
            let project = db(&s.database, |d| d.project(&session.project_id))?;
            let root = Path::new(&project.path);
            let validation = turn.clone();
            let validation_root = root.to_owned();
            tokio::task::spawn_blocking(move || validation.validate_undo(&validation_root))
                .await??;
            // Close only idle runtimes in overlapping directories before restoration.
            let running_ids: Vec<_> = s
                .runs
                .lock()
                .map_err(|_| anyhow::anyhow!("执行状态锁异常"))?
                .keys()
                .cloned()
                .collect();
            for id in running_ids {
                let session = db(&s.database, |d| d.session(&id))?;
                let other = db(&s.database, |d| d.project(&session.project_id))?;
                let other = Path::new(&other.path);
                if latte_work_platform::paths::overlap(root, other)? {
                    runtime::close(&s.database, &s.runs, &session.id, true).await?;
                }
            }
            #[cfg(windows)]
            let git_dir = task_changes::windows_undo_git_dir(root).await?;
            let root = root.to_owned();
            let database = s.database.clone();
            // The blocking worker owns admission and prepared guards through all
            // mutations and the durable result, even if this connection is cancelled.
            let changes = tokio::task::spawn_blocking(move || {
                let _admission = _exclusive_guard;
                let prepared = turn.prepare_undo_blocking(
                    &root,
                    &database,
                    #[cfg(windows)]
                    git_dir.as_deref(),
                )?;
                prepared.record_intent(&mut turn);
                turn.changes.undo = TurnUndoStatus::Unknown;
                db(&database, |d| d.update_turn(&session_id, &turn))?;
                let _restored = prepared
                    .restore()
                    .context("撤销未能全部完成，状态待核对；请检查工作区，系统不会自动重试")?;
                turn.changes.undo = TurnUndoStatus::Reverted;
                db(&database, |d| d.update_turn(&session_id, &turn))?;
                Ok::<_, anyhow::Error>(turn.changes)
            })
            .await??;
            Response::TurnChanges { changes }
        }
        Request::LastTurnChanges { session_id } => {
            let turn = db(&s.database, |d| d.last_turn_changes(&session_id))?;
            Response::LastTurnChanges {
                changes: turn.map(|turn| turn.changes),
            }
        }
        Request::TurnChangeSummary {
            session_id,
            request_id,
        } => {
            let turn = db(&s.database, |d| d.turn_changes(&session_id, &request_id))?;
            Response::TurnChanges {
                changes: turn.changes,
            }
        }
        Request::TurnChangeDiff {
            session_id,
            request_id,
            path,
        } => {
            let turn = db(&s.database, |d| d.turn_changes(&session_id, &request_id))?;
            let (text, truncated) = turn.patch(&path).await?;
            Response::Content { text, truncated }
        }
        Request::TaskChangeDiff { session_id, path } => {
            let session = db(&s.database, |d| d.session(&session_id))?;
            let project = db(&s.database, |d| d.project(&session.project_id))?;
            let baseline =
                db(&s.database, |d| d.baseline(&session_id))?.context("此任务没有记录文件基线")?;
            let exclusions = managed_recovery::ManagedRecoveryExclusions::load_live(
                &s.database,
                Path::new(&project.path),
            )
            .await?;
            let (text, truncated) = baseline
                .patch(Path::new(&project.path), &path, &exclusions)
                .await?;
            Response::Content { text, truncated }
        }
        Request::PreviewSource {
            project_id,
            session_id,
            path,
            offset,
        } => {
            let p = db(&s.database, |d| d.project(&project_id))?;
            if path.len() > 4096 || path.contains('\0') {
                bail!("来源路径无效");
            }
            let requested = if Path::new(&path).is_absolute() {
                latte_work_platform::paths::absolute(Path::new(&path))?;
                std::path::PathBuf::from(&path)
            } else {
                files::resolve(Path::new(&p.path), &path)?
            };
            let target = tokio::fs::canonicalize(&requested).await?;
            let mut allowed = target.starts_with(&p.path)
                || db(&s.database, |d| d.reference_allowed(&project_id, &target))?;
            allowed |= s
                .attachments
                .lock()
                .map_err(|_| anyhow::anyhow!("附件存储锁异常"))?
                .is_completed(&target);
            if let Some(id) = session_id {
                let session = db(&s.database, |d| d.session(&id))?;
                if session.project_id != project_id {
                    bail!("来源不属于当前项目");
                }
                let (sources, _) = db(&s.database, |d| d.sources(&id))?;
                for source in sources {
                    if let Some(path) = source.path {
                        let original = Path::new(&path);
                        let original = if original.is_absolute() {
                            original.to_path_buf()
                        } else {
                            Path::new(&p.path).join(original)
                        };
                        // An explicit canonical reference never follows a replacement symlink.
                        if original == target
                            || (source.kind == latte_work_protocol::SourceKind::Directory
                                && target.starts_with(&original))
                        {
                            allowed = true;
                            break;
                        }
                    }
                }
            }
            if !allowed {
                bail!("请先将项目外文件或目录添加为此项目的来源");
            }
            sources::preview(&target, offset).await?
        }
        Request::ReadFile { project_id, path } => {
            let p = db(&s.database, |d| d.project(&project_id))?;
            let exclusions = managed_recovery::ManagedRecoveryExclusions::load_live(
                &s.database,
                Path::new(&p.path),
            )
            .await?;
            exclusions.require_visible(Path::new(&p.path), &path)?;
            let (text, truncated) = files::read(Path::new(&p.path), &path).await?;
            Response::Content { text, truncated }
        }
        Request::Changes { project_id } => {
            let p = db(&s.database, |d| d.project(&project_id))?;
            let exclusions = managed_recovery::ManagedRecoveryExclusions::load_live(
                &s.database,
                Path::new(&p.path),
            )
            .await?;
            let (entries, truncated) =
                files::changes_excluding(Path::new(&p.path), &exclusions).await?;
            Response::Changes { entries, truncated }
        }
        Request::GitInfo { project_id } => {
            let p = db(&s.database, |d| d.project(&project_id))?;
            Response::GitInfo {
                info: git_review::info(Path::new(&p.path)).await?,
            }
        }
        Request::GitCommits {
            project_id,
            head,
            offset,
        } => {
            let p = db(&s.database, |d| d.project(&project_id))?;
            let (head, commits, has_more) =
                git_review::commits(Path::new(&p.path), head.as_deref(), offset).await?;
            Response::GitCommits {
                head,
                commits,
                has_more,
            }
        }
        Request::GitReview {
            project_id,
            scope,
            base,
        } => {
            let p = db(&s.database, |d| d.project(&project_id))?;
            let exclusions = if matches!(
                scope,
                latte_work_protocol::GitReviewScope::Branch
                    | latte_work_protocol::GitReviewScope::Commit
            ) {
                None
            } else {
                Some(
                    managed_recovery::ManagedRecoveryExclusions::load_live(
                        &s.database,
                        Path::new(&p.path),
                    )
                    .await?,
                )
            };
            Response::GitReview {
                review: git_review::review_excluding(
                    Path::new(&p.path),
                    scope,
                    base.as_deref(),
                    exclusions.as_ref(),
                )
                .await?,
            }
        }
        Request::GitReviewDiff {
            project_id,
            scope,
            base,
            head,
            path,
            full_context,
        } => {
            let p = db(&s.database, |d| d.project(&project_id))?;
            let exclusions = if matches!(
                scope,
                latte_work_protocol::GitReviewScope::Branch
                    | latte_work_protocol::GitReviewScope::Commit
            ) {
                None
            } else {
                Some(
                    managed_recovery::ManagedRecoveryExclusions::load_live(
                        &s.database,
                        Path::new(&p.path),
                    )
                    .await?,
                )
            };
            let (text, truncated) = git_review::diff_excluding(
                Path::new(&p.path),
                scope,
                base.as_deref(),
                head.as_deref(),
                &path,
                full_context,
                exclusions.as_ref(),
            )
            .await?;
            Response::Content { text, truncated }
        }
        Request::ChangeDiff {
            project_id,
            path,
            section,
        } => {
            let p = db(&s.database, |d| d.project(&project_id))?;
            let (text, truncated) = files::change_diff_excluding(
                Path::new(&p.path),
                &managed_recovery::ManagedRecoveryExclusions::load_live(
                    &s.database,
                    Path::new(&p.path),
                )
                .await?,
                &path,
                section,
            )
            .await?;
            Response::Content { text, truncated }
        }
        Request::Diff { project_id } => {
            let p = db(&s.database, |d| d.project(&project_id))?;
            let exclusions = managed_recovery::ManagedRecoveryExclusions::load_live(
                &s.database,
                Path::new(&p.path),
            )
            .await?;
            let (text, truncated) = files::diff_excluding(Path::new(&p.path), &exclusions).await?;
            Response::Content { text, truncated }
        }
    };
    if let Some(resource) = closed_resource {
        s.lifecycle.forget(&resource)?;
    }
    runtime::refresh_titles(&s.database, &s.runs, &s.title_refresh, &mut response).await?;
    runtime::project_session_state(&s.runs, &mut response)?;
    Ok(response)
}
