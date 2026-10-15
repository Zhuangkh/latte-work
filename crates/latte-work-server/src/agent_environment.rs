//! Host-local exported shell environment, shared by agent probes and launches.
//! Never mutate the multithreaded daemon's global environment or persist secrets.
use anyhow::{Context, Result, bail};
use nix::{
    sys::signal::{Signal, killpg},
    unistd::{Pid, Uid, User},
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::OnceLock,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

const LIMIT: u64 = 512 * 1024;
static ENVIRONMENT: OnceLock<BTreeMap<OsString, OsString>> = OnceLock::new();

pub async fn initialize() -> Option<String> {
    if std::env::var("LATTE_WORK_AGENT_ENV").as_deref() == Ok("inherit") {
        let _ = ENVIRONMENT.set(BTreeMap::new());
        return None;
    }
    let shell = std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_file())
        .or_else(|| {
            User::from_uid(Uid::current())
                .ok()
                .flatten()
                .map(|u| u.shell)
        });
    let result = match shell {
        Some(shell) => capture(&shell, &[], Duration::from_secs(3)).await,
        None => Err(anyhow::anyhow!("无法确定默认 Shell")),
    };
    match result {
        Ok(env) => {
            let _ = ENVIRONMENT.set(env);
            None
        }
        Err(error) => {
            let _ = ENVIRONMENT.set(BTreeMap::new());
            // Errors contain no captured output or environment values.
            Some(format!(
                "Shell 环境加载失败（{error}），当前使用后台继承环境；修复 Shell 配置后重启后台。"
            ))
        }
    }
}

/// Resolve a native storage setting from the same environment used by the agent.
pub fn variable(name: &str) -> Option<OsString> {
    ENVIRONMENT
        .get()
        .and_then(|env| env.get(std::ffi::OsStr::new(name)))
        .cloned()
        .or_else(|| std::env::var_os(name))
}

pub fn command(binary: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(binary);
    if let Some(env) = ENVIRONMENT.get() {
        cmd.envs(env);
    }
    cmd
}

struct ProcessGroup(i32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        let _ = killpg(Pid::from_raw(self.0), Signal::SIGKILL);
    }
}

async fn capture(
    shell: &Path,
    overrides: &[(OsString, OsString)],
    timeout: Duration,
) -> Result<BTreeMap<OsString, OsString>> {
    let name = shell
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or_default();
    if !matches!(name, "zsh" | "bash" | "sh" | "dash" | "fish") {
        bail!("默认 Shell 尚不支持环境采集");
    }
    let marker = format!("LATTE_ENV_{}", uuid::Uuid::new_v4().simple());
    let script = format!("/usr/bin/printf '\\0{marker}\\0'; exec /usr/bin/env -0");
    let mut command = Command::new(shell);
    command
        .arg("-ilc")
        .arg(script)
        .envs(overrides.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .kill_on_drop(true);
    if let Some(home) = std::env::var_os("HOME") {
        command.current_dir(home);
    }
    let mut child = command.spawn().context("无法启动默认 Shell")?;
    let group = ProcessGroup(child.id().context("无法获取 Shell 进程")? as i32);
    let mut stdout = child
        .stdout
        .take()
        .context("Shell 无输出管道")?
        .take(LIMIT + 1);
    let result = tokio::time::timeout(timeout, async {
        let mut bytes = Vec::new();
        stdout
            .read_to_end(&mut bytes)
            .await
            .context("读取 Shell 环境失败")?;
        if bytes.len() as u64 > LIMIT {
            bail!("Shell 输出超出限制");
        }
        if !child.wait().await?.success() {
            bail!("Shell 初始化失败");
        }
        parse(&bytes, &marker)
    })
    .await;
    // Reap the child on every failure path; the group guard handles descendants.
    let _ = killpg(Pid::from_raw(group.0), Signal::SIGKILL);
    let _ = child.wait().await;
    result.context("Shell 初始化超时")?
}

fn parse(bytes: &[u8], marker: &str) -> Result<BTreeMap<OsString, OsString>> {
    let boundary = format!("\0{marker}\0");
    let start = bytes
        .windows(boundary.len())
        .position(|w| w == boundary.as_bytes())
        .context("Shell 未返回环境快照")?
        + boundary.len();
    let mut env = BTreeMap::new();
    for entry in bytes[start..].split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let equal = entry
            .iter()
            .position(|b| *b == b'=')
            .context("Shell 环境格式无效")?;
        let key = &entry[..equal];
        if key.is_empty() || !key.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_') {
            continue;
        }
        let name = std::str::from_utf8(key)?;
        // Keep host identity, cwd, and the daemon's explicit controls authoritative.
        if matches!(
            name,
            "HOME" | "USER" | "LOGNAME" | "SHELL" | "PWD" | "OLDPWD" | "SHLVL" | "_"
        ) || name.starts_with("LATTE_WORK_")
        {
            continue;
        }
        env.insert(
            OsString::from_vec(key.to_vec()),
            OsString::from_vec(entry[equal + 1..].to_vec()),
        );
    }
    if !env.contains_key(std::ffi::OsStr::new("PATH")) {
        bail!("Shell 未返回 PATH");
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_exported_values_without_banner_or_host_controls() {
        let env = parse(b"welcome\n\0test\0PATH=/custom/bin\0MULTI=a\nb=c\0HOME=/other\0LATTE_WORK_CLAUDE=bad\0", "test").unwrap();
        assert_eq!(env[std::ffi::OsStr::new("MULTI")], "a\nb=c");
        assert!(!env.contains_key(std::ffi::OsStr::new("HOME")));
        assert!(!env.contains_key(std::ffi::OsStr::new("LATTE_WORK_CLAUDE")));
        assert!(parse(b"banner only", "test").is_err());
    }
    #[tokio::test]
    async fn loads_real_zsh_login_and_interactive_exports() {
        if !Path::new("/bin/zsh").exists() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".zprofile"),
            "export LATTE_TEST_PROFILE='profile value'\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(".zshrc"),
            "echo banner\nexport LATTE_TEST_RC='rc value'\nexport PATH=/custom/bin:$PATH\n",
        )
        .unwrap();
        let env = capture(
            Path::new("/bin/zsh"),
            &[("ZDOTDIR".into(), dir.path().into())],
            Duration::from_secs(3),
        )
        .await
        .unwrap();
        assert_eq!(
            env[std::ffi::OsStr::new("LATTE_TEST_PROFILE")],
            "profile value"
        );
        assert_eq!(env[std::ffi::OsStr::new("LATTE_TEST_RC")], "rc value");
        assert!(
            env[std::ffi::OsStr::new("PATH")]
                .to_string_lossy()
                .starts_with("/custom/bin:")
        );
    }
    #[tokio::test]
    async fn hung_shell_is_bounded() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let shell = dir.path().join("bash");
        std::fs::write(&shell, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            capture(&shell, &[], Duration::from_millis(100))
                .await
                .unwrap_err()
                .to_string()
                .contains("超时")
        );
    }
}
