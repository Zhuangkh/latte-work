//! Read-only display metadata. Selection still sends native aliases to Claude.
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};

const MAX_SETTINGS: u64 = 1024 * 1024;
const FAMILIES: &[&str] = &["SONNET", "OPUS", "HAIKU", "FABLE"];

pub fn labels(project: Option<&Path>) -> BTreeMap<String, String> {
    let directory = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            latte_work_platform::dirs::home_dir()
                .ok()
                .map(|h| h.join(".claude"))
        });
    let mut files = Vec::new();
    if let Some(directory) = directory {
        files.push(directory.join("settings.json"));
    }
    if let Some(project) = project {
        files.push(project.join(".claude/settings.json"));
        files.push(project.join(".claude/settings.local.json"));
    }
    // Local managed JSON can override ordinary settings. Cloud/MDM policy is
    // resolved by Claude itself; this metadata is not an execution policy.
    files.push(if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json")
    } else {
        PathBuf::from("/etc/claude-code/managed-settings.json")
    });
    let env = FAMILIES
        .iter()
        .flat_map(|family| {
            [
                format!("ANTHROPIC_DEFAULT_{family}_MODEL"),
                format!("ANTHROPIC_DEFAULT_{family}_MODEL_NAME"),
            ]
        })
        .filter_map(|key| std::env::var(&key).ok().map(|value| (key, value)))
        .collect();
    labels_from_sources(&files, env)
}

fn labels_from_sources(
    files: &[PathBuf],
    mut env: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let keys: Vec<_> = FAMILIES
        .iter()
        .flat_map(|family| {
            [
                format!("ANTHROPIC_DEFAULT_{family}_MODEL"),
                format!("ANTHROPIC_DEFAULT_{family}_MODEL_NAME"),
            ]
        })
        .collect();
    env.retain(|key, _| keys.contains(key));
    for path in files {
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        if !meta.is_file() || meta.len() > MAX_SETTINGS {
            continue;
        }
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        let mut bytes = Vec::new();
        if file.take(MAX_SETTINGS + 1).read_to_end(&mut bytes).is_err()
            || bytes.len() as u64 > MAX_SETTINGS
        {
            continue;
        }
        let Ok(settings) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        for key in &keys {
            if let Some(value) = settings
                .get("env")
                .and_then(|e| e.get(key))
                .and_then(Value::as_str)
            {
                env.insert(key.clone(), value.to_owned());
            }
        }
    }
    let mut labels = BTreeMap::new();
    for family in FAMILIES {
        let key = format!("ANTHROPIC_DEFAULT_{family}_MODEL");
        let Some(model) = env.get(&key).and_then(|v| display_text(v)) else {
            continue;
        };
        let name = env
            .get(&format!("{key}_NAME"))
            .and_then(|v| display_text(v))
            .unwrap_or(model);
        labels.insert(family.to_ascii_lowercase(), name.to_owned());
    }
    labels
}

fn display_text(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty() && value.chars().count() <= 256 && !value.chars().any(char::is_control))
        .then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn names_fallback_to_ids_and_only_model_metadata_leaves_the_adapter() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("settings.json");
        std::fs::write(
            &path,
            json!({"env": {
                "ANTHROPIC_DEFAULT_SONNET_MODEL":"gateway/sonnet[1m]",
                "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME":"My Sonnet",
                "ANTHROPIC_DEFAULT_HAIKU_MODEL":"gateway/small",
                "ANTHROPIC_API_KEY":"private-value",
                "ANTHROPIC_BASE_URL":"https://private.example"
            }})
            .to_string(),
        )
        .unwrap();
        let labels = labels_from_sources(&[path], BTreeMap::new());
        assert_eq!(
            labels,
            BTreeMap::from([
                ("sonnet".into(), "My Sonnet".into()),
                ("haiku".into(), "gateway/small".into())
            ])
        );
        let serialized = serde_json::to_string(&labels).unwrap();
        assert!(!serialized.contains("private"));
    }
    #[test]
    fn settings_override_environment_and_higher_scopes_override_lower_scopes() {
        let root = tempfile::tempdir().unwrap();
        let files: Vec<_> = ["user", "project", "local", "managed"]
            .iter()
            .map(|name| root.path().join(name))
            .collect();
        let key = "ANTHROPIC_DEFAULT_OPUS_MODEL";
        let name = "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME";
        for (index, file) in files.iter().enumerate() {
            std::fs::write(
                file,
                json!({"env": {key: "mapped-model", name: format!("scope-{index}")}}).to_string(),
            )
            .unwrap();
        }
        let env = BTreeMap::from([
            (key.into(), "env-model".into()),
            (name.into(), "env-name".into()),
        ]);
        assert_eq!(labels_from_sources(&[], env.clone())["opus"], "env-name");
        for count in 1..=files.len() {
            assert_eq!(
                labels_from_sources(&files[..count], env.clone())["opus"],
                format!("scope-{}", count - 1)
            );
        }
    }
    #[test]
    fn absent_broken_or_oversized_files_and_invalid_names_are_safe() {
        let root = tempfile::tempdir().unwrap();
        let broken = root.path().join("broken");
        let large = root.path().join("large");
        std::fs::write(&broken, "not json").unwrap();
        std::fs::write(&large, vec![b' '; MAX_SETTINGS as usize + 1]).unwrap();
        assert!(
            labels_from_sources(
                &[broken, large, root.path().join("missing")],
                BTreeMap::new()
            )
            .is_empty()
        );
        let env = BTreeMap::from([
            ("ANTHROPIC_DEFAULT_SONNET_MODEL".into(), "mapped-id".into()),
            (
                "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME".into(),
                "bad\nname".into(),
            ),
        ]);
        assert_eq!(labels_from_sources(&[], env)["sonnet"], "mapped-id");
    }
}
