use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use async_stream::try_stream;
use fiddlesticks::{
    ChatEvent, ChatSession, ChatTurnRequest, build_provider_from_api_key, chat_service,
    parse_provider_id,
};
use futures_util::{Stream, StreamExt};
use looper_common::{Effect, Percept, SessionOrigin};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::settings::AgentKeys;

const CHAT_DOMAIN: &str = "chat";
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct PeasRuntime {
    agent_id: String,
    db_path: PathBuf,
    plugins: Vec<LoadedPlugin>,
}

#[derive(Debug, Clone)]
struct LoadedPlugin {
    manifest_path: PathBuf,
    root_dir: PathBuf,
    entry_path: PathBuf,
    manifest: PluginManifest,
}

#[derive(Debug, Clone, Deserialize)]
struct PluginManifest {
    name: String,
    description: String,
    version: String,
    entry: String,
    permissions: PluginPermissions,
    peas: PluginPeas,
}

#[derive(Debug, Clone, Deserialize)]
struct PluginPermissions {
    #[serde(default)]
    read: Vec<String>,
    #[serde(default)]
    run: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct PluginPeas {
    #[serde(default)]
    performance: Vec<PluginPerformance>,
    #[serde(default)]
    environment: Option<PluginEnvironment>,
    #[serde(default)]
    actuators: Vec<PluginComponent>,
    #[serde(default)]
    sensors: Vec<PluginComponent>,
}

#[derive(Debug, Clone, Deserialize)]
struct PluginPerformance {
    name: String,
    description: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PluginEnvironment {
    name: String,
    description: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PluginComponent {
    name: String,
    description: String,
}

#[derive(Debug, Serialize)]
struct ChatPluginPerceptInput {
    session_id: String,
    turn_id: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct ChatPluginPerceptPlan {
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default)]
    user_prompt: Option<String>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    task_completion: Option<ChatTaskCompletionOutput>,
}

#[derive(Debug, Deserialize)]
struct ChatTaskCompletionOutput {
    status: String,
    details: String,
}

fn default_mode() -> String {
    "stream_chat".to_string()
}

type EffectStream = Pin<Box<dyn Stream<Item = anyhow::Result<Effect>> + Send>>;

impl PeasRuntime {
    pub fn new(agent_id: String) -> anyhow::Result<Self> {
        let db_path = chats_db_path()?;
        initialize_db(&db_path)?;

        let plugins = load_plugins(&Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins"))?;

        if plugins.is_empty() {
            bail!("no PEAS plugins were loaded");
        }

        Ok(Self {
            agent_id,
            db_path,
            plugins,
        })
    }

    pub fn start_session(&self, origin: SessionOrigin) -> anyhow::Result<String> {
        if origin != SessionOrigin::TerminalChat {
            bail!("unsupported session origin for chat persistence");
        }

        let session_id = next_id("sess");
        let conn = open_db(&self.db_path)?;
        conn.execute(
            "INSERT INTO sessions (id, agent_id, origin, started_at, ended_at, metadata_json)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
            params![
                session_id,
                self.agent_id,
                "terminal_chat",
                now_millis() as i64,
                "{}"
            ],
        )
        .context("failed to persist new chat session")?;
        Ok(session_id)
    }

    pub fn end_session(&self, session_id: &str) -> anyhow::Result<()> {
        let conn = open_db(&self.db_path)?;
        conn.execute(
            "UPDATE sessions SET ended_at = ?2 WHERE id = ?1",
            params![session_id, now_millis() as i64],
        )
        .with_context(|| format!("failed to end chat session {session_id}"))?;
        Ok(())
    }

    pub async fn stream_percept_effects(
        &self,
        session_id: &str,
        domain: &str,
        percept: Percept,
        provider_name: &str,
        model: &str,
        keys: &AgentKeys,
    ) -> anyhow::Result<EffectStream> {
        if domain != CHAT_DOMAIN {
            bail!("unsupported domain: {domain}");
        }

        let provider_name = provider_name.to_string();
        let model = model.to_string();
        let keys = keys.clone();
        let session_id = session_id.to_string();
        let runtime = self.clone();

        let Percept::UserText { turn_id, text } = percept;
        runtime.append_event(
            &session_id,
            Some(turn_id.as_str()),
            "percept_user_text",
            Some("user"),
            &text,
        )?;

        let chat_plugin = runtime.chat_plugin()?;
        let plan = runtime.run_chat_plugin(chat_plugin, ChatPluginPerceptInput {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            text: text.clone(),
        })?;

        if plan.mode != "stream_chat" {
            bail!("unsupported chat plugin mode: {}", plan.mode);
        }

        let prompt = plan.user_prompt.unwrap_or(text);
        let turn_id_for_stream = turn_id.clone();

        let stream = try_stream! {
            let provider_id = parse_provider_id(&provider_name)
                .ok_or_else(|| anyhow::anyhow!("unsupported provider '{provider_name}' for fiddlesticks facade"))?;

            let api_key = keys
                .api_keys
                .iter()
                .find(|entry| entry.provider.eq_ignore_ascii_case(&provider_name) && !entry.api_key.trim().is_empty())
                .map(|entry| entry.api_key.clone())
                .ok_or_else(|| anyhow::anyhow!("missing API key for provider '{provider_name}'"))?;

            let provider = build_provider_from_api_key(provider_id, api_key)
                .map_err(|error| anyhow::anyhow!("failed to build provider facade: {error}"))?;

            let service = chat_service(provider);
            let mut session = ChatSession::new(session_id.clone(), provider_id, model.clone());
            let full_system_prompt = runtime.build_chat_system_prompt(plan.system_prompt.clone());
            if !full_system_prompt.trim().is_empty() {
                session = session.with_system_prompt(full_system_prompt);
            }

            let request = ChatTurnRequest::new(session, prompt).enable_streaming();
            let mut stream = service
                .stream_turn(request)
                .await
                .map_err(|error| anyhow::anyhow!("chat stream failed to start: {error}"))?;

            let mut assembled = String::new();
            let mut emitted_final = false;

            while let Some(event_result) = stream.next().await {
                let event = event_result
                    .map_err(|error| anyhow::anyhow!("chat stream failed: {error}"))?;

                match event {
                    ChatEvent::TextDelta(delta) => {
                        if delta.is_empty() {
                            continue;
                        }
                        assembled.push_str(&delta);
                        yield Effect::ChatResponseDelta {
                            turn_id: turn_id_for_stream.clone(),
                            text_delta: delta,
                        };
                    }
                    ChatEvent::AssistantMessageComplete(text) => {
                        emitted_final = true;
                        yield Effect::ChatResponse {
                            turn_id: turn_id_for_stream.clone(),
                            text,
                        };
                    }
                    ChatEvent::TurnComplete(_) => {
                        if !emitted_final {
                            yield Effect::ChatResponse {
                                turn_id: turn_id_for_stream.clone(),
                                text: assembled.clone(),
                            };
                        }
                    }
                    ChatEvent::ToolExecutionStarted(tool_call) => {
                        yield Effect::TaskCompletion {
                            turn_id: turn_id_for_stream.clone(),
                            status: "in_progress".to_string(),
                            details: format!("tool started: {tool_call:?}"),
                        };
                    }
                    ChatEvent::ToolExecutionFinished(tool_call) => {
                        yield Effect::TaskCompletion {
                            turn_id: turn_id_for_stream.clone(),
                            status: "completed".to_string(),
                            details: format!("tool completed: {tool_call:?}"),
                        };
                    }
                    ChatEvent::ToolRoundLimitReached { .. } | ChatEvent::ToolCallDelta(_) => {}
                }
            }

            if let Some(task_completion) = plan.task_completion {
                yield Effect::TaskCompletion {
                    turn_id: turn_id_for_stream,
                    status: task_completion.status,
                    details: task_completion.details,
                };
            }
        };

        Ok(Box::pin(stream))
    }

    pub fn record_effect(&self, session_id: &str, effect: &Effect) -> anyhow::Result<()> {
        match effect {
            Effect::ChatResponseDelta {
                turn_id,
                text_delta,
            } => self.append_event(
                session_id,
                Some(turn_id.as_str()),
                "effect_chat_response_delta",
                Some("assistant"),
                text_delta,
            ),
            Effect::ChatResponse { turn_id, text } => self.append_event(
                session_id,
                Some(turn_id.as_str()),
                "effect_chat_response",
                Some("assistant"),
                text,
            ),
            Effect::TaskCompletion {
                turn_id,
                status,
                details,
            } => {
                let payload = format!("status={status}; details={details}");
                self.append_event(
                    session_id,
                    Some(turn_id.as_str()),
                    "effect_task_completion",
                    Some("assistant"),
                    &payload,
                )
            }
        }
    }

    fn run_chat_plugin(
        &self,
        plugin: &LoadedPlugin,
        input: ChatPluginPerceptInput,
    ) -> anyhow::Result<ChatPluginPerceptPlan> {
        if !plugin.entry_path.exists() {
            bail!(
                "chat plugin '{}' entrypoint is missing at {}",
                plugin.manifest.name,
                plugin.entry_path.display()
            );
        }

        let mut cmd = Command::new("deno");
        cmd.arg("run").arg("--quiet");
        append_deno_permissions(&mut cmd, plugin);

        let mut child = cmd
            .arg(&plugin.entry_path)
            .current_dir(&plugin.root_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to start deno for chat plugin '{}'",
                    plugin.manifest.name
                )
            })?;

