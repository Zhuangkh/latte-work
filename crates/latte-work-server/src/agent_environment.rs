//! Host-local environment snapshot shared by agent probes and launches.
//! Never mutate the daemon's environment or persist captured credentials.
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    sync::OnceLock,
};
use tokio::process::Command;

#[cfg(unix)]
#[path = "agent_environment/unix.rs"]
mod backend;
#[cfg(windows)]
#[path = "agent_environment/windows.rs"]
mod backend;

static ENVIRONMENT: OnceLock<BTreeMap<OsString, OsString>> = OnceLock::new();

pub async fn initialize() -> Option<String> {
    let result = if std::env::var("LATTE_WORK_AGENT_ENV").as_deref() == Ok("inherit") {
        Ok(BTreeMap::new())
    } else {
        backend::capture_default().await
    };
    match result {
        Ok(env) => {
            let _ = ENVIRONMENT.set(env);
            None
        }
        Err(error) => {
            let _ = ENVIRONMENT.set(BTreeMap::new());
            Some(format!(
                "Shell 环境加载失败（{error}），当前使用后台继承环境；修复 Shell 配置后重启后台。"
            ))
        }
    }
}

/// Resolve a native storage setting from the same environment used by the agent.
pub fn variable(name: impl AsRef<OsStr>) -> Option<OsString> {
    let name = name.as_ref();
    ENVIRONMENT
        .get()
        .and_then(|env| env.get(name).cloned())
        .or_else(|| std::env::var_os(name))
}

pub fn command(binary: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(binary);
    if let Some(env) = ENVIRONMENT.get() {
        cmd.envs(env);
    }
    cmd
}
