use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, anyhow};
use glob::glob;
use regex::Regex;
use walkdir::WalkDir;

use crate::model::Action;

/// A pluggable actuator executor.
pub trait ActuatorExecutor: Send + Sync {
    /// Executes a planned action and returns its output.
    fn execute(&self, action: &Action) -> Result<String>;
}

/// Chat actuator executor.
#[derive(Default)]
pub struct ChatActuatorExecutor;

impl ActuatorExecutor for ChatActuatorExecutor {
    fn execute(&self, action: &Action) -> Result<String> {
        if let Action::ChatResponse { message } = action {
            return Ok(message.clone());
        }
        Err(anyhow!("chat executor received incompatible action"))
    }
}

/// Glob actuator executor.
pub struct GlobActuatorExecutor {
    workspace_root: PathBuf,
}

impl GlobActuatorExecutor {
    /// Creates a glob executor rooted at a workspace path.
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

impl ActuatorExecutor for GlobActuatorExecutor {
    fn execute(&self, action: &Action) -> Result<String> {
        if let Action::Glob { pattern, path } = action {
            let base = normalize_rooted_path(&self.workspace_root, path);
            let full_pattern = base.join(pattern).to_string_lossy().to_string();
            let mut matches = Vec::new();

            for path in glob(&full_pattern)?.flatten() {
                matches.push(path.to_string_lossy().to_string());
            }

            matches.sort();
            if matches.is_empty() {
                return Ok("no files matched".to_string());
            }
            return Ok(matches.join("\n"));
        }

        Err(anyhow!("glob executor received incompatible action"))
    }
}

/// Grep actuator executor.
pub struct GrepActuatorExecutor {
    workspace_root: PathBuf,
}

impl GrepActuatorExecutor {
    /// Creates a grep executor rooted at a workspace path.
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

impl ActuatorExecutor for GrepActuatorExecutor {
    fn execute(&self, action: &Action) -> Result<String> {
        if let Action::Grep { pattern, path } = action {
            let root = normalize_rooted_path(&self.workspace_root, path);
            let regex = Regex::new(pattern)?;
            let mut hits = Vec::new();

            for entry in WalkDir::new(&root).into_iter().flatten() {
                if !entry.file_type().is_file() {
                    continue;
                }

                let file_path = entry.path();
                let Ok(content) = fs::read_to_string(file_path) else {
                    continue;
                };

                for (idx, line) in content.lines().enumerate() {
                    if regex.is_match(line) {
                        hits.push(format!(
                            "{}:{}:{}",
                            file_path.to_string_lossy(),
                            idx + 1,
                            line
                        ));
                    }
                }
            }

            if hits.is_empty() {
                return Ok("no matches found".to_string());
            }
            return Ok(hits.join("\n"));
        }

        Err(anyhow!("grep executor received incompatible action"))
    }
}

/// Shell actuator executor.
pub struct ShellActuatorExecutor {
    workspace_root: PathBuf,
}

impl ShellActuatorExecutor {
    /// Creates a shell executor rooted at a workspace path.
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

impl ActuatorExecutor for ShellActuatorExecutor {
    fn execute(&self, action: &Action) -> Result<String> {
        if let Action::Shell { command } = action {
            let output = if cfg!(target_os = "windows") {
                Command::new("cmd")
                    .arg("/C")
                    .arg(command)
                    .current_dir(&self.workspace_root)
                    .output()?
            } else {
                Command::new("sh")
                    .arg("-c")
                    .arg(command)
                    .current_dir(&self.workspace_root)
                    .output()?
            };

            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let mut parts = vec![format!("status: {}", output.status)];
            if !stdout.is_empty() {
                parts.push(format!("stdout:\n{}", stdout));
            }
            if !stderr.is_empty() {
                parts.push(format!("stderr:\n{}", stderr));
            }

            return Ok(parts.join("\n"));
        }

        Err(anyhow!("shell executor received incompatible action"))
    }
}

/// Web search actuator executor.
#[derive(Default)]
pub struct WebSearchActuatorExecutor;

impl ActuatorExecutor for WebSearchActuatorExecutor {
    fn execute(&self, action: &Action) -> Result<String> {
        if let Action::WebSearch { query } = action {
            return Ok(format!(
                "web search request accepted for query: '{query}' (provider integration pending)"
            ));
        }

        Err(anyhow!("web_search executor received incompatible action"))
    }
}

fn normalize_rooted_path(root: &Path, requested: &str) -> PathBuf {
    let requested_path = Path::new(requested);
    if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        root.join(requested_path)
    }
}
