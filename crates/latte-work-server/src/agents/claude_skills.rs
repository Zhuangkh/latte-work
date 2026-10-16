//! Current installed-file metadata only, never an execution/availability decision.
//! Skill locations: <https://code.claude.com/docs/en/skills#choose-where-skills-load>
use anyhow::{Context, Result, bail};
use latte_work_protocol::SkillInfo;
use std::{
    io::Read,
    path::{Path, PathBuf},
};
const FILE_LIMIT: u64 = 64 * 1024;

fn component(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn read(path: &Path, limit: u64) -> Result<String> {
    // Skill folders and in-directory entry links are resolved by inspect first.
    // Open the resolved leaf without following a replacement link.
    let file = latte_work_platform::fs::open_regular_file(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.len() > limit {
        bail!("Skill 元数据超出读取范围");
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("Skill 元数据超出读取范围");
    }
    String::from_utf8(bytes).context("Skill 文件不是 UTF-8")
}
// Only display scalar frontmatter fields. Never evaluate templates, YAML tags or body commands.
fn field(document: &str, key: &str) -> String {
    let mut lines = document.trim_start_matches('\u{feff}').lines();
    if lines.next().map(str::trim) != Some("---") {
        return String::new();
    }
    let mut found = false;
    let mut value = String::new();
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if found {
            if line.starts_with([' ', '\t']) || line.trim().is_empty() {
                if !value.is_empty() {
                    value.push(' ');
                }
                value.push_str(line.trim());
                continue;
            }
            break;
        }
        if let Some(raw) = line.strip_prefix(&format!("{key}:")) {
            let raw = raw.trim();
            if [">", ">-", ">+", "|", "|-", "|+", ""].contains(&raw) {
                found = true;
                continue;
            }
            value = if raw.starts_with('"') {
                serde_json::from_str::<String>(raw).unwrap_or_default()
            } else if raw.starts_with('\'') && raw.ends_with('\'') && raw.len() >= 2 {
                raw[1..raw.len() - 1].replace("''", "'")
            } else if raw.starts_with(['[', '{', '!', '&', '*']) {
                String::new()
            } else {
                raw.into()
            };
            break;
        }
    }
    value
        .chars()
        .take(4096)
        .collect::<String>()
        .trim()
        .to_owned()
}
fn inspect(directory: &Path, requested: &str) -> Result<Option<SkillInfo>> {
    let directory = match directory.canonicalize() {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let entry = directory.join("SKILL.md");
    let target = match entry.canonicalize() {
        Ok(path) => path,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if !target.starts_with(&directory) {
        bail!("Skill 说明文件超出目录范围");
    }
    let document = read(&target, FILE_LIMIT)?;
    let name = field(&document, "name");
    Ok(Some(SkillInfo {
        name: if name.is_empty() {
            requested.into()
        } else {
            name
        },
        description: field(&document, "description"),
        directory: directory.to_str().context("Skill 路径不是 UTF-8")?.into(),
    }))
}

pub fn find(project: &Path, name: &str) -> Result<Option<SkillInfo>> {
    let config = crate::agent_environment::variable("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            crate::agent_environment::variable("HOME")
                .map(|home| PathBuf::from(home).join(".claude"))
        })
        .or_else(|| {
            latte_work_platform::dirs::home_dir()
                .ok()
                .map(|p| p.join(".claude"))
        })
        .context("无法确定 Agent 配置目录")?;
    find_in(project, &config, name)
}
fn find_in(project: &Path, config: &Path, name: &str) -> Result<Option<SkillInfo>> {
    let parts: Vec<_> = name.split(':').collect();
    if parts.len() > 2 || !parts.iter().all(|part| component(part)) {
        bail!("Skill 名称无效");
    }
    if parts.len() == 2 {
        // Resolve exact installed plugin identity, never scan arbitrary cached versions.
        let index = config.join("plugins/installed_plugins.json");
        if !index.exists() {
            return Ok(None);
        }
        let index: serde_json::Value =
            serde_json::from_str(&read(&index.canonicalize()?, 256 * 1024)?)?;
        let mut matches = Vec::new();
        if let Some(plugins) = index.get("plugins").and_then(|p| p.as_object()) {
            for (key, installs) in plugins.iter().take(256) {
                if key.split('@').next() != Some(parts[0]) {
                    continue;
                }
                for install in installs.as_array().into_iter().flatten().take(16) {
                    if let Some(scope) = install.get("projectPath").and_then(|p| p.as_str())
                        && !project.starts_with(scope)
                    {
                        continue;
                    }
                    if let Some(path) = install.get("installPath").and_then(|p| p.as_str()) {
                        let root = Path::new(path);
                        if !root.is_absolute() {
                            continue;
                        }
                        let info = inspect(&root.join("skills").join(parts[1]), name)?;
                        let info = if info.is_some() {
                            info
                        } else {
                            inspect(root, name)?
                        };
                        if let Some(info) = info
                            && !matches
                                .iter()
                                .any(|existing: &SkillInfo| existing.directory == info.directory)
                        {
                            matches.push(info);
                        }
                    }
                }
            }
        }
        if matches.len() > 1 {
            bail!("此 Skill 有多个安装位置，无法确认当前目录");
        }
        return Ok(matches.pop());
    }
    // Personal precedes project. Symlinked skill folders are supported by canonicalizing them.
    if let Some(info) = inspect(&config.join("skills").join(name), name)? {
        return Ok(Some(info));
    }
    for parent in project.ancestors().take(32) {
        if let Some(info) = inspect(&parent.join(".claude/skills").join(name), name)? {
            return Ok(Some(info));
        }
        if parent.join(".git").exists() {
            break;
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn skill(directory: &Path, contents: &str) {
        std::fs::create_dir_all(directory).unwrap();
        std::fs::write(directory.join("SKILL.md"), contents).unwrap();
    }
    #[test]
    fn reads_real_frontmatter_and_personal_precedence_without_running_body() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config");
        let project = temp.path().join("project");
        skill(
            &project.join(".claude/skills/review"),
            "---\nname: project\n---\n",
        );
        skill(
            &config.join("skills/review"),
            "---\nname: Review\ndescription: >-\n  Inspect changes\n  with evidence.\n---\n!`touch MUST_NOT_EXIST`\n",
        );
        let info = find_in(&project, &config, "review").unwrap().unwrap();
        assert_eq!(info.name, "Review");
        assert_eq!(info.description, "Inspect changes with evidence.");
        assert!(!temp.path().join("MUST_NOT_EXIST").exists());
        assert_eq!(
            field("---\ndescription: 'It''s read-only'\n---", "description"),
            "It's read-only"
        );
        assert_eq!(
            field("---\ndescription: \"Line\\nTwo\"\n---", "description"),
            "Line\nTwo"
        );
    }
    #[test]
    fn rejects_traversal_oversize_and_symlinked_entry_escape_but_accepts_skill_folder_links() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for name in ["../secret", "/tmp", "a:b:c", "a/skill", "."] {
            assert!(find_in(root, root, name).is_err());
        }
        skill(&root.join("actual"), "---\ndescription: linked\n---");
        #[cfg(unix)]
        {
            std::fs::create_dir_all(root.join("skills")).unwrap();
            std::os::unix::fs::symlink(root.join("actual"), root.join("skills/linked")).unwrap();
            assert_eq!(
                find_in(root, root, "linked").unwrap().unwrap().description,
                "linked"
            );
        }
        skill(
            &root.join("skills/large"),
            &"a".repeat(FILE_LIMIT as usize + 1),
        );
        assert!(find_in(root, root, "large").is_err());
        #[cfg(unix)]
        {
            std::fs::create_dir_all(root.join("skills/escape")).unwrap();
            std::os::unix::fs::symlink(
                root.join("actual/SKILL.md"),
                root.join("skills/escape/SKILL.md"),
            )
            .unwrap();
            assert!(find_in(root, root, "escape").is_err());
        }
        assert!(find_in(root, root, "missing").unwrap().is_none());
    }
    #[test]
    fn resolves_only_the_requested_installed_plugin() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let install = root.join("installed");
        skill(
            &install.join("skills/review"),
            "---\nname: Review\ndescription: Plugin review\n---",
        );
        std::fs::create_dir_all(root.join("plugins")).unwrap();
        std::fs::write(
            root.join("plugins/installed_plugins.json"),
            serde_json::json!({"plugins":{"example@market":[{"installPath":install}]}}).to_string(),
        )
        .unwrap();
        assert_eq!(
            find_in(root, root, "example:review")
                .unwrap()
                .unwrap()
                .description,
            "Plugin review"
        );
        assert!(find_in(root, root, "other:review").unwrap().is_none());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn linked_skill_folders_are_supported_but_entry_escape_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let actual = root.join("actual");
        skill(&actual, "---\ndescription: linked\n---");
        let linked = root.join("skills/linked");
        let escape = root.join("skills/escape/SKILL.md");
        std::fs::create_dir_all(escape.parent().unwrap()).unwrap();
        let mut command = tokio::process::Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:LATTE_SKILL_LINK -Target $env:LATTE_SKILL_TARGET | Out-Null; New-Item -ItemType Junction -Path $env:LATTE_SKILL_ESCAPE -Target $env:LATTE_SKILL_TARGET | Out-Null"])
            .env("LATTE_SKILL_LINK", &linked)
            .env("LATTE_SKILL_TARGET", &actual)
            .env("LATTE_SKILL_ESCAPE", &escape);
        let (status, _) = latte_work_platform::process::output(
            &mut command,
            std::time::Duration::from_secs(10),
            4096,
        )
        .await
        .unwrap();
        assert!(status.success());
        assert_eq!(
            find_in(root, root, "linked").unwrap().unwrap().description,
            "linked"
        );
        assert!(find_in(root, root, "escape").is_err());
        assert!(read(&actual, FILE_LIMIT).is_err());
        assert!(latte_work_platform::fs::open_regular_file(&linked).is_err());
        assert_eq!(
            std::fs::read_to_string(actual.join("SKILL.md")).unwrap(),
            "---\ndescription: linked\n---"
        );
        std::fs::remove_dir(escape).unwrap();
        std::fs::remove_dir(linked).unwrap();
    }
}
