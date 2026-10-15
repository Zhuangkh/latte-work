//! A single host daemon; `connect` is a disposable local/SSH byte bridge.
mod agent_environment;
mod agents;
mod attachments;
mod files;
mod git_review;
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
use latte_work_protocol::{AgentInfo, MAX_FRAME, Request, Response, VERSION};
use runtime::{Control, Database, Runs, db};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    net::{UnixListener, UnixStream},
    sync::oneshot,
};
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
    agent_binary: String,
    agent: AgentInfo,
    server_id: String,
    providers: Arc<Mutex<providers::ProviderStore>>,
}
#[tokio::main]
async fn main() {
    if let Err(error) = entry().await {
        eprintln!("latte-work-server: {error:#}");
        std::process::exit(1);
    }
}
async fn entry() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" {
        println!(
            "latte-work-server 0.1.0\nUsage: latte-work-server <serve|connect|connect-local> [--state-dir PATH]\nOne daemon per host user; connect starts it if absent.\nLATTE_WORK_CLAUDE: absolute CLI path override (default auto-detected claude)."
        );
        return Ok(());
    }
    let dir = if args.len() == 3 && args[1] == "--state-dir" {
        PathBuf::from(&args[2])
    } else if args.len() == 1 {
        PathBuf::from(std::env::var_os("HOME").context("HOME missing")?)
            .join(".local/share/latte-work")
    } else {
        bail!("无效参数；使用 --help 查看用法");
    };
    let dir = prepare_dir(&dir)?;
    match args[0].as_str() {
        "serve" => serve(&dir).await,
        "connect" => bridge(&dir).await,
        "connect-local" => match upgrade::connect_local(&dir).await {
            Ok(stream) => forward(stream).await,
            Err(error) => {
                let response = Response::Error {
                    code: "local_upgrade_required".into(),
                    message: error.to_string(),
                };
                let mut bytes = serde_json::to_vec(&response)?;
                bytes.push(b'\n');
                tokio::io::stdout().write_all(&bytes).await?;
                Ok(())
            }
        },
        _ => bail!("未知命令"),
    }
}
fn prepare_dir(dir: &Path) -> Result<PathBuf> {
    if dir.exists() && dir.symlink_metadata()?.file_type().is_symlink() {
        bail!("状态目录不能是符号链接");
    }
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(dir.canonicalize()?)
}
fn lock(dir: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("server.lock"))?;
    file.try_lock().context("此 Host 的 Server 已运行")?;
    Ok(file)
}
async fn bridge(dir: &Path) -> Result<()> {
    forward(connect_socket(dir).await?).await
}
async fn connect_socket(dir: &Path) -> Result<UnixStream> {
    let socket = dir.join("control.sock");
    let stream = match UnixStream::connect(&socket).await {
        Ok(s) => s,
        Err(_) => {
            let mut command = std::process::Command::new(std::env::current_exe()?);
            use std::os::unix::process::CommandExt;
            command
                .arg("serve")
                .arg("--state-dir")
                .arg(dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0);
            let mut daemon = command.spawn().context("无法启动 Host Server")?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
            loop {
                if let Ok(s) = UnixStream::connect(&socket).await {
                    break s;
                }
                if tokio::time::Instant::now() > deadline {
                    bail!("无法连接 Server；请运行 latte-work-server serve 查看诊断");
                }
                // A concurrent bridge may win the singleton lock; still wait for its socket.
                let _ = daemon.try_wait();
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    };
    Ok(stream)
}
async fn forward(stream: UnixStream) -> Result<()> {
    let (mut reader, mut writer) = stream.into_split();
    let mut input = tokio::io::stdin();
    let mut output = tokio::io::stdout();
    tokio::select! {result=tokio::io::copy(&mut input,&mut writer)=>{result?;},result=tokio::io::copy(&mut reader,&mut output)=>{result?;}}
    Ok(())
}
async fn serve(dir: &Path) -> Result<()> {
    let _lock = lock(dir)?;
    let socket = dir.join("control.sock");
    if let Ok(metadata) = socket.symlink_metadata() {
        if !metadata.file_type().is_socket() {
            bail!("control.sock 被非 socket 文件占用");
        }
        std::fs::remove_file(&socket)?;
    }
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
    let listener = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    eprintln!(
        "Latte Work server {} listening at {}",
        service.server_id,
        socket.display()
    );
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            result=listener.accept()=>{let (stream,_)=result?;let service=service.clone();tokio::spawn(async move {if let Err(e)=connection(stream,service).await {eprintln!("client disconnected: {e}");}});},
            _=tokio::signal::ctrl_c()=>break,
        _=terminate.recv()=>break,
        _=service.lifecycle.shutdown.notified()=>break,
        }
    }
    service
        .terminals
        .lock()
        .map_err(|_| anyhow::anyhow!("终端锁异常"))?
        .close_all();
    let controls: Vec<_> = service
        .runs
        .lock()
        .map_err(|_| anyhow::anyhow!("run lock poisoned"))?
        .values()
        .map(|run| run.control.clone())
        .collect();
    for control in controls {
        let _ = control
            .send(Control::Cancel {
                reply: None,
                only_if_idle: false,
            })
            .await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    std::fs::remove_file(socket)?;
    Ok(())
}
async fn connection(stream: UnixStream, service: Service) -> Result<()> {
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
                            agents: vec![service.agent.clone()],
                            permission_settings: true,
                            history_window: true,
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
    // Undo holds exclusive admission until file restoration and its durable result.
    // Ordinary turns retain concurrent admission; no request is replayed.
    let undo = matches!(&request, Request::UndoTurnChanges { .. });
    let _undo_guard = if undo {
        Some(s.file_admission.write().await)
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
    ) {
        Some(s.file_admission.read().await)
    } else {
        None
    };
    let closed_resource = match &request {
        Request::CloseAgentSession { session_id, .. } => {
            Some(upgrade::Resource::AgentSession(session_id.clone()))
        }
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
                modes: agents::permission_modes(&agent, &s.agent_binary).await?,
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
                commands: runtime::commands(&s.agent_binary, &agent, &project.path, &config)
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
            let terminals = s.terminals.clone();
            tokio::task::spawn_blocking(move || {
                terminals
                    .lock()
                    .map_err(|_| anyhow::anyhow!("终端锁异常"))?
                    .create(
                        project_id,
                        Path::new(&project.path),
                        terminal_id,
                        cols,
                        rows,
                    )
            })
            .await??
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
        } => s
            .terminals
            .lock()
            .map_err(|_| anyhow::anyhow!("终端锁异常"))?
            .resize(&terminal_id, cols, rows)?,
        Request::CloseTerminal { terminal_id } => s
            .terminals
            .lock()
            .map_err(|_| anyhow::anyhow!("终端锁异常"))?
            .close(&terminal_id),
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
            session: db(&s.database, |d| d.archive_session(&session_id, archived))?,
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
                s.agent_binary.clone(),
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
        Request::Send {
            provider,
            session_id,
            request_id,
            text,
            model,
            effort,
            permission_mode,
        } => {
            let fingerprint = providers::turn_fingerprint(provider.as_ref())?;
            if db(&s.database, |d| {
                d.already_accepted(
                    &session_id,
                    &request_id,
                    &text,
                    model.as_deref(),
                    effort,
                    fingerprint.as_deref(),
                    permission_mode.as_deref(),
                )
            })? {
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
                if !agents::permission_modes(&agent_id, &s.agent_binary)
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
            let new = runtime::submit(
                s.database.clone(),
                s.runs.clone(),
                s.agent_binary.clone(),
                session,
                path,
                runtime::SendTurn {
                    request_id,
                    text,
                    model,
                    fingerprint,
                    config: launch_config,
                },
            )
            .await?;
            Response::Accepted { duplicate: !new }
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
            Response::Files {
                entries: files::list(Path::new(&p.path), &path)?,
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
                baseline.summary(Path::new(&p.path)).await?
            } else {
                task_changes::workspace_summary(Path::new(&p.path)).await?
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
            turn.validate_undo(root)?;
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
                if root.starts_with(other) || other.starts_with(root) {
                    runtime::close(&s.database, &s.runs, &session.id, true).await?;
                }
            }
            turn.validate_undo(root)?;
            turn.changes.undo = TurnUndoStatus::Unknown;
            db(&s.database, |d| d.update_turn(&session_id, &turn))?;
            turn.restore(root)
                .context("撤销未能全部完成，状态待核对；请检查工作区，系统不会自动重试")?;
            turn.changes.undo = TurnUndoStatus::Reverted;
            db(&s.database, |d| d.update_turn(&session_id, &turn))?;
            Response::TurnChanges {
                changes: turn.changes,
            }
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
            let (text, truncated) = baseline.patch(Path::new(&project.path), &path).await?;
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
            let (text, truncated) = files::read(Path::new(&p.path), &path).await?;
            Response::Content { text, truncated }
        }
        Request::Changes { project_id } => {
            let p = db(&s.database, |d| d.project(&project_id))?;
            let (entries, truncated) = files::changes(Path::new(&p.path)).await?;
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
            Response::GitReview {
                review: git_review::review(Path::new(&p.path), scope, base.as_deref()).await?,
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
            let (text, truncated) = git_review::diff(
                Path::new(&p.path),
                scope,
                base.as_deref(),
                head.as_deref(),
                &path,
                full_context,
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
            let (text, truncated) = files::change_diff(Path::new(&p.path), &path, section).await?;
            Response::Content { text, truncated }
        }
        Request::Diff { project_id } => {
            let p = db(&s.database, |d| d.project(&project_id))?;
            let (text, truncated) = files::diff(Path::new(&p.path)).await?;
            Response::Content { text, truncated }
        }
    };
    if matches!(response, Response::Ok)
        && let Some(resource) = closed_resource
    {
        s.lifecycle.forget(&resource)?;
    }
    runtime::refresh_titles(&s.database, &s.runs, &s.title_refresh, &mut response).await?;
    runtime::project_session_state(&s.runs, &mut response)?;
    Ok(response)
}
