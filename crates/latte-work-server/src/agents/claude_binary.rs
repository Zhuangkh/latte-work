//! Claude installation policy. Resolve once, then probe and execute that exact path.
use anyhow::{Context, Result, bail};
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

#[cfg(unix)]
#[path = "claude_binary/unix.rs"]
mod backend;
#[cfg(windows)]
#[path = "claude_binary/windows.rs"]
mod backend;

pub(super) fn resolve() -> Result<PathBuf> {
    resolve_in(
        std::env::var_os("LATTE_WORK_CLAUDE").as_deref(),
        latte_work_platform::dirs::home_dir().ok().as_deref(),
        crate::agent_environment::variable(OsStr::new("PATH")).as_deref(),
        &std::env::current_dir()?,
    )
}
fn resolve_in(
    override_path: Option<&OsStr>,
    home: Option<&Path>,
    paths: Option<&OsStr>,
    cwd: &Path,
) -> Result<PathBuf> {
    let candidate = if let Some(value) = override_path {
        let path = PathBuf::from(value);
        if !path.is_absolute() {
            bail!("LATTE_WORK_CLAUDE 必须是原生 CLI 的绝对路径");
        }
        backend::validate(&path)?;
        which::which_in(&path, paths, cwd).context("LATTE_WORK_CLAUDE 不可执行或不存在")?
    } else {
        let installed = home.map(|home| home.join(".local").join("bin").join(backend::NAME));
        if let Some(path) = installed.filter(|path| path.is_file()) {
            which::which_in(path, paths, cwd).context("Claude 安装路径不可执行")?
        } else {
            which::which_in(backend::NAME, paths, cwd)
                .context("Agent 环境 PATH 中未找到 Claude 原生 CLI")?
        }
    };
    backend::validate(&candidate)?;
    let path = candidate
        .canonicalize()
        .context("无法解析 Claude CLI 路径")?;
    backend::validate(&path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_effective_path_and_explicit_override_without_lossy_conversion() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("工具 bin");
        std::fs::create_dir(&bin).unwrap();
        let executable = bin.join(backend::NAME);
        std::fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
        let paths = std::env::join_paths([&bin]).unwrap();
        assert_eq!(
            resolve_in(None, None, Some(&paths), root.path()).unwrap(),
            executable.canonicalize().unwrap()
        );
        assert_eq!(
            resolve_in(Some(executable.as_os_str()), None, None, root.path()).unwrap(),
            executable.canonicalize().unwrap()
        );
        assert!(resolve_in(Some(OsStr::new("claude")), None, Some(&paths), root.path()).is_err());
        assert!(
            resolve_in(
                Some(root.path().join("missing.exe").as_os_str()),
                None,
                Some(&paths),
                root.path()
            )
            .is_err()
        );
    }
}