        let input_json = serde_json::to_string(&input).context("serialize plugin percept input")?;
        {
            let stdin = child
                .stdin
                .as_mut()
                .context("failed to open stdin for chat plugin")?;
            stdin
                .write_all(input_json.as_bytes())
                .context("failed to write plugin percept input")?;
        }

        let output = child
            .wait_with_output()
            .context("failed to wait for chat plugin process")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("chat plugin execution failed: {stderr}");
        }

        let stdout =
            String::from_utf8(output.stdout).context("chat plugin emitted invalid utf8")?;
        let trimmed = stdout.trim();
        if trimmed.is_empty() {
            bail!("chat plugin returned empty output");
        }

        serde_json::from_str::<ChatPluginPerceptPlan>(trimmed)
            .context("chat plugin returned invalid json plan payload")
    }

    fn chat_plugin(&self) -> anyhow::Result<&LoadedPlugin> {
        self.plugins
            .iter()
            .find(|plugin| {
                plugin
                    .manifest
                    .peas
                    .sensors
                    .iter()
                    .any(|sensor| sensor.name == "terminal_chat_percept")
                    && plugin
                        .manifest
                        .peas
                        .actuators
                        .iter()
                        .any(|actuator| actuator.name == "chat_effect_append")
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no active plugin can process chat percepts (missing sensor 'terminal_chat_percept' and actuator 'chat_effect_append')"
                )
            })
    }

    fn build_chat_system_prompt(&self, plugin_system_prompt: Option<String>) -> String {
        let mut sections = Vec::new();
        if let Some(system_prompt) = plugin_system_prompt {
            if !system_prompt.trim().is_empty() {
                sections.push(system_prompt);
            }
        }

        let context = self.build_component_context();
        if !context.is_empty() {
            sections.push(format!(
                "PEAS plugin context (available for consideration during this chat):\n{context}\nUse these components when relevant, and do not claim to have used a component unless it was actually invoked."
            ));
        }

        sections.join("\n\n")
    }

    fn build_component_context(&self) -> String {
        let mut lines = Vec::new();
        for plugin in &self.plugins {
            lines.push(format!(
                "- plugin '{}' v{}: {}",
                plugin.manifest.name, plugin.manifest.version, plugin.manifest.description
            ));

            if let Some(environment) = &plugin.manifest.peas.environment {
                lines.push(format!(
                    "  environment: {} - {}",
                    environment.name, environment.description
                ));
            }

            if !plugin.manifest.peas.actuators.is_empty() {
                let actuators = plugin
                    .manifest
                    .peas
                    .actuators
                    .iter()
                    .map(|entry| format!("{} ({})", entry.name, entry.description))
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!("  actuators: {actuators}"));
            }

            if !plugin.manifest.peas.sensors.is_empty() {
                let sensors = plugin
                    .manifest
                    .peas
                    .sensors
                    .iter()
                    .map(|entry| format!("{} ({})", entry.name, entry.description))
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!("  sensors: {sensors}"));
            }

            if !plugin.manifest.peas.performance.is_empty() {
                let performance = plugin
                    .manifest
                    .peas
                    .performance
                    .iter()
                    .map(|entry| format!("{} ({})", entry.name, entry.description))
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(format!("  performance: {performance}"));
            }
        }

        lines.join("\n")
    }

    fn append_event(
        &self,
        session_id: &str,
        turn_id: Option<&str>,
        event_kind: &str,
        role: Option<&str>,
        payload_json: &str,
    ) -> anyhow::Result<()> {
        let conn = open_db(&self.db_path)?;
        let event_id = next_id("evt");
        conn.execute(
            "INSERT INTO events (id, session_id, turn_id, event_kind, role, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                event_id,
                session_id,
                turn_id,
                event_kind,
                role,
                payload_json,
                now_millis() as i64
            ],
        )
        .with_context(|| format!("failed to append event for session {session_id}"))?;
        Ok(())
    }
}

