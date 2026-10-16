//! Explicit quit residual resources: finish local closes, reattach remote sessions.
//! No passwords, prompts or Provider secrets.
use crate::{Connections, Host};
use latte_work_client::Client;
use latte_work_platform::fs::PrivateDir;
use latte_work_platform::lock::FileLock;
use latte_work_protocol::lifecycle::Resource;
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, TryLockError},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use tauri::Manager;

#[derive(Serialize, Deserialize)]
struct Intent {
    owner_id: String,
    hosts: Vec<HostIntent>,
}
#[derive(Serialize, Deserialize)]
struct HostIntent {
    host: Host,
    server_id: String,
    resources: Vec<Resource>,
}
const LIMIT: u64 = 1024 * 1024;
fn directory(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?
        .join("pending-resources"))
}
/// The validated private leaf stays pinned for as long as the lock is held.
struct LockedDir {
    directory: PrivateDir,
    _lock: FileLock,
}
impl LockedDir {
    fn directory(&self) -> &PrivateDir {
        &self.directory
    }
}
fn lock(path: &Path) -> Result<LockedDir, String> {
    // The app config parent is ordinary state; only the leaf is strictly private.
    // Windows PrivateDir creates the leaf alone, so create missing parents first.
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let directory = PrivateDir::prepare(path).map_err(|e| e.to_string())?;
    let lock =
        FileLock::try_acquire(&directory.path().join("pending.lock")).map_err(
            |error| match error {
                TryLockError::WouldBlock => "关闭记录正在处理，请稍后重新连接".to_string(),
                TryLockError::Error(error) => error.to_string(),
            },
        )?;
    Ok(LockedDir {
        directory,
        _lock: lock,
    })
}
fn regular(path: &Path) -> Result<(), String> {
    match path.symlink_metadata() {
        Ok(m) if m.is_file() => Ok(()),
        Ok(_) => Err("关闭记录必须为普通文件".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
fn write(directory: &PrivateDir, intent: &Intent) -> Result<(), String> {
    let owner = uuid::Uuid::parse_str(&intent.owner_id).map_err(|e| e.to_string())?;
    let name = format!("{owner}.json");
    let path = directory.path().join(&name);
    regular(&path)?;
    if intent.hosts.is_empty() {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.to_string()),
        }
    }
    let data = serde_json::to_vec(intent).map_err(|e| e.to_string())?;
    if data.len() as u64 > LIMIT {
        return Err("关闭记录超过大小限制".into());
    }
    let temp = directory
        .tempfile(&format!("{owner}."), ".tmp")
        .map_err(|e| e.to_string())?;
    let result = (|| -> std::io::Result<()> {
        let mut file = temp.as_file();
        file.write_all(&data)?;
        file.sync_all()?;
        temp.persist(&path).map_err(|e| e.error)?;
        #[cfg(unix)]
        File::open(directory.path())?.sync_all()?;
        Ok(())
    })();
    result.map_err(|e| e.to_string())
}
fn read(directory: &PrivateDir, name: &str) -> Result<Intent, String> {
    directory.protect_file(name).map_err(|e| e.to_string())?;
    let path = directory.path().join(name);
    let mut data = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(LIMIT + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() as u64 > LIMIT {
        return Err("关闭记录超过大小限制".into());
    }
    let intent: Intent = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    let owner = uuid::Uuid::parse_str(&intent.owner_id).map_err(|e| e.to_string())?;
    if name != format!("{owner}.json")
        || intent.hosts.len() > 128
        || intent.hosts.iter().any(|h| h.resources.len() > 128)
    {
        return Err("无效关闭记录".into());
    }
    Ok(intent)
}
fn same_target(a: &Host, b: &Host) -> bool {
    a.id == b.id
        && a.ssh == b.ssh
        && a.port == b.port
        && a.server_path == b.server_path
        && a.platform == b.platform
        && a.identity_file == b.identity_file
        && a.auth == b.auth
}
pub(crate) async fn persist(app: &tauri::AppHandle, state: &Connections) -> Result<(), String> {
    let instances = state.owned_instances.lock().await.clone();
    let hosts = state.host_targets.lock().await;
    let mut grouped = std::collections::BTreeMap::<(String, String), Vec<Resource>>::new();
    for ((host, resource), server_id) in instances {
        grouped.entry((host, server_id)).or_default().push(resource);
    }
    let mut intents = Vec::new();
    for ((host, server_id), resources) in grouped {
        let host = hosts.get(&host).cloned().ok_or("缺少关闭记录的主机信息")?;
        intents.push(HostIntent {
            host,
            server_id,
            resources,
        });
    }
    let intent = Intent {
        owner_id: state.owner_id().to_owned(),
        hosts: intents,
    };
    let locked = lock(&directory(app)?)?;
    write(locked.directory(), &intent)
}
/// Resolve local cleanup or remote reattachment before exposing a connection.
pub(crate) async fn drain(
    app: &tauri::AppHandle,
    state: &Connections,
    host: &Host,
    client: &mut Client,
) -> Result<Vec<Resource>, String> {
    let _admission = state.cleanup_admission.lock().await;
    let adopted = drain_directory(&directory(app)?, host, client, state.owner_id()).await?;
    for resource in &adopted {
        state.owned_instances.lock().await.insert(
            (host.id.clone(), resource.clone()),
            client.server_id().to_owned(),
        );
        match resource {
            Resource::AgentSession(id) => {
                state
                    .turns
                    .lock()
                    .await
                    .insert((host.id.clone(), id.clone()), "open".into());
            }
            Resource::Terminal(id) => {
                state
                    .terminals
                    .lock()
                    .await
                    .insert((host.id.clone(), id.clone()));
            }
        }
    }
    Ok(adopted)
}
async fn drain_directory(
    directory: &Path,
    host: &Host,
    client: &mut Client,
    owner_id: &str,
) -> Result<Vec<Resource>, String> {
    let locked = lock(directory)?;
    let directory = locked.directory();
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(directory.path()).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_some_and(|e| e == "json") {
            if paths.len() >= 128 {
                return Err("待关闭记录数量超过限制".into());
            }
            paths.push(path);
        }
    }
    paths.sort();
    let mut adopted = Vec::new();
    for path in paths {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Err("无效关闭记录".into());
        };
        let mut intent = read(directory, name)?;
        for item in intent.hosts.iter().filter(|h| same_target(&h.host, host)) {
            if host.ssh.is_some() {
                adopted.extend(
                    client
                        .adopt_resources(
                            item.server_id.clone(),
                            intent.owner_id.clone(),
                            owner_id.to_owned(),
                            item.resources.clone(),
                        )
                        .await
                        .map_err(|e| format!("{e:#}"))?,
                );
            } else {
                client
                    .close_owned_resources(
                        item.server_id.clone(),
                        intent.owner_id.clone(),
                        item.resources.clone(),
                    )
                    .await
                    .map_err(|e| format!("{e:#}"))?;
            }
        }
        intent.hosts.retain(|h| !same_target(&h.host, host));
        write(directory, &intent)?;
    }
    Ok(adopted)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn host() -> Host {
        serde_json::from_value(serde_json::json!({"id":"local", "name":"本机", "ssh":null}))
            .unwrap()
    }
    fn temporary_path() -> PathBuf {
        // Do not pre-create the leaf: the private directory must be created and
        // validated by lock(), exactly like the first production run.
        std::env::temp_dir().join(format!("lw-pending-{}", uuid::Uuid::new_v4()))
    }
    #[test]
    fn quit_intent_is_private_atomic_and_contains_no_authentication_secret() {
        let directory = temporary_path();
        let locked = lock(&directory).unwrap();
        let intent = Intent {
            owner_id: uuid::Uuid::new_v4().to_string(),
            hosts: vec![HostIntent {
                host: host(),
                server_id: "old-server".into(),
                resources: vec![Resource::AgentSession("session".into())],
            }],
        };
        write(locked.directory(), &intent).unwrap();
        assert_eq!(
            read(locked.directory(), &format!("{}.json", intent.owner_id))
                .unwrap()
                .hosts[0]
                .server_id,
            "old-server"
        );
        let path = locked
            .directory()
            .path()
            .join(format!("{}.json", intent.owner_id));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(
                locked
                    .directory()
                    .path()
                    .metadata()
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        // A second write replaces the record without leaving a temporary file.
        write(locked.directory(), &intent).unwrap();
        assert!(
            std::fs::read_dir(locked.directory().path())
                .unwrap()
                .all(|entry| entry
                    .unwrap()
                    .path()
                    .extension()
                    .is_none_or(|ext| ext != "tmp"))
        );
        assert!(!std::fs::read_to_string(&path).unwrap().contains("password"));
        let mut different = host();
        different.ssh = Some("other-target".into());
        assert!(!same_target(&host(), &different));
        let mut different_platform = host();
        different_platform.platform = latte_work_client::SshPlatform::Windows;
        assert!(!same_target(&host(), &different_platform));
        drop(locked);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn journal_rejects_symlinks_and_mismatched_owner_file() {
        let directory = temporary_path();
        let locked = lock(&directory).unwrap();
        #[cfg(unix)]
        {
            let outside = locked.directory().path().join("outside");
            std::fs::write(&outside, "private").unwrap();
            let path = locked.directory().path().join("alias.json");
            std::os::unix::fs::symlink(&outside, &path).unwrap();
            assert!(read(locked.directory(), "alias.json").is_err());
            std::fs::remove_file(&path).unwrap();
        }
        let path = locked.directory().path().join("alias.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&Intent {
                owner_id: uuid::Uuid::new_v4().to_string(),
                hosts: vec![],
            })
            .unwrap(),
        )
        .unwrap();
        assert!(read(locked.directory(), "alias.json").is_err());
        drop(locked);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn reconnect_closes_local_resources_but_only_adopts_remote_resources() {
        use std::os::unix::fs::PermissionsExt;
        for remote in [false, true] {
            let directory = temporary_path();
            let locked = lock(&directory).unwrap();
            let binary = directory.join("bridge");
            std::fs::write(&binary, r#"#!/usr/bin/env python3
import sys,json,pathlib
base=pathlib.Path(sys.argv[sys.argv.index('--state-dir')+1])
for line in sys.stdin:
    request=json.loads(line)
    with (base/'requests').open('a') as f: f.write(request['method']+'\n')
    if request['method']=='hello': response={'kind':'hello','version':1,'server_id':'fixture','agents':[]}
    elif request['method']=='adopt_resources': response={'kind':'resources_adopted','resources':request['resources']}
    elif request['method']=='close_owned_resources': response={'kind':'resources_closed'}
    else: response={'kind':'error','code':'unexpected','message':'No Open or Send allowed'}
    print(json.dumps(response),flush=True)
"#).unwrap();
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
            let mut host = host();
            if remote {
                host.id = "remote".into();
                host.ssh = Some("devbox".into());
            }
            let owner = uuid::Uuid::new_v4().to_string();
            let resource = Resource::AgentSession("session".into());
            write(
                locked.directory(),
                &Intent {
                    owner_id: owner.clone(),
                    hosts: vec![HostIntent {
                        host: host.clone(),
                        server_id: "fixture".into(),
                        resources: vec![resource.clone()],
                    }],
                },
            )
            .unwrap();
            drop(locked);
            let mut client = Client::local(&binary, Some(&directory)).await.unwrap();
            let adopted = drain_directory(&directory, &host, &mut client, "new-app")
                .await
                .unwrap();
            assert_eq!(adopted, if remote { vec![resource] } else { vec![] });
            assert!(!directory.join(format!("{owner}.json")).exists());
            assert_eq!(
                std::fs::read_to_string(directory.join("requests")).unwrap(),
                if remote {
                    "hello\nadopt_resources\n"
                } else {
                    "hello\nclose_owned_resources\n"
                }
            );
            drop(client);
            std::fs::remove_dir_all(directory).unwrap();
        }
    }
}
