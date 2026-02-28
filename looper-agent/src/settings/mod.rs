use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use looper_common::ProviderApiKey;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSettings {
    pub workspace_dir: String,
    pub port: u16,
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentKeys {
    #[serde(default)]
    pub api_keys: Vec<ProviderApiKey>,
}

#[derive(Debug, Clone)]
pub struct PersistedAgentConfig {
    pub settings: AgentSettings,
    pub keys: AgentKeys,
}

pub fn load_persisted_config(workspace_dir: &Path) -> anyhow::Result<Option<PersistedAgentConfig>> {
    let settings_path = workspace_dir.join("settings.json");
    let keys_path = workspace_dir.join("keys.json");

    if !settings_path.exists() || !keys_path.exists() {
        return Ok(None);
    }

    let settings_text = fs::read_to_string(&settings_path)
        .with_context(|| format!("failed to read {}", settings_path.display()))?;
    let keys_text = fs::read_to_string(&keys_path)
        .with_context(|| format!("failed to read {}", keys_path.display()))?;

    let settings: AgentSettings = serde_json::from_str(&settings_text)
        .with_context(|| format!("invalid settings file {}", settings_path.display()))?;
    let keys: AgentKeys = serde_json::from_str(&keys_text)
        .with_context(|| format!("invalid keys file {}", keys_path.display()))?;

    if settings.workspace_dir.trim().is_empty() {
        bail!("settings.json has empty workspace_dir");
    }

    Ok(Some(PersistedAgentConfig { settings, keys }))
}

pub fn is_config_complete(config: &PersistedAgentConfig) -> bool {
    if config.settings.provider.trim().is_empty() {
        return false;
    }

    config
        .keys
        .api_keys
        .iter()
        .any(|key| key.provider == config.settings.provider && !key.api_key.trim().is_empty())
}

pub fn persist_config(
    workspace_dir: &Path,
    settings: AgentSettings,
    keys: AgentKeys,
) -> anyhow::Result<PersistedAgentConfig> {
    fs::create_dir_all(workspace_dir)
        .with_context(|| format!("failed to create workspace {}", workspace_dir.display()))?;

    let settings_path = workspace_dir.join("settings.json");
    let keys_path = workspace_dir.join("keys.json");

    let settings_text = serde_json::to_string_pretty(&settings).context("serialize settings")?;
    let keys_text = serde_json::to_string_pretty(&keys).context("serialize keys")?;

    fs::write(&settings_path, settings_text)
        .with_context(|| format!("failed to write {}", settings_path.display()))?;
    fs::write(&keys_path, keys_text)
        .with_context(|| format!("failed to write {}", keys_path.display()))?;

    Ok(PersistedAgentConfig { settings, keys })
}

pub fn normalize_workspace_dir(workspace_dir: &str) -> anyhow::Result<PathBuf> {
    let trimmed = workspace_dir.trim();
    if trimmed.is_empty() {
        bail!("workspace directory cannot be empty");
    }
    Ok(PathBuf::from(trimmed))
}