fn load_plugins(plugins_root: &Path) -> anyhow::Result<Vec<LoadedPlugin>> {
    if !plugins_root.exists() {
        return Ok(Vec::new());
    }

    let mut plugins = Vec::new();
    for entry in fs::read_dir(plugins_root)
        .with_context(|| format!("failed to read plugins root at {}", plugins_root.display()))?
    {
        let entry = entry.context("failed to read plugin directory entry")?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let manifest_path = path.join("looper-plugin.json");
        if !manifest_path.exists() {
            continue;
        }

        let manifest_text = fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?;
        let manifest = serde_json::from_str::<PluginManifest>(&manifest_text)
            .with_context(|| format!("invalid plugin manifest {}", manifest_path.display()))?;

        if manifest.name.trim().is_empty() {
            bail!("plugin at {} has empty name", manifest_path.display());
        }

        let entry_path = path.join(&manifest.entry);
        if !entry_path.exists() {
            bail!(
                "plugin '{}' entry file is missing at {}",
                manifest.name,
                entry_path.display()
            );
        }

        plugins.push(LoadedPlugin {
            manifest_path,
            root_dir: path,
            entry_path,
            manifest,
        });
    }

    plugins.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));

    for pair in plugins.windows(2) {
        if pair[0].manifest.name == pair[1].manifest.name {
            bail!(
                "duplicate plugin id '{}' in {} and {}",
                pair[0].manifest.name,
                pair[0].manifest_path.display(),
                pair[1].manifest_path.display()
            );
        }
    }

    Ok(plugins)
}

