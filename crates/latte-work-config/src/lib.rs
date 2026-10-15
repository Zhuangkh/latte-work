//! App-owned Provider configuration. No Server connection or Agent installation is required.
pub mod discovery;
mod validation;
use anyhow::{Context, Result, bail};
use latte_work_platform::{fs::PrivateDir, lock::FileLock};
use latte_work_protocol::{
    AgentProviderBinding, Provider, ProviderDraft, ProviderSnapshot, Response,
};
use serde::{Deserialize, Serialize};
#[cfg(not(windows))]
use std::fs::File;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
pub use validation::validate_provider;
const MAX_CONFIG: u64 = 128 * 1024;

// Keep schema 2 readable so existing App installations migrate without dropping associations.
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    #[serde(default)]
    synced: bool,
    metadata: Provider,
    credential: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Config {
    schema: u32,
    #[serde(default)]
    bindings: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_id: Option<String>,
    #[serde(default)]
    host_bindings: BTreeMap<String, BTreeMap<String, String>>,
    providers: Vec<Record>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            schema: 2,
            bindings: BTreeMap::new(),
            active_id: None,
            host_bindings: BTreeMap::new(),
            providers: vec![],
        }
    }
}
/// Short-lived transaction: holds an OS lock and reloads configuration for every operation.
/// Intentionally not Debug/Serialize: credentials must never reach the WebView or logs.
pub struct ProviderStore {
    path: PathBuf,
    directory: PrivateDir,
    config: Config,
    _lock: FileLock,
}
fn regular(path: &Path) -> Result<bool> {
    match path.symlink_metadata() {
        Ok(meta) if meta.is_file() && !latte_work_platform::fs::is_link(&meta) => Ok(true),
        Ok(_) => bail!("Provider 配置路径必须是普通文件"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn read_config(path: &Path) -> Result<Config> {
    regular(path)?;
    let mut bytes = vec![];
    #[cfg(windows)]
    let file = latte_work_platform::fs::open_private_file(path)
        .context("Provider 配置权限不安全；保留原文件，请由用户恢复私有权限后重试")?;
    #[cfg(not(windows))]
    let file = File::open(path)?;
    file.take(MAX_CONFIG + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_CONFIG {
        bail!("Provider 配置超出大小限制");
    }
    let mut config: Config =
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("Provider 配置文件格式无效"))?;
    if config.schema == 1 {
        config.schema = 2;
        // Schema 1's global selection only applied to Claude when compatible.
        // Preserve that legacy behavior without consulting any installed Agent.
        if let Some(id) = config.active_id.take()
            && config.providers.iter().any(|record| {
                record.metadata.id == id
                    && record.metadata.protocol
                        == latte_work_protocol::ProviderProtocol::AnthropicMessages
            })
        {
            config.bindings.insert("claude".into(), id);
        }
        for record in &mut config.providers {
            if record.metadata.revision.is_empty() {
                record.metadata.revision = uuid::Uuid::new_v4().to_string();
            }
        }
    }
    validate_config(&config)?;
    Ok(config)
}
impl ProviderStore {
    /// Open App-owned storage without starting a daemon. Windows uses a dedicated
    /// private child so unrelated App files keep their existing permissions.
    /// Import the old App file, then the supplied legacy file, only when the new
    /// configuration is absent. Migration sources are never modified; unsafe
    /// Windows permissions and invalid configuration fail closed before import.
    pub fn open(directory: &Path, legacy: Option<&Path>) -> Result<Self> {
        #[cfg(windows)]
        let old_app_path = directory.join("providers.json");
        #[cfg(windows)]
        let directory = {
            let root = latte_work_platform::fs::prepare_state_dir(directory)?;
            PrivateDir::prepare(&root.join("private-providers"))?
        };
        #[cfg(not(windows))]
        let directory = PrivateDir::prepare(directory)?;
        let lock_path = directory.path().join("providers.lock");
        regular(&lock_path)?;
        let deadline = Instant::now() + Duration::from_secs(2);
        let lock = loop {
            match FileLock::try_acquire(&lock_path) {
                Ok(guard) => break guard,
                Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    bail!("Provider 配置正被其他窗口使用，请重试")
                }
                Err(std::fs::TryLockError::Error(error)) => {
                    return Err(error).context("无法获取 Provider 配置锁");
                }
            }
        };
        directory.verify()?;
        let path = directory.path().join("providers.json");
        let exists = regular(&path)?;
        #[cfg(windows)]
        let legacy = if !exists && regular(&old_app_path)? {
            Some(old_app_path.as_path())
        } else {
            legacy
        };
        let config = if exists {
            read_config(&path)?
        } else if let Some(legacy) = legacy.filter(|p| p != &path) {
            if regular(legacy)? {
                read_config(legacy)?
            } else {
                Config::default()
            }
        } else {
            Config::default()
        };
        let mut store = Self {
            path,
            directory,
            config,
            _lock: lock,
        };
        if !exists {
            store.persist(store.config.clone())?;
        } else {
            store.directory.protect_file("providers.json")?;
        }
        Ok(store)
    }
    fn bindings(&self, host: &str) -> Option<&BTreeMap<String, String>> {
        if host == "local" {
            Some(&self.config.bindings)
        } else {
            self.config.host_bindings.get(host)
        }
    }
    fn catalog<'a>(&self, bindings: impl Iterator<Item = (&'a String, &'a String)>) -> Response {
        Response::Providers {
            providers: self
                .config
                .providers
                .iter()
                .map(|r| r.metadata.clone())
                .collect(),
            bindings: bindings
                .map(|(agent, id)| AgentProviderBinding {
                    agent: agent.clone(),
                    provider_id: id.clone(),
                    provider_revision: self
                        .config
                        .providers
                        .iter()
                        .find(|r| &r.metadata.id == id)
                        .expect("validated binding")
                        .metadata
                        .revision
                        .clone(),
                })
                .collect(),
        }
    }
    /// Includes every association so a Provider in use cannot be offered for deletion.
    pub fn list(&self) -> Response {
        self.catalog(
            self.config
                .bindings
                .iter()
                .chain(self.config.host_bindings.values().flat_map(|b| b.iter())),
        )
    }
    pub fn list_for_host(&self, host: &str) -> Result<Response> {
        validate_id(host)?;
        Ok(self.catalog(self.bindings(host).into_iter().flat_map(|b| b.iter())))
    }
    pub fn snapshot_for_host(&self, host: &str, agent: &str) -> Result<Option<ProviderSnapshot>> {
        validate_id(host)?;
        validate_id(agent)?;
        self.bindings(host)
            .and_then(|b| b.get(agent))
            .map(|id| {
                let record = self
                    .config
                    .providers
                    .iter()
                    .find(|r| &r.metadata.id == id)
                    .context("关联的 Provider 不存在")?;
                Ok(ProviderSnapshot {
                    provider: record.metadata.clone(),
                    credential: record.credential.clone(),
                })
            })
            .transpose()
    }
    pub fn bind(&mut self, host: &str, agent: &str, id: Option<String>) -> Result<Response> {
        validate_id(host)?;
        validate_id(agent)?;
        let mut next = self.config.clone();
        let bindings = if host == "local" {
            &mut next.bindings
        } else {
            next.host_bindings.entry(host.into()).or_default()
        };
        if let Some(id) = id {
            bindings.insert(agent.into(), id);
        } else {
            bindings.remove(agent);
        }
        self.persist(next)?;
        self.list_for_host(host)
    }
    pub fn forget_host(&mut self, host: &str) -> Result<()> {
        validate_id(host)?;
        let mut next = self.config.clone();
        if host == "local" {
            next.bindings.clear();
        } else {
            next.host_bindings.remove(host);
        }
        self.persist(next)?;
        Ok(())
    }
    fn bound(&self, id: &str) -> bool {
        self.config
            .bindings
            .values()
            .chain(self.config.host_bindings.values().flat_map(|b| b.values()))
            .any(|value| value == id)
    }
    pub fn delete(&mut self, id: &str) -> Result<Response> {
        let mut next = self.config.clone();
        next.providers.retain(|r| r.metadata.id != id);
        next.bindings.retain(|_, provider| provider != id);
        for bindings in next.host_bindings.values_mut() {
            bindings.retain(|_, provider| provider != id);
        }
        next.host_bindings
            .retain(|_, bindings| !bindings.is_empty());
        if next.active_id.as_deref() == Some(id) {
            next.active_id = None;
        }
        self.persist(next)
    }
    fn persist(&mut self, config: Config) -> Result<Response> {
        validate_config(&config)?;
        let bytes = serde_json::to_vec_pretty(&config)?;
        if bytes.len() as u64 > MAX_CONFIG {
            bail!("Provider 配置超出大小限制");
        }
        self.directory.verify()?;
        if regular(&self.path)? {
            self.directory.protect_file("providers.json")?;
        }
        let mut file = self.directory.tempfile("providers-", ".json")?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        self.directory.verify()?;
        if regular(&self.path)? {
            self.directory.protect_file("providers.json")?;
        }
        file.persist(&self.path)
            .map_err(|_| anyhow::anyhow!("无法保存 Provider 配置"))?;
        self.config = config;
        Ok(self.list())
    }
    pub fn model_endpoint(&self, draft: ProviderDraft) -> Result<discovery::ModelEndpoint> {
        let old = draft
            .id
            .as_ref()
            .and_then(|id| self.config.providers.iter().find(|p| &p.metadata.id == id));
        let credential = if draft.auth == latte_work_protocol::ProviderAuth::None {
            String::new()
        } else if let Some(value) = draft.credential {
            value
        } else {
            let old = old.context("请填写 API Key")?;
            if old.metadata.base_url.trim_end_matches('/') != draft.base_url.trim_end_matches('/')
                || old.metadata.auth != draft.auth
                || old.metadata.protocol != draft.protocol
            {
                bail!("地址或协议已更改，请重新填写 API Key 后获取模型");
            }
            old.credential.clone()
        };
        discovery::ModelEndpoint::new(draft.base_url, draft.protocol, draft.auth, credential)
    }
    pub fn save(&mut self, mut draft: ProviderDraft) -> Result<Response> {
        draft.name = draft.name.trim().to_owned();
        draft.base_url = draft.base_url.trim().trim_end_matches('/').to_owned();
        draft.model = draft.model.trim().to_owned();
        draft.models = draft
            .models
            .into_iter()
            .map(|m| m.trim().to_owned())
            .filter(|m| !m.is_empty())
            .collect();
        draft.models.sort();
        draft.models.dedup();
        let existing = match &draft.id {
            Some(id) => Some(
                self.config
                    .providers
                    .iter()
                    .find(|r| &r.metadata.id == id)
                    .context("Provider 不存在")?,
            ),
            None => None,
        };
        if let Some(old) = existing
            && draft.auth != latte_work_protocol::ProviderAuth::None
            && draft.credential.is_none()
            && (old.metadata.base_url != draft.base_url || old.metadata.auth != draft.auth)
        {
            bail!("更改地址或认证方式时，请重新填写凭据");
        }
        if let Some(old) = existing
            && old.metadata.protocol != draft.protocol
            && self.bound(&old.metadata.id)
        {
            bail!("请先解除 Provider 关联，再更改接口协议");
        }
        let credential = if draft.auth == latte_work_protocol::ProviderAuth::None {
            String::new()
        } else {
            draft
                .credential
                .take()
                .or_else(|| existing.map(|r| r.credential.clone()))
                .context("请填写 Provider 凭据")?
        };
        let metadata = Provider {
            id: draft.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            name: draft.name,
            protocol: draft.protocol,
            base_url: draft.base_url,
            model: draft.model,
            models: draft.models,
            model_labels: draft.model_labels,
            auth: draft.auth,
            has_credential: !credential.is_empty(),
            revision: uuid::Uuid::new_v4().to_string(),
        };
        validate_provider(&metadata, &credential)?;
        let mut next = self.config.clone();
        let record = Record {
            synced: false,
            metadata,
            credential,
        };
        if let Some(old) = next
            .providers
            .iter_mut()
            .find(|r| r.metadata.id == record.metadata.id)
        {
            *old = record;
        } else {
            next.providers.push(record);
        }
        self.persist(next)
    }
}
fn validate_id(id: &str) -> Result<()> {
    if id.is_empty() || id.len() > 255 || id.chars().any(char::is_control) {
        bail!("主机或 Agent ID 无效");
    }
    Ok(())
}
fn validate_config(config: &Config) -> Result<()> {
    if config.schema != 2 || config.providers.len() > 16 {
        bail!("Provider 配置版本或数量无效（最多 16 个）");
    }
    let mut ids = std::collections::HashSet::new();
    for record in &config.providers {
        validate_provider(&record.metadata, &record.credential)?;
        if !ids.insert(&record.metadata.id) {
            bail!("Provider ID 重复");
        }
    }
    if config.host_bindings.len() > 256 {
        bail!("主机关联数量超出限制");
    }
    for host in config.host_bindings.keys() {
        validate_id(host)?;
        if host == "local" {
            bail!("本机关联应使用 bindings 字段");
        }
    }
    for (agent, id) in config
        .bindings
        .iter()
        .chain(config.host_bindings.values().flat_map(|b| b.iter()))
    {
        validate_id(agent)?;
        if !ids.contains(id) {
            bail!("关联的 Provider 不存在");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
