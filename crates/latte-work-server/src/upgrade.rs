//! Local lifecycle handshake, intentionally independent of the application wire version.
//! SSH continues to use `connect`, which never replaces a running daemon.
use crate::{Service, connect_server, db};
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use latte_work_platform::ipc::{Endpoint, Stream};
pub use latte_work_protocol::lifecycle::{Request, Resource, Response};
use sha2::{Digest, Sha256};
use std::{io::Read, time::Duration};
use tokio::{
    io::AsyncWriteExt,
    sync::{Notify, RwLock},
};
use tokio_util::codec::{FramedRead, LinesCodec};

pub struct Lifecycle {
    build_id: String,
    pub draining: RwLock<bool>,
    owners: std::sync::Mutex<std::collections::HashMap<Resource, String>>,
    pub shutdown: Notify,
}
impl Lifecycle {
    pub fn new() -> Result<Self> {
        Ok(Self {
            build_id: executable_id()?,
            draining: RwLock::new(false),
            owners: std::sync::Mutex::new(std::collections::HashMap::new()),
            shutdown: Notify::new(),
        })
    }
}
impl Lifecycle {
    pub fn forget(&self, resource: &Resource) -> Result<()> {
        self.owners
            .lock()
            .map_err(|_| anyhow::anyhow!("资源归属锁异常"))?
            .remove(resource);
        Ok(())
    }
}
fn executable_id() -> Result<String> {
    let mut file = std::fs::File::open(std::env::current_exe()?)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub async fn handle(service: &Service, request: Request) -> Response {
    match handle_inner(service, request).await {
        Ok(response) => response,
        Err(error) => Response::Error {
            code: "upgrade_blocked".into(),
            message: error.to_string(),
        },
    }
}
async fn handle_inner(service: &Service, request: Request) -> Result<Response> {
    match request {
        Request::ClaimResources {
            owner_id,
            resources,
        } => {
            validate_resources(&owner_id, &resources)?;
            let draining = service.lifecycle.draining.read().await;
            if *draining {
                bail!("后台正在停止，请重新连接");
            }
            let mut owners = service
                .lifecycle
                .owners
                .lock()
                .map_err(|_| anyhow::anyhow!("资源归属锁异常"))?;
            let new = resources
                .iter()
                .filter(|r| !owners.contains_key(*r))
                .count();
            if owners.len() + new > 4096 {
                bail!("会话资源归属数量已达上限");
            }
            for resource in resources {
                owners.insert(resource, owner_id.clone());
            }
            Ok(Response::ResourcesClaimed)
        }
        Request::AdoptResources {
            server_id,
            previous_owner_id,
            owner_id,
            resources,
        } => {
            validate_resources(&owner_id, &resources)?;
            if previous_owner_id.is_empty() || previous_owner_id.len() > 128 {
                bail!("无效的资源归属");
            }
            if server_id != service.server_id {
                return Ok(Response::ResourcesAdopted { resources: vec![] });
            }
            let _admission = service.lifecycle.draining.read().await;
            let runs = service
                .runs
                .lock()
                .map_err(|_| anyhow::anyhow!("执行状态锁异常"))?;
            let terminals = service
                .terminals
                .lock()
                .map_err(|_| anyhow::anyhow!("终端锁异常"))?;
            let mut owners = service
                .lifecycle
                .owners
                .lock()
                .map_err(|_| anyhow::anyhow!("资源归属锁异常"))?;
            let mut adopted = Vec::new();
            for resource in resources {
                let live = match &resource {
                    Resource::AgentSession(id) => runs.get(id).is_some_and(|r| {
                        !r.control.is_closed()
                            && (r.session_open.load(std::sync::atomic::Ordering::SeqCst)
                                || !r.safe_to_close.load(std::sync::atomic::Ordering::SeqCst))
                    }),
                    Resource::Terminal(id) => terminals.is_open(id),
                };
                if live
                    && owners
                        .get(&resource)
                        .is_some_and(|id| id == &previous_owner_id || id == &owner_id)
                {
                    owners.insert(resource.clone(), owner_id.clone());
                    adopted.push(resource);
                }
            }
            Ok(Response::ResourcesAdopted { resources: adopted })
        }
        Request::CloseOwnedResources {
            server_id,
            owner_id,
            mut resources,
        } => {
            validate_resources(&owner_id, &resources)?;
            // A restarted daemon has already lost these ephemeral resources. Never
            // apply an old close to a new server or a resource reclaimed by another App.
            if server_id != service.server_id {
                return Ok(Response::ResourcesClosed);
            }
            let _admission =
                tokio::time::timeout(Duration::from_secs(2), service.lifecycle.draining.write())
                    .await
                    .context("后台正在处理请求")?;
            // Each actor must receive at most one close even when the batch repeats it.
            let mut seen = std::collections::HashSet::new();
            resources.retain(|resource| seen.insert(resource.clone()));
            let results = futures_util::future::join_all(resources.into_iter().map(|resource| {
                let owner_id = &owner_id;
                async move {
                    let owned = service
                        .lifecycle
                        .owners
                        .lock()
                        .map_err(|_| anyhow::anyhow!("资源归属锁异常"))?
                        .get(&resource)
                        .is_some_and(|id| id == owner_id);
                    if !owned {
                        return Ok::<_, anyhow::Error>(());
                    }
                    match &resource {
                        Resource::AgentSession(id) => {
                            let exists = service
                                .runs
                                .lock()
                                .map_err(|_| anyhow::anyhow!("执行状态锁异常"))?
                                .contains_key(id);
                            if exists {
                                crate::runtime::close(&service.database, &service.runs, id, false)
                                    .await?;
                            }
                        }
                        Resource::Terminal(id) => {
                            crate::terminal::close(&service.terminals, id).await?;
                        }
                    }
                    service
                        .lifecycle
                        .owners
                        .lock()
                        .map_err(|_| anyhow::anyhow!("资源归属锁异常"))?
                        .remove(&resource);
                    Ok(())
                }
            }))
            .await;
            for result in results {
                result?;
            }
            Ok(Response::ResourcesClosed)
        }
        Request::ServerStatus => Ok(Response::ServerStatus {
            server_id: service.server_id.clone(),
            build_id: service.lifecycle.build_id.clone(),
            draining: *service.lifecycle.draining.read().await,
        }),
        Request::PrepareUpgrade { server_id } => {
            if server_id != service.server_id {
                bail!("后台实例已变化，请重新连接");
            }
            let mut draining =
                tokio::time::timeout(Duration::from_secs(2), service.lifecycle.draining.write())
                    .await
                    .context("后台正在处理请求，请稍后重新连接以更新")?;
            let running = service
                .runs
                .lock()
                .map_err(|_| anyhow::anyhow!("run lock poisoned"))?
                .values()
                .any(|run| !run.safe_to_close.load(std::sync::atomic::Ordering::SeqCst));
            let terminals = !service
                .terminals
                .lock()
                .map_err(|_| anyhow::anyhow!("终端锁异常"))?
                .is_empty();
            if running || terminals || db(&service.database, |d| d.has_active_sessions())? {
                bail!(
                    "本机后台需要更新，但仍有任务或终端；请等待任务完成并关闭终端后重新连接。当前任务不会中断"
                );
            }
            *draining = true;
            Ok(Response::UpgradeReady)
        }
    }
}
fn validate_resources(owner: &str, resources: &[Resource]) -> Result<()> {
    if owner.is_empty()
        || owner.len() > 128
        || resources.len() > 128
        || resources
            .iter()
            .any(|r| r.id().is_empty() || r.id().len() > 256)
    {
        bail!("无效的资源关闭请求");
    }
    Ok(())
}
async fn exchange(stream: Stream, request: &Request) -> Result<Response> {
    tokio::time::timeout(Duration::from_secs(3), async {
        let (reader, mut writer) = stream.into_split();
        let mut bytes = serde_json::to_vec(request)?;
        bytes.push(b'\n');
        writer.write_all(&bytes).await?;
        let mut lines = FramedRead::new(reader, LinesCodec::new_with_max_length(4096));
        let line = lines.next().await.context("后台连接关闭")??;
        Ok::<_, anyhow::Error>(serde_json::from_str(&line)?)
    })
    .await
    .context("后台升级检查超时")?
}
// A daemon can close the first control connection while a desktop is reopening.
// Retry only the read-only status query; never retry upgrade admission or user work.
async fn server_status(endpoint: &Endpoint) -> Result<Response> {
    for attempt in 0..3 {
        // Transport/security failure is not a reason to retry daemon startup.
        let stream = connect_server(endpoint).await?;
        let result = exchange(stream, &Request::ServerStatus).await;
        match result {
            Ok(response) => return Ok(response),
            Err(error) if attempt == 2 => return Err(error),
            Err(_) => tokio::time::sleep(Duration::from_millis(150)).await,
        }
    }
    unreachable!("bounded status attempts return on the last iteration")
}
const LEGACY: &str = "检测到旧版本机后台，无法安全确认其任务状态；请先在旧版应用中完成任务并关闭终端，再手动重启本机 Server，然后重新连接。仅重启桌面应用不会停止后台";

/// Serialize local coordinators, ask the owning daemon to drain atomically, then wait for
/// its singleton lock. Never kill a daemon, remove its socket, or replay application requests.
pub async fn connect_local(endpoint: &Endpoint) -> Result<Stream> {
    tokio::time::timeout(Duration::from_secs(20), connect_local_inner(endpoint))
        .await
        .context("本机后台切换超时，请稍后重新连接；任务不会自动重发")?
}
async fn connect_local_inner(endpoint: &Endpoint) -> Result<Stream> {
    let _coordinator = loop {
        match endpoint.try_lock_upgrade() {
            Ok(guard) => break guard,
            Err(std::fs::TryLockError::WouldBlock) => {
                tokio::time::sleep(Duration::from_millis(50)).await
            }
            Err(error) => return Err(error.into()),
        }
    };
    let build_id = executable_id()?;
    match server_status(endpoint)
        .await
        .context("无法确认本机后台版本，请稍后重新连接；后台未被停止")?
    {
        Response::ServerStatus {
            build_id: current,
            draining: false,
            ..
        } if current == build_id => {}
        Response::ServerStatus {
            server_id,
            draining,
            ..
        } => {
            if !draining {
                let stream = endpoint.connect(Duration::from_secs(3)).await?;
                match exchange(stream, &Request::PrepareUpgrade { server_id }).await? {
                    Response::UpgradeReady => {}
                    Response::Error { message, .. } => bail!("{message}"),
                    _ => bail!("后台升级响应不兼容，请重新连接"),
                }
            }
            // Listener and all accepted streams release before the singleton lease.
            loop {
                match endpoint.try_lock_server() {
                    Ok(guard) => {
                        drop(guard);
                        break;
                    }
                    Err(std::fs::TryLockError::WouldBlock) => {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            let stream = connect_server(endpoint).await?;
            match exchange(stream, &Request::ServerStatus).await? {
                Response::ServerStatus {
                    build_id: current,
                    draining: false,
                    ..
                } if current == build_id => {}
                _ => bail!("另一个版本的后台已启动，请关闭其他版本应用后重新连接"),
            }
        }
        _ => bail!("{LEGACY}"),
    }
    // Return this exact, verified instance's socket. Recheck after reconnect because a
    // concurrent external shutdown/start may otherwise attach us to a different build.
    let mut stream = endpoint.connect(Duration::from_secs(3)).await?;
    let mut bytes = serde_json::to_vec(&Request::ServerStatus)?;
    bytes.push(b'\n');
    stream.write_all(&bytes).await?;
    let mut response = Vec::new();
    // Read exactly one line; do not buffer and discard any subsequent application bytes.
    use tokio::io::AsyncReadExt;
    loop {
        let byte = stream.read_u8().await?;
        if byte == b'\n' {
            break;
        }
        if response.len() >= 4096 {
            bail!("后台状态响应过大");
        }
        response.push(byte);
    }
    match serde_json::from_slice::<Response>(&response)? {
        Response::ServerStatus {
            build_id: current,
            draining: false,
            ..
        } if current == build_id => Ok(stream),
        _ => bail!("后台实例已变化，请重新连接"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    #[tokio::test]
    async fn status_retries_a_closed_connection_without_requesting_upgrade() {
        let directory = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::prepare(directory.path()).unwrap();
        let listener = endpoint.bind().await.unwrap();
        let service = tokio::spawn(async move {
            for attempt in 0..2 {
                let mut stream = listener.accept().await.unwrap();
                let mut bytes = vec![];
                loop {
                    let byte = stream.read_u8().await.unwrap();
                    if byte == b'\n' {
                        break;
                    }
                    bytes.push(byte);
                }
                assert!(matches!(
                    serde_json::from_slice::<Request>(&bytes).unwrap(),
                    Request::ServerStatus
                ));
                if attempt == 1 {
                    let mut response = serde_json::to_vec(&Response::ServerStatus {
                        server_id: "fixture".into(),
                        build_id: "build".into(),
                        draining: false,
                    })
                    .unwrap();
                    response.push(b'\n');
                    stream.write_all(&response).await.unwrap();
                }
            }
        });
        let response = tokio::time::timeout(Duration::from_secs(2), server_status(&endpoint))
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            response,
            Response::ServerStatus {
                draining: false,
                ..
            }
        ));
        service.await.unwrap();
    }
}