fn append_deno_permissions(cmd: &mut Command, plugin: &LoadedPlugin) {
    append_deno_permission(cmd, "--allow-read", &plugin.permissions().read, &plugin.root_dir);
    append_deno_permission(cmd, "--allow-run", &plugin.permissions().run, &plugin.root_dir);
}

fn append_deno_permission(cmd: &mut Command, flag: &str, values: &[String], plugin_root: &Path) {
    if values.is_empty() {
        return;
    }

    if values.iter().any(|value| value.trim() == ".") {
        cmd.arg(flag);
        return;
    }

    let allowed = values
        .iter()
        .map(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return String::new();
            }

            if Path::new(trimmed).is_absolute() {
                trimmed.to_string()
            } else {
                plugin_root.join(trimmed).to_string_lossy().to_string()
            }
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(",");

    if !allowed.is_empty() {
        cmd.arg(format!("{flag}={allowed}"));
    }
}

impl LoadedPlugin {
    fn permissions(&self) -> &PluginPermissions {
        &self.manifest.permissions
    }
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn next_id(prefix: &str) -> String {
    let counter = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{counter}", now_millis())
}

fn chats_db_path() -> anyhow::Result<PathBuf> {
    let home = env::var("USERPROFILE")
        .or_else(|_| env::var("HOME"))
        .map(PathBuf::from)
        .context("failed to resolve USERPROFILE/HOME for chat sqlite path")?;
    let dir = home.join(".looper");
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create looper home at {}", dir.display()))?;
    Ok(dir.join("chats.sqlite"))
}

fn open_db(path: &Path) -> anyhow::Result<Connection> {
    Connection::open(path)
        .with_context(|| format!("failed to open sqlite db at {}", path.display()))
}

fn initialize_db(path: &Path) -> anyhow::Result<()> {
    let conn = open_db(path)?;
    conn.execute_batch(
        "BEGIN;
         CREATE TABLE IF NOT EXISTS sessions (
             id TEXT PRIMARY KEY,
             agent_id TEXT NOT NULL,
             origin TEXT NOT NULL,
             started_at INTEGER NOT NULL,
             ended_at INTEGER,
             metadata_json TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS events (
             id TEXT PRIMARY KEY,
             session_id TEXT NOT NULL,
             turn_id TEXT,
             event_kind TEXT NOT NULL,
             role TEXT,
             payload_json TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             FOREIGN KEY(session_id) REFERENCES sessions(id)
         );
         CREATE INDEX IF NOT EXISTS idx_events_session_created
             ON events(session_id, created_at);
         CREATE INDEX IF NOT EXISTS idx_sessions_agent_started
             ON sessions(agent_id, started_at);
         COMMIT;",
    )
    .context("failed to initialize chat sqlite schema")?;
    Ok(())
}
