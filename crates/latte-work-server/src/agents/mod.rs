//! Protocol adapters translate native agent messages; they do not own storage or UI.
mod adapter;
mod claude;
mod claude_models;
mod claude_sessions;
pub use adapter::{Action, AgentAdapter, AgentCommand, Input};
use anyhow::{Result, bail};
pub type ChildTranscript = (Vec<latte_work_protocol::Event>, bool);
pub fn confirmed_child_report(
    tool: &latte_work_protocol::EventKind,
    result: &latte_work_protocol::EventKind,
) -> Option<String> {
    claude::confirmed_child_report(tool, result)
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct ChildHistoryCursor {
    pub offset: u64,
    pub identity: String,
    #[serde(default)]
    pub has_more: bool,
    #[serde(default)]
    pub pending_tail: bool,
    pub handbacks: std::collections::HashMap<String, String>,
}
pub struct ChildHistoryPage {
    pub events: Vec<latte_work_protocol::Event>,
    pub cursor: ChildHistoryCursor,
    pub has_more: bool,
}
pub async fn child_history(
    cwd: std::path::PathBuf,
    parent: String,
    child: String,
    session_id: String,
    config: crate::providers::LaunchConfig,
    cursor: ChildHistoryCursor,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<Option<ChildHistoryPage>> {
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let Some(raw) = claude_sessions::read_child_page(&cwd, &parent, &child, cursor)? else {
                return Ok(None);
            };
            claude::parse_child_history(raw, &parent, &child, &session_id, &config).map(Some)
        }),
    )
    .await
    .map_err(|_| anyhow::anyhow!("读取子智能体过程超时"))??
}
pub fn normalize_child_transcript(
    agent: &str,
    snapshot: ChildTranscript,
) -> Result<ChildTranscript> {
    match agent {
        "claude" => claude::normalize_child_transcript(snapshot),
        _ => bail!("此 Agent 尚未实现子智能体过程：{agent}"),
    }
}
pub fn child_read_permit() -> Result<tokio::sync::OwnedSemaphorePermit> {
    static READS: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::OnceLock::new();
    READS
        .get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(2)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| anyhow::anyhow!("子智能体过程正在读取，请稍后重试"))
}

/// Read only a child owned by the caller's existing runtime/redaction context.
pub async fn child_transcript(
    cwd: std::path::PathBuf,
    parent: String,
    child: String,
    session_id: String,
    config: crate::providers::LaunchConfig,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<Option<ChildTranscript>> {
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let Some(bytes) = claude_sessions::read_child(&cwd, &parent, &child)? else {
                return Ok(None);
            };
            claude::parse_child_transcript(bytes, &parent, &child, &session_id, &config).map(Some)
        }),
    )
    .await
    .map_err(|_| anyhow::anyhow!("读取子智能体过程超时"))??
}

/// Concrete adapters are constructed only here; unsupported IDs never fall back.
pub fn create(agent: &str) -> Result<Box<dyn AgentAdapter>> {
    match agent {
        "claude" => Ok(Box::<claude::Claude>::default()),
        _ => bail!("此 Agent 尚未实现：{agent}"),
    }
}

/// The registry currently contains one implementation. Discovery stays implementation-owned.
pub async fn discover() -> (String, latte_work_protocol::AgentInfo) {
    claude::discover().await
}

/// Protocol compatibility belongs to the adapter registry, never to presentation code.
pub fn provider_protocols(agent: &str) -> Option<&'static [latte_work_protocol::ProviderProtocol]> {
    match agent {
        "claude" => Some(&[latte_work_protocol::ProviderProtocol::AnthropicMessages]),
        _ => None,
    }
}

/// Built-in choices when an agent has no custom Provider binding.
pub fn model_aliases(agent: &str) -> &'static [&'static str] {
    match agent {
        "claude" => claude::MODEL_ALIASES,
        _ => &[],
    }
}

/// Only the selected adapter defines valid levels. Never share a fallback scale.
pub fn effort_levels(agent: &str, model: Option<&str>) -> &'static [latte_work_protocol::Effort] {
    match agent {
        "claude" => claude::effort_levels(model),
        _ => &[],
    }
}

/// Probe the selected host's executable; unknown adapters never inherit another policy.
pub async fn permission_modes(
    agent: &str,
    binary: &str,
) -> Result<Vec<latte_work_protocol::AgentPermissionMode>> {
    match agent {
        "claude" => claude::permission_modes(binary).await,
        _ => bail!("此 Agent 尚未实现权限设置：{agent}"),
    }
}

/// Return display-only model names from the selected Host's native Agent settings.
pub fn model_labels(
    agent: &str,
    project: Option<&std::path::Path>,
) -> std::collections::BTreeMap<String, String> {
    match agent {
        "claude" => claude_models::labels(project),
        _ => std::collections::BTreeMap::new(),
    }
}

pub use claude_sessions::Title as NativeTitle;
pub fn session_title(agent: &str, path: &std::path::Path, id: &str) -> Result<Option<NativeTitle>> {
    match agent {
        "claude" => claude_sessions::read(path, id),
        _ => bail!("此 Agent 尚不支持读取会话标题"),
    }
}
pub fn rename_saved_session(
    agent: &str,
    path: &std::path::Path,
    id: &str,
    title: &str,
) -> Result<String> {
    match agent {
        "claude" => claude_sessions::rename(path, id, title),
        _ => bail!("此 Agent 尚不支持修改会话标题"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn unimplemented_agents_do_not_inherit_claude_effort() {
        for agent in ["codex", "opencode", "unknown", ""] {
            assert!(super::create(agent).is_err());
            assert!(super::effort_levels(agent, Some("opus")).is_empty());
            assert!(super::effort_levels(agent, None).is_empty());
        }
        assert!(!super::effort_levels("claude", Some("opus")).is_empty());
    }
}
