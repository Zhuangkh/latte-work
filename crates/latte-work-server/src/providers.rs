//! Host-owned provider configuration, independent of agent-native launch protocols.
use anyhow::{Context, Result, bail};
use latte_work_config::validate_provider as validate;
use latte_work_platform::fs::PrivateDir;
use latte_work_protocol::{
    AgentProviderBinding, Provider, ProviderDraft, ProviderSnapshot, Response, TurnProvider,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::{
    io::Write,
    path::{Path, PathBuf},
};

const MAX_PROVIDERS: usize = 16;
const MAX_CONFIG: usize = 128 * 1024;

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
// Intentionally no Debug/Serialize: launch snapshots contain a credential.
#[derive(Clone)]
pub struct LaunchConfig {
    pub provider: Option<ResolvedProvider>,
    pub settings_dir: PathBuf,
    pub model: Option<String>,
    pub effort: Option<latte_work_protocol::Effort>,
    pub permission_mode: Option<String>,
}
#[derive(Clone)]
pub struct ResolvedProvider {
    pub metadata: Provider,
    pub credential: String,
}
impl LaunchConfig {
    pub fn redact(&self, text: &str) -> String {
        match &self.provider {
            Some(p) => text.replace(&p.credential, "[redacted]"),
            None => text.to_owned(),
        }
    }
    pub fn redact_event(
        &self,
        event: latte_work_protocol::EventKind,
    ) -> Result<latte_work_protocol::EventKind> {
        fn visit(value: &mut serde_json::Value, config: &LaunchConfig) {
            match value {
                serde_json::Value::String(text) => *text = config.redact(text),
                serde_json::Value::Array(items) => items.iter_mut().for_each(|v| visit(v, config)),
                serde_json::Value::Object(fields) => {
                    fields.values_mut().for_each(|v| visit(v, config))
                }
                _ => {}
            }
        }
        let mut value = serde_json::to_value(event)?;
        visit(&mut value, self);
        Ok(serde_json::from_value(value)?)
    }
}
pub struct ProviderStore {
    directory: PrivateDir,
    path: PathBuf,
    config: Config,
}
impl ProviderStore {
    pub fn open(directory: &Path) -> Result<Self> {
        let directory = PrivateDir::prepare(&crate::host_paths::provider_directory(directory)?)?;
        let path = directory.path().join("providers.json");
        let mut config = if path.exists() {
            let metadata = path.symlink_metadata()?;
            if !metadata.is_file() || metadata.len() > MAX_CONFIG as u64 {
                bail!("Provider 配置文件无效");
            }
            directory.protect_file("providers.json")?;
            serde_json::from_slice::<Config>(&std::fs::read(&path)?)
                .map_err(|_| anyhow::anyhow!("Provider 配置文件格式无效"))?
        } else {
            Config::default()
        };
        if config.schema == 1 {
            config.schema = 2;
            if let Some(id) = config.active_id.take()
                && config
                    .providers
                    .iter()
                    .any(|r| r.metadata.id == id && compatible("claude", &r.metadata).is_ok())
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
        Ok(Self {
            directory,
            path,
            config,
        })
    }
    pub fn list(&self) -> Response {
        self.list_bindings(&self.config.bindings)
    }
    fn list_bindings(&self, bindings: &BTreeMap<String, String>) -> Response {
        Response::Providers {
            providers: self
                .config
                .providers
                .iter()
                .map(|r| r.metadata.clone())
                .collect(),
            bindings: bindings
                .iter()
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
    pub fn list_for_host(&self, host_id: &str) -> Result<Response> {
        validate_host_id(host_id)?;
        Ok(self.list_bindings(
            self.config
                .host_bindings
                .get(host_id)
                .unwrap_or(&BTreeMap::new()),
        ))
    }
    pub fn bind_host(
        &mut self,
        host_id: &str,
        agent: &str,
        id: Option<String>,
    ) -> Result<Response> {
        validate_host_id(host_id)?;
        if crate::agents::provider_protocols(agent).is_none() {
            bail!("Agent 尚未实现");
        }
        if let Some(id) = &id {
            self.snapshot(agent, id)?;
        }
        let mut next = self.config.clone();
        let bindings = next.host_bindings.entry(host_id.into()).or_default();
        if let Some(id) = id {
            bindings.insert(agent.into(), id);
        } else {
            bindings.remove(agent);
        }
        self.persist(next)?;
        self.list_for_host(host_id)
    }
    pub fn host_configured(&self, host_id: &str) -> bool {
        self.config.host_bindings.contains_key(host_id)
    }
    pub fn forget_host(&mut self, host_id: &str) -> Result<Response> {
        validate_host_id(host_id)?;
        let mut next = self.config.clone();
        next.host_bindings.remove(host_id);
        self.persist(next)
    }
    pub fn snapshot_for_host(
        &self,
        host_id: &str,
        agent: &str,
    ) -> Result<Option<ProviderSnapshot>> {
        validate_host_id(host_id)?;
        if crate::agents::provider_protocols(agent).is_none() {
            bail!("Agent 尚未实现");
        }
        self.config
            .host_bindings
            .get(host_id)
            .and_then(|bindings| bindings.get(agent))
            .map(|id| self.snapshot(agent, id))
            .transpose()
    }
    fn persist(&mut self, config: Config) -> Result<Response> {
        validate_config(&config)?;
        let bytes = serde_json::to_vec_pretty(&config)?;
        if bytes.len() > MAX_CONFIG {
            bail!("Provider 配置超出大小限制");
        }
        let mut file = self.directory.tempfile("providers-", ".json")?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(&self.path)
            .map_err(|_| anyhow::anyhow!("无法保存 Provider 配置"))?;
        self.config = config;
        Ok(self.list())
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
            && draft.credential.is_none()
            && (old.metadata.base_url != draft.base_url || old.metadata.auth != draft.auth)
        {
            bail!("更改地址或认证方式时，请重新填写凭据");
        }
        let credential = draft
            .credential
            .take()
            .or_else(|| existing.map(|r| r.credential.clone()))
            .context("请填写 Provider 凭据")?;
        let metadata = Provider {
            id: draft.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            name: draft.name,
            protocol: draft.protocol,
            base_url: draft.base_url,
            model: draft.model,
            models: draft.models,
            model_labels: draft.model_labels,
            auth: draft.auth,
            has_credential: true,
            revision: uuid::Uuid::new_v4().to_string(),
        };
        validate(&metadata, &credential)?;
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
    pub fn bind(&mut self, agent: &str, id: Option<String>) -> Result<Response> {
        if crate::agents::provider_protocols(agent).is_none() {
            bail!("Agent 尚未实现");
        }
        let mut next = self.config.clone();
        if let Some(id) = id {
            next.bindings.insert(agent.into(), id);
        } else {
            next.bindings.remove(agent);
        }
        next.providers.retain(|r| {
            !r.synced
                || next
                    .bindings
                    .values()
                    .chain(next.host_bindings.values().flat_map(|b| b.values()))
                    .any(|id| id == &r.metadata.id)
        });
        self.persist(next)
    }
    pub fn snapshot(&self, agent: &str, id: &str) -> Result<ProviderSnapshot> {
        let record = self
            .config
            .providers
            .iter()
            .find(|r| r.metadata.id == id)
            .context("Provider 不存在")?;
        compatible(agent, &record.metadata)?;
        Ok(ProviderSnapshot {
            provider: record.metadata.clone(),
            credential: record.credential.clone(),
        })
    }
    /// Atomically install the selected snapshot and its agent binding on the receiving host.
    pub fn sync(&mut self, agent: &str, snapshot: Option<ProviderSnapshot>) -> Result<Response> {
        let Some(snapshot) = snapshot else {
            return self.bind(agent, None);
        };
        compatible(agent, &snapshot.provider)?;
        let mut next = self.config.clone();
        let id = snapshot.provider.id.clone();
        next.providers.retain(|r| r.metadata.id != id);
        next.providers.push(Record {
            synced: true,
            metadata: snapshot.provider,
            credential: snapshot.credential,
        });
        next.bindings.insert(agent.into(), id);
        next.providers.retain(|r| {
            !r.synced
                || next
                    .bindings
                    .values()
                    .chain(next.host_bindings.values().flat_map(|b| b.values()))
                    .any(|id| id == &r.metadata.id)
        });
        self.persist(next)
    }
    pub fn delete(&mut self, id: &str) -> Result<Response> {
        if self
            .config
            .bindings
            .values()
            .chain(self.config.host_bindings.values().flat_map(|b| b.values()))
            .any(|bound| bound == id)
        {
            bail!("请先在 Code Agent 设置中解除所有主机的关联，再删除此 Provider");
        }
        let mut next = self.config.clone();
        let length = next.providers.len();
        next.providers.retain(|r| r.metadata.id != id);
        if next.providers.len() == length {
            bail!("Provider 不存在");
        }
        self.persist(next)
    }
    pub fn models(&self, agent: &str, model: Option<&str>) -> Result<Response> {
        let config = self.launch_config(agent);
        Self::models_for_provider(agent, model, config.provider.map(|p| p.metadata))
    }
    pub fn models_for_provider(
        agent: &str,
        model: Option<&str>,
        provider: Option<Provider>,
    ) -> Result<Response> {
        if crate::agents::provider_protocols(agent).is_none() {
            bail!("Agent 尚未实现");
        }
        if let Some(p) = &provider {
            compatible(agent, p)?;
            // The catalog carries metadata only; validate it using a non-secret placeholder.
            validate(
                p,
                if p.auth == latte_work_protocol::ProviderAuth::None {
                    ""
                } else {
                    "metadata-only"
                },
            )?;
        }
        let effort_levels = crate::agents::effort_levels(
            agent,
            model.or_else(|| provider.as_ref().map(|p| p.model.as_str())),
        )
        .to_vec();
        if let Some(p) = provider {
            let mut models = vec![p.model.clone()];
            for model in p.models {
                if !models.contains(&model) {
                    models.push(model);
                }
            }
            Ok(Response::Models {
                models,
                provider: Some(p.name),
                default_model: Some(p.model),
                effort_levels,
                model_labels: p.model_labels,
            })
        } else {
            Ok(Response::Models {
                models: crate::agents::model_aliases(agent)
                    .iter()
                    .map(|m| (*m).into())
                    .collect(),
                provider: None,
                default_model: None,
                effort_levels,
                model_labels: BTreeMap::new(),
            })
        }
    }
    pub fn selected_config(&self, agent: &str, model: Option<&str>) -> Result<LaunchConfig> {
        let Response::Models { models, .. } = self.models(agent, model)? else {
            unreachable!()
        };
        if model.is_some_and(|m| !models.iter().any(|value| value == m)) {
            bail!("模型不在当前 Agent 的可选列表中，请刷新模型列表");
        }
        let mut config = self.launch_config(agent);
        config.model = model.map(str::to_owned);
        Ok(config)
    }
    pub fn open_config(
        &self,
        agent: &str,
        historical_model: Option<&str>,
        provider: Option<TurnProvider>,
    ) -> Result<LaunchConfig> {
        let metadata = match &provider {
            Some(TurnProvider::Snapshot(snapshot)) => Some(snapshot.provider.clone()),
            Some(TurnProvider::Cli) => None,
            None => self.launch_config(agent).provider.map(|p| p.metadata),
        };
        let Response::Models {
            models,
            default_model,
            ..
        } = Self::models_for_provider(agent, historical_model, metadata)?
        else {
            unreachable!()
        };
        let model = historical_model.filter(|model| models.iter().any(|value| value == model));
        self.turn_config(agent, model.or(default_model.as_deref()), provider)
    }
    pub fn turn_config(
        &self,
        agent: &str,
        model: Option<&str>,
        provider: Option<TurnProvider>,
    ) -> Result<LaunchConfig> {
        let Some(provider) = provider else {
            return self.selected_config(agent, model);
        };
        let mut config = self.launch_config(agent);
        config.provider = match provider {
            TurnProvider::Cli => None,
            TurnProvider::Snapshot(snapshot) => {
                compatible(agent, &snapshot.provider)?;
                validate(&snapshot.provider, &snapshot.credential)?;
                Some(ResolvedProvider {
                    metadata: snapshot.provider,
                    credential: snapshot.credential,
                })
            }
        };
        let Response::Models { models, .. } = Self::models_for_provider(
            agent,
            model,
            config.provider.as_ref().map(|p| p.metadata.clone()),
        )?
        else {
            unreachable!()
        };
        if model.is_some_and(|m| !models.iter().any(|value| value == m)) {
            bail!("模型不在当前 Provider 的可选列表中，请刷新模型列表");
        }
        config.model = model.map(str::to_owned);
        Ok(config)
    }
    pub fn launch_config(&self, agent: &str) -> LaunchConfig {
        LaunchConfig {
            effort: None,
            permission_mode: None,
            model: None,
            settings_dir: self.directory.path().to_owned(),
            provider: self
                .config
                .bindings
                .get(agent)
                .and_then(|id| self.config.providers.iter().find(|p| &p.metadata.id == id))
                .map(|p| ResolvedProvider {
                    metadata: p.metadata.clone(),
                    credential: p.credential.clone(),
                }),
        }
    }
}
fn validate_config(config: &Config) -> Result<()> {
    if config.schema != 2 || config.providers.len() > MAX_PROVIDERS {
        bail!("Provider 配置版本或数量无效（最多 16 个）");
    }
    let mut ids = std::collections::HashSet::new();
    for record in &config.providers {
        validate(&record.metadata, &record.credential)?;
        if !ids.insert(&record.metadata.id) {
            bail!("Provider ID 重复");
        }
    }
    if config.host_bindings.len() > 256 {
        bail!("主机关联数量超出限制");
    }
    for host_id in config.host_bindings.keys() {
        validate_host_id(host_id)?;
    }
    for (agent, id) in config
        .bindings
        .iter()
        .chain(config.host_bindings.values().flat_map(|b| b.iter()))
    {
        let provider = config
            .providers
            .iter()
            .find(|r| &r.metadata.id == id)
            .context("关联的 Provider 不存在")?;
        compatible(agent, &provider.metadata)?;
    }
    Ok(())
}
fn validate_host_id(host_id: &str) -> Result<()> {
    if host_id.is_empty()
        || host_id == "local"
        || host_id.len() > 255
        || host_id.chars().any(char::is_control)
    {
        bail!("远程主机 ID 无效");
    }
    Ok(())
}
/// Only a digest reaches the durable request ledger; never persist a snapshot or credential.
pub fn turn_fingerprint(provider: Option<&TurnProvider>) -> Result<Option<String>> {
    use sha2::{Digest, Sha256};
    provider
        .map(|value| Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?))))
        .transpose()
}
fn compatible(agent: &str, provider: &Provider) -> Result<()> {
    let protocols = crate::agents::provider_protocols(agent).context("Agent 尚未实现")?;
    if !protocols.contains(&provider.protocol) {
        bail!("此 Code Agent 不支持所选 Provider 的协议");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use latte_work_protocol::{ProviderAuth, ProviderProtocol};
    #[test]
    fn display_names_do_not_change_model_ids_in_no_auth_catalog() {
        let provider: Provider = serde_json::from_value(serde_json::json!({
            "id":"p", "name":"Local", "base_url":"http://localhost:8000", "protocol":"anthropic_messages",
            "model":"real-id", "models":["other"], "model_labels":{"real-id":"Friendly"},
            "auth":"none", "has_credential":false
        })).unwrap();
        let Response::Models {
            models,
            model_labels,
            default_model,
            ..
        } = ProviderStore::models_for_provider("claude", None, Some(provider)).unwrap()
        else {
            panic!()
        };
        assert_eq!(models, vec!["real-id", "other"]);
        assert_eq!(default_model.as_deref(), Some("real-id"));
        assert_eq!(model_labels["real-id"], "Friendly");
    }
    #[test]
    fn opening_history_uses_current_provider_without_relaxing_send_validation() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProviderStore::open(dir.path()).unwrap();
        let id = saved(&mut store, draft(ProviderProtocol::AnthropicMessages));
        store.bind("claude", Some(id.clone())).unwrap();
        let snapshot = Some(store.snapshot("claude", &id).unwrap());
        let open = store
            .open_config(
                "claude",
                Some("opus"),
                snapshot.clone().map(TurnProvider::Snapshot),
            )
            .unwrap();
        assert_eq!(open.model.as_deref(), Some("first"));
        assert!(
            store
                .turn_config("claude", Some("opus"), snapshot.map(TurnProvider::Snapshot))
                .is_err()
        );
        assert_eq!(
            store
                .open_config("claude", Some("first"), None)
                .unwrap()
                .model
                .as_deref(),
            Some("first")
        );
    }
    fn draft(protocol: ProviderProtocol) -> ProviderDraft {
        ProviderDraft {
            id: None,
            name: "Team".into(),
            protocol,
            base_url: "https://example.test".into(),
            model: "first".into(),
            models: vec![],
            model_labels: Default::default(),
            auth: ProviderAuth::ApiKey,
            credential: Some("fixture-private-key".into()),
        }
    }
    fn saved(store: &mut ProviderStore, draft: ProviderDraft) -> String {
        match store.save(draft).unwrap() {
            Response::Providers { providers, .. } => providers.last().unwrap().id.clone(),
            _ => unreachable!(),
        }
    }
    #[test]
    fn remote_associations_are_local_durable_and_validate_edits() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProviderStore::open(dir.path()).unwrap();
        let id = saved(&mut store, draft(ProviderProtocol::AnthropicMessages));
        store
            .bind_host("remote-a", "claude", Some(id.clone()))
            .unwrap();
        assert!(
            store
                .snapshot_for_host("remote-b", "claude")
                .unwrap()
                .is_none()
        );
        assert!(store.snapshot_for_host("remote-a", "codex").is_err());
        assert!(store.launch_config("claude").provider.is_none());
        assert!(store.delete(&id).is_err());
        let first = store
            .snapshot_for_host("remote-a", "claude")
            .unwrap()
            .unwrap();
        let mut edit = draft(ProviderProtocol::OpenaiChat);
        edit.id = Some(id.clone());
        assert!(store.save(edit.clone()).is_err());
        edit.protocol = ProviderProtocol::AnthropicMessages;
        edit.model = "second".into();
        store.save(edit).unwrap();
        let mut reopened = ProviderStore::open(dir.path()).unwrap();
        let current = reopened
            .snapshot_for_host("remote-a", "claude")
            .unwrap()
            .unwrap();
        assert_eq!(first.provider.model, "first");
        assert_eq!(current.provider.model, "second");
        assert!(
            !serde_json::to_string(&reopened.list_for_host("remote-a").unwrap())
                .unwrap()
                .contains("fixture-private-key")
        );
        reopened.bind_host("remote-a", "claude", None).unwrap();
        reopened.delete(&id).unwrap();
        let mut reopened = ProviderStore::open(dir.path()).unwrap();
        assert!(reopened.host_configured("remote-a"));
        assert!(
            reopened
                .snapshot_for_host("remote-a", "claude")
                .unwrap()
                .is_none()
        );
        reopened.forget_host("remote-a").unwrap();
        assert!(
            !ProviderStore::open(dir.path())
                .unwrap()
                .host_configured("remote-a")
        );
    }
    #[test]
    fn turn_overrides_ignore_saved_bindings_and_never_persist() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProviderStore::open(dir.path()).unwrap();
        let id = saved(&mut store, draft(ProviderProtocol::AnthropicMessages));
        store.bind("claude", Some(id.clone())).unwrap();
        let before = std::fs::read(&store.path).unwrap();
        assert!(
            store
                .turn_config("claude", None, Some(TurnProvider::Cli))
                .unwrap()
                .provider
                .is_none()
        );
        let original = store.snapshot("claude", &id).unwrap();
        let mut changed = original.clone();
        changed.provider.model = "temporary".into();
        let config = store
            .turn_config(
                "claude",
                Some("temporary"),
                Some(TurnProvider::Snapshot(changed.clone())),
            )
            .unwrap();
        assert_eq!(config.provider.unwrap().metadata.model, "temporary");
        assert!(
            store
                .turn_config(
                    "claude",
                    Some("outside-list"),
                    Some(TurnProvider::Snapshot(changed.clone()))
                )
                .is_err()
        );
        changed.provider.protocol = ProviderProtocol::OpenaiChat;
        assert!(
            store
                .turn_config("claude", None, Some(TurnProvider::Snapshot(changed)))
                .is_err()
        );
        let mut empty_key = original.clone();
        empty_key.credential.clear();
        assert!(
            store
                .turn_config("claude", None, Some(TurnProvider::Snapshot(empty_key)))
                .is_err()
        );
        let fingerprint =
            turn_fingerprint(Some(&TurnProvider::Snapshot(original.clone()))).unwrap();
        let mut rotated = original;
        rotated.credential = "rotated-same-revision".into();
        assert_ne!(
            fingerprint,
            turn_fingerprint(Some(&TurnProvider::Snapshot(rotated))).unwrap()
        );
        assert_eq!(std::fs::read(&store.path).unwrap(), before);
    }
    #[test]
    fn catalog_binding_sync_and_revision_are_independent() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let mut local = ProviderStore::open(a.path()).unwrap();
        let mut remote = ProviderStore::open(b.path()).unwrap();
        for protocol in [
            ProviderProtocol::OpenaiChat,
            ProviderProtocol::OpenaiResponses,
        ] {
            let id = saved(&mut local, draft(protocol));
            assert!(local.bind("claude", Some(id)).is_err());
        }
        let id = saved(&mut local, draft(ProviderProtocol::AnthropicMessages));
        assert!(local.launch_config("claude").provider.is_none());
        let snapshot = local.snapshot("claude", &id).unwrap();
        let old_revision = snapshot.provider.revision.clone();
        remote.sync("claude", Some(snapshot)).unwrap();
        assert!(local.launch_config("claude").provider.is_none());
        let before = remote.launch_config("claude");
        let mut edit = draft(ProviderProtocol::AnthropicMessages);
        edit.id = Some(id.clone());
        edit.credential = None;
        edit.model = "second".into();
        local.save(edit.clone()).unwrap();
        assert_eq!(before.provider.unwrap().metadata.model, "first");
        assert_eq!(
            remote
                .launch_config("claude")
                .provider
                .unwrap()
                .metadata
                .revision,
            old_revision
        );
        remote
            .sync("claude", Some(local.snapshot("claude", &id).unwrap()))
            .unwrap();
        assert_eq!(
            remote
                .launch_config("claude")
                .provider
                .unwrap()
                .metadata
                .model,
            "second"
        );
        local.bind("claude", Some(id.clone())).unwrap();
        edit.protocol = ProviderProtocol::OpenaiResponses;
        assert!(local.save(edit).is_err());
        assert!(local.delete(&id).is_err());
        assert!(
            !serde_json::to_string(&remote.list())
                .unwrap()
                .contains("fixture-private-key")
        );
        latte_work_platform::fs::verify_private_file(&std::fs::File::open(&remote.path).unwrap())
            .unwrap();
        let mut reopened = ProviderStore::open(b.path()).unwrap();
        assert_eq!(
            reopened
                .launch_config("claude")
                .provider
                .unwrap()
                .metadata
                .model,
            "second"
        );
        reopened.bind("claude", None).unwrap();
        assert!(
            matches!(reopened.list(),Response::Providers{providers,bindings} if providers.is_empty() && bindings.is_empty())
        );
    }
    #[test]
    fn invalid_updates_preserve_previous_state_and_migrate_v1() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ProviderStore::open(dir.path()).unwrap();
        let id = saved(&mut store, draft(ProviderProtocol::AnthropicMessages));
        let mut edit = draft(ProviderProtocol::AnthropicMessages);
        edit.id = Some(id.clone());
        edit.credential = None;
        edit.base_url = "https://other.test".into();
        assert!(store.save(edit).is_err());
        assert!(store.bind("unknown", Some(id.clone())).is_err());
        assert!(store.bind("claude", Some("missing".into())).is_err());
        for url in [
            "file:///tmp/x",
            "https://user:key@example.test/v1",
            "https://example.test?key=x",
        ] {
            let mut d = draft(ProviderProtocol::AnthropicMessages);
            d.base_url = url.into();
            assert!(store.save(d).is_err());
        }
        let mut value = serde_json::to_value(&store.config).unwrap();
        value["schema"] = serde_json::json!(1);
        value["active_id"] = serde_json::json!(id);
        value.as_object_mut().unwrap().remove("bindings");
        std::fs::write(&store.path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(
            ProviderStore::open(dir.path())
                .unwrap()
                .launch_config("claude")
                .provider
                .is_some()
        );
    }
}
