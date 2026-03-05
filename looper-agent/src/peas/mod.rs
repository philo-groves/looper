use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use async_stream::try_stream;
use fiddlesticks::{
    ChatEvent, ChatSession, ChatTurnRequest, build_provider_from_api_key, chat_service,
    parse_provider_id,
};
use futures_util::{Stream, StreamExt};
use globset::Glob;
use looper_common::{Effect, Percept, PlannedAction, PlannedActionStatus, SessionOrigin};
use regex::Regex;
use rusqlite::{Connection, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

use crate::settings::AgentKeys;

const CHAT_DOMAIN: &str = "chat";
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct PeasRuntime {
    agent_id: String,
    db_path: PathBuf,
    builtin_plugins: Vec<LoadedPlugin>,
    pending_approvals: Arc<Mutex<HashMap<String, Vec<PendingApproval>>>>,
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
    actuator_executor: Option<String>,
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
    #[serde(default)]
    weight: Option<f64>,
    #[serde(default)]
    evaluation_mode: Option<String>,
    #[serde(default)]
    success_criteria: Vec<String>,
    #[serde(default)]
    rewards: Vec<PluginPerformanceReward>,
}

#[derive(Debug, Clone, Deserialize)]
struct PluginPerformanceReward {
    #[serde(default)]
    name: String,
    #[serde(default)]
    when: String,
    #[serde(default)]
    weight: Option<f64>,
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
    #[serde(default)]
    executor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct WorkspacePluginRegistry {
    #[serde(default)]
    plugins: Vec<WorkspacePluginState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspacePluginState {
    name: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    version: Option<String>,
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
    #[serde(default)]
    planned_actions: Vec<PlannedActionSpec>,
}

#[derive(Debug, Deserialize)]
struct ChatTaskCompletionOutput {
    status: String,
    details: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PlannedActionSpec {
    #[serde(default)]
    plugin: Option<String>,
    actuator: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Deserialize)]
struct FilesystemActionPlan {
    actuator: String,
    #[serde(default)]
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    max_lines: Option<usize>,
}

#[derive(Debug, Clone)]
struct ActionOutcome {
    status: String,
    details: String,
    sensor_output: String,
}

#[derive(Debug, Serialize)]
struct PluginActuatorInput {
    kind: String,
    actuator: String,
    args: Value,
    workspace_dir: String,
}

#[derive(Debug, Deserialize)]
struct PluginActuatorOutput {
    #[serde(default = "default_completed_status")]
    status: String,
    #[serde(default)]
    details: String,
    #[serde(default)]
    sensor_output: Option<String>,
}

#[derive(Debug, Clone)]
struct PendingApproval {
    action: PlannedAction,
    reason: String,
}

#[derive(Debug, Clone, Copy)]
enum PermissionMode {
    Enforce,
    AllowOneShot,
}

#[derive(Debug, Clone)]
enum ApprovalDecision {
    Approve { action_ids: HashSet<String> },
    Deny { action_ids: HashSet<String> },
}

#[derive(Debug, Clone)]
struct PerformanceMeasureContext {
    name: String,
    description: String,
    weight: f64,
    evaluation_mode: String,
    success_criteria: Vec<String>,
}

#[derive(Debug, Clone)]
struct PerformanceScoreTracker {
    measures_by_plugin: HashMap<String, Vec<PerformanceMeasureContext>>,
    total_score: f64,
    max_abs_score: f64,
    notes: Vec<String>,
}

fn default_mode() -> String {
    "stream_chat".to_string()
}

fn default_true() -> bool {
    true
}

fn default_completed_status() -> String {
    "completed".to_string()
}

type EffectStream = Pin<Box<dyn Stream<Item = anyhow::Result<Effect>> + Send>>;

impl PeasRuntime {
    pub fn new(agent_id: String) -> anyhow::Result<Self> {
        let db_path = chats_db_path()?;
        initialize_db(&db_path)?;

        let builtin_plugins = load_plugins(&Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins"))?;

        if builtin_plugins.is_empty() {
            bail!("no PEAS plugins were loaded");
        }

        Ok(Self {
            agent_id,
            db_path,
            builtin_plugins,
            pending_approvals: Arc::new(Mutex::new(HashMap::new())),
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

    pub fn install_workspace_plugin(
        &self,
        workspace_dir: &str,
        source: &str,
    ) -> anyhow::Result<String> {
        let source_path = PathBuf::from(source.trim());
        if source_path.as_os_str().is_empty() {
            bail!("plugin source path cannot be empty");
        }
        if !source_path.exists() {
            bail!(
                "plugin source path does not exist: {}",
                source_path.display()
            );
        }
        if !source_path.is_dir() {
            bail!(
                "plugin source must be a directory containing looper-plugin.json: {}",
                source_path.display()
            );
        }

        let plugin = load_plugin_from_dir(&source_path)?;
        if self
            .builtin_plugins
            .iter()
            .any(|entry| entry.manifest.name == plugin.manifest.name)
        {
            bail!(
                "cannot install plugin '{}' because a builtin plugin already uses that name",
                plugin.manifest.name
            );
        }
        let plugins_dir = workspace_plugins_dir(workspace_dir);
        fs::create_dir_all(&plugins_dir)
            .with_context(|| format!("failed to create {}", plugins_dir.display()))?;

        let destination = plugins_dir.join(&plugin.manifest.name);
        if destination.exists() {
            fs::remove_dir_all(&destination).with_context(|| {
                format!(
                    "failed to replace existing plugin directory {}",
                    destination.display()
                )
            })?;
        }

        copy_dir_recursive(&source_path, &destination)?;
        let _ = load_plugin_from_dir(&destination)?;

        let source_meta = fs::canonicalize(&source_path)
            .unwrap_or(source_path)
            .to_string_lossy()
            .to_string();
        upsert_workspace_plugin_registry(
            workspace_dir,
            &plugin.manifest.name,
            true,
            Some(source_meta),
            Some(plugin.manifest.version.clone()),
        )?;

        Ok(format!(
            "installed plugin '{}' v{} into {}",
            plugin.manifest.name,
            plugin.manifest.version,
            destination.display()
        ))
    }

    pub fn remove_workspace_plugin(
        &self,
        workspace_dir: &str,
        plugin_name: &str,
    ) -> anyhow::Result<String> {
        let trimmed = plugin_name.trim();
        if trimmed.is_empty() {
            bail!("plugin name cannot be empty");
        }

        if self
            .builtin_plugins
            .iter()
            .any(|plugin| plugin.manifest.name == trimmed)
        {
            bail!(
                "cannot remove builtin plugin '{}'; disable it instead",
                trimmed
            );
        }

        let plugin_dir = workspace_plugins_dir(workspace_dir).join(trimmed);
        if !plugin_dir.exists() {
            bail!("workspace plugin '{}' not found", trimmed);
        }

        fs::remove_dir_all(&plugin_dir)
            .with_context(|| format!("failed to remove {}", plugin_dir.display()))?;
        remove_workspace_plugin_registry_entry(workspace_dir, trimmed)?;

        Ok(format!("removed plugin '{}'", trimmed))
    }

    pub fn set_workspace_plugin_enabled(
        &self,
        workspace_dir: &str,
        plugin_name: &str,
        enabled: bool,
    ) -> anyhow::Result<String> {
        let trimmed = plugin_name.trim();
        if trimmed.is_empty() {
            bail!("plugin name cannot be empty");
        }

        let active = self.plugins_for_workspace(workspace_dir)?;
        let has_active = active.iter().any(|plugin| plugin.manifest.name == trimmed);
        let has_builtin = self
            .builtin_plugins
            .iter()
            .any(|plugin| plugin.manifest.name == trimmed);
        let has_external = workspace_plugins_dir(workspace_dir).join(trimmed).exists();

        if !has_active && !has_builtin && !has_external {
            bail!(
                "plugin '{}' is not installed for workspace {}",
                trimmed,
                workspace_dir
            );
        }

        upsert_workspace_plugin_registry(workspace_dir, trimmed, enabled, None, None)?;
        let status = if enabled { "enabled" } else { "disabled" };
        Ok(format!("plugin '{}' {status}", trimmed))
    }

    pub fn list_workspace_plugins(&self, workspace_dir: &str) -> anyhow::Result<String> {
        let all_plugins = self.plugins_with_registry(workspace_dir)?;
        if all_plugins.is_empty() {
            return Ok("no plugins available".to_string());
        }

        let mut lines = Vec::new();
        for (plugin, enabled, source) in all_plugins {
            lines.push(format!(
                "- {} v{} [{}] source={}",
                plugin.manifest.name,
                plugin.manifest.version,
                if enabled { "enabled" } else { "disabled" },
                source
            ));
        }

        Ok(lines.join("\n"))
    }

    pub fn catalog_external_plugins(&self) -> anyhow::Result<String> {
        let catalog_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("external-plugins");
        let catalog_plugins = load_plugins(&catalog_root)?;
        if catalog_plugins.is_empty() {
            return Ok("no bundled external plugins found".to_string());
        }

        let mut lines = vec![format!(
            "bundled plugin catalog ({})",
            catalog_root.display()
        )];
        for plugin in catalog_plugins {
            lines.push(format!(
                "- {} v{}: {}",
                plugin.manifest.name, plugin.manifest.version, plugin.manifest.description
            ));
            lines.push(format!(
                "  install: /plugin add {}",
                plugin.root_dir.display()
            ));
        }

        Ok(lines.join("\n"))
    }

    pub async fn stream_percept_effects(
        &self,
        session_id: &str,
        domain: &str,
        percept: Percept,
        workspace_dir: &str,
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
        let workspace_dir = workspace_dir.to_string();
        let runtime = self.clone();
        let active_plugins = runtime.plugins_for_workspace(&workspace_dir)?;

        let Percept::UserText { turn_id, text } = percept;
        runtime.append_event(
            &session_id,
            Some(turn_id.as_str()),
            "percept_user_text",
            Some("user"),
            &text,
        )?;

        let pending = runtime.take_pending_approvals(&session_id);
        if !pending.is_empty() {
            let Some(decision) = parse_approval_decision(&text, &pending) else {
                runtime.set_pending_approvals(&session_id, pending.clone());
                let pending_prompt = format_pending_approval_prompt(&pending);
                let stream = try_stream! {
                    yield Effect::ChatResponse {
                        turn_id: turn_id.clone(),
                        text: pending_prompt,
                    };
                };
                return Ok(Box::pin(stream));
            };

            let mut pending_by_id = pending
                .into_iter()
                .map(|entry| (entry.action.action_id.clone(), entry))
                .collect::<HashMap<_, _>>();

            let mut effects = Vec::new();
            let mut sensor_notes = Vec::new();
            let mut remaining = Vec::new();

            match decision {
                ApprovalDecision::Approve { action_ids } => {
                    for action_id in action_ids {
                        if let Some(entry) = pending_by_id.remove(&action_id) {
                            let mut action = entry.action;
                            action.status = PlannedActionStatus::InProgress;
                            action.details = Some("Action resumed after approval".to_string());
                            effects.push(Effect::ActionStatusChanged {
                                turn_id: turn_id.clone(),
                                action: action.clone(),
                            });

                            match runtime.execute_planned_action(
                                &active_plugins,
                                &workspace_dir,
                                &action,
                                PermissionMode::AllowOneShot,
                            )? {
                                Some(outcome) => {
                                    action.status = map_outcome_status(&outcome.status);
                                    action.details = Some(outcome.details.clone());
                                    effects.push(Effect::ActionStatusChanged {
                                        turn_id: turn_id.clone(),
                                        action,
                                    });
                                    sensor_notes.push(outcome.sensor_output);
                                }
                                None => {
                                    action.status = PlannedActionStatus::Skipped;
                                    action.details = Some(
                                        "No executor available for approved action".to_string(),
                                    );
                                    effects.push(Effect::ActionStatusChanged {
                                        turn_id: turn_id.clone(),
                                        action,
                                    });
                                }
                            }
                        }
                    }
                    remaining.extend(pending_by_id.into_values());
                }
                ApprovalDecision::Deny { action_ids } => {
                    for action_id in action_ids {
                        if let Some(entry) = pending_by_id.remove(&action_id) {
                            let mut action = entry.action;
                            action.status = PlannedActionStatus::Skipped;
                            action.details = Some("Action denied by user".to_string());
                            effects.push(Effect::ActionStatusChanged {
                                turn_id: turn_id.clone(),
                                action,
                            });
                        }
                    }
                    remaining.extend(pending_by_id.into_values());
                }
            }

            runtime.set_pending_approvals(&session_id, remaining.clone());

            let mut response = String::new();
            if !sensor_notes.is_empty() {
                response.push_str("Approved action results:\n\n");
                response.push_str(&sensor_notes.join("\n\n"));
            } else {
                response.push_str("Acknowledged. Updated pending actions.");
            }

            if !remaining.is_empty() {
                response.push_str("\n\n");
                response.push_str(&format_pending_approval_prompt(&remaining));
            }

            let stream = try_stream! {
                for effect in effects {
                    yield effect;
                }
                yield Effect::ChatResponse {
                    turn_id: turn_id.clone(),
                    text: response,
                };
            };
            return Ok(Box::pin(stream));
        }

        let chat_plugin = runtime.chat_plugin(&active_plugins)?;
        let plan = runtime.run_chat_plugin(
            chat_plugin,
            ChatPluginPerceptInput {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                text: text.clone(),
            },
        )?;

        if plan.mode != "stream_chat" {
            bail!("unsupported chat plugin mode: {}", plan.mode);
        }

        let mut prompt = plan.user_prompt.unwrap_or(text);
        let mut pre_effects = Vec::new();
        let mut sensor_notes = Vec::new();
        let mut performance_tracker = PerformanceScoreTracker::new(&active_plugins);
        let mut planned_actions = runtime.materialize_planned_actions(&plan.planned_actions);
        if !planned_actions.is_empty() {
            pre_effects.push(Effect::PlanUpdated {
                turn_id: turn_id.clone(),
                actions: planned_actions.clone(),
            });
        }

        for action in &mut planned_actions {
            action.status = PlannedActionStatus::InProgress;
            action.details = Some("Action started".to_string());
            pre_effects.push(Effect::ActionStatusChanged {
                turn_id: turn_id.clone(),
                action: action.clone(),
            });

            match runtime.execute_planned_action(
                &active_plugins,
                &workspace_dir,
                action,
                PermissionMode::Enforce,
            )? {
                Some(outcome) => {
                    sensor_notes.push(outcome.sensor_output.clone());
                    if let Some(plugin) = runtime.resolve_action_plugin(&active_plugins, action) {
                        performance_tracker.record(plugin, action, &outcome);
                    }
                    if outcome.status == "blocked" {
                        action.status = PlannedActionStatus::AwaitingApproval;
                        action.details = Some(outcome.details.clone());
                        runtime.push_pending_approval(
                            &session_id,
                            PendingApproval {
                                action: action.clone(),
                                reason: outcome.details,
                            },
                        );
                    } else {
                        action.status = map_outcome_status(&outcome.status);
                        action.details = Some(outcome.details.clone());
                    }
                    pre_effects.push(Effect::ActionStatusChanged {
                        turn_id: turn_id.clone(),
                        action: action.clone(),
                    });
                }
                None => {
                    action.status = PlannedActionStatus::Skipped;
                    action.details = Some("No executor available for action".to_string());
                    pre_effects.push(Effect::ActionStatusChanged {
                        turn_id: turn_id.clone(),
                        action: action.clone(),
                    });
                }
            }
        }

        if !sensor_notes.is_empty() {
            prompt = format!(
                "{prompt}\n\nPlugin sensor observations:\n{}\nUse these observations directly. If an action is blocked by permissions, ask the user for explicit per-action approval before requesting broader access.",
                sensor_notes.join("\n\n")
            );
        }

        let performance_summary = performance_tracker.summary();
        if let Some(payload) = performance_tracker.event_payload() {
            let _ = runtime.append_event(
                &session_id,
                Some(turn_id.as_str()),
                "performance_score",
                Some("system"),
                &payload,
            );
        }
        if !performance_summary.trim().is_empty() {
            prompt = format!(
                "{prompt}\n\nPerformance review:\n{performance_summary}\nUse this feedback to improve your next response and action choices."
            );
        }

        let turn_id_for_stream = turn_id.clone();

        let awaiting = runtime.take_pending_approvals(&session_id);
        if !awaiting.is_empty() {
            let approval_prompt = format_pending_approval_prompt(&awaiting);
            runtime.set_pending_approvals(&session_id, awaiting);
            let stream = try_stream! {
                for effect in pre_effects {
                    yield effect;
                }
                yield Effect::ChatResponse {
                    turn_id: turn_id_for_stream,
                    text: approval_prompt,
                };
            };
            return Ok(Box::pin(stream));
        }

        let stream = try_stream! {
            for effect in pre_effects {
                yield effect;
            }

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
            let full_system_prompt =
                runtime.build_chat_system_prompt(&active_plugins, plan.system_prompt.clone(), &workspace_dir);
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
            Effect::PlanUpdated { turn_id, actions } => {
                let payload = serde_json::to_string(actions)
                    .context("failed to serialize planned actions")?;
                self.append_event(
                    session_id,
                    Some(turn_id.as_str()),
                    "effect_plan_updated",
                    Some("assistant"),
                    &payload,
                )
            }
            Effect::ActionStatusChanged { turn_id, action } => {
                let payload = serde_json::to_string(action)
                    .context("failed to serialize planned action update")?;
                self.append_event(
                    session_id,
                    Some(turn_id.as_str()),
                    "effect_action_status_changed",
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
        self.run_plugin_with_input(plugin, &input)
            .context("chat plugin returned invalid json plan payload")
    }

    fn run_plugin_with_input<TInput, TOutput>(
        &self,
        plugin: &LoadedPlugin,
        input: &TInput,
    ) -> anyhow::Result<TOutput>
    where
        TInput: Serialize,
        TOutput: DeserializeOwned,
    {
        if !plugin.entry_path.exists() {
            bail!(
                "plugin '{}' entrypoint is missing at {}",
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
                format!("failed to start deno for plugin '{}'", plugin.manifest.name)
            })?;

        let input_json = serde_json::to_string(input).context("serialize plugin percept input")?;
        {
            let stdin = child
                .stdin
                .as_mut()
                .context("failed to open stdin for plugin")?;
            stdin
                .write_all(input_json.as_bytes())
                .context("failed to write plugin percept input")?;
        }

        let output = child
            .wait_with_output()
            .context("failed to wait for plugin process")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "plugin '{}' execution failed: {stderr}",
                plugin.manifest.name
            );
        }

        let stdout = String::from_utf8(output.stdout).context("plugin emitted invalid utf8")?;
        let trimmed = stdout.trim();
        if trimmed.is_empty() {
            bail!("plugin '{}' returned empty output", plugin.manifest.name);
        }

        Ok(serde_json::from_str::<TOutput>(trimmed)?)
    }

    fn chat_plugin<'a>(&self, plugins: &'a [LoadedPlugin]) -> anyhow::Result<&'a LoadedPlugin> {
        plugins
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

    fn materialize_planned_actions(&self, specs: &[PlannedActionSpec]) -> Vec<PlannedAction> {
        specs
            .iter()
            .enumerate()
            .map(|(index, spec)| PlannedAction {
                action_id: format!("act-{}-{}", now_millis(), index + 1),
                plugin: spec.plugin.clone().unwrap_or_else(|| "auto".to_string()),
                actuator: spec.actuator.clone(),
                args: spec.args.clone(),
                status: PlannedActionStatus::Planned,
                details: None,
            })
            .collect()
    }

    fn execute_planned_action(
        &self,
        plugins: &[LoadedPlugin],
        workspace_dir: &str,
        action: &PlannedAction,
        permission_mode: PermissionMode,
    ) -> anyhow::Result<Option<ActionOutcome>> {
        let Some(plugin) = self.resolve_action_plugin(plugins, action) else {
            return Ok(None);
        };

        let actuator_executor = plugin.actuator_executor(&action.actuator);
        if actuator_executor == "native_filesystem" {
            let fs_action = FilesystemActionPlan {
                actuator: action.actuator.clone(),
                pattern: action
                    .args
                    .get("pattern")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                path: action
                    .args
                    .get("path")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                max_results: action
                    .args
                    .get("max_results")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize),
                file_path: action
                    .args
                    .get("file_path")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                max_lines: action
                    .args
                    .get("max_lines")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize),
            };

            return self
                .execute_filesystem_action(plugin, workspace_dir, &fs_action, permission_mode)
                .map(Some);
        }

        if actuator_executor == "plugin_process" {
            return self
                .execute_plugin_actuator(plugin, workspace_dir, action)
                .map(Some);
        }

        Ok(Some(ActionOutcome {
            status: "skipped".to_string(),
            details: format!(
                "unsupported actuator executor '{}' for actuator '{}'",
                actuator_executor, action.actuator
            ),
            sensor_output: format!(
                "sensor plugin_command_error: unsupported actuator executor '{}' for {}",
                actuator_executor, action.actuator
            ),
        }))
    }

    fn resolve_action_plugin<'a>(
        &self,
        plugins: &'a [LoadedPlugin],
        action: &PlannedAction,
    ) -> Option<&'a LoadedPlugin> {
        if action.plugin != "auto"
            && let Some(plugin) = self.plugin_by_name(plugins, &action.plugin)
            && plugin
                .manifest
                .peas
                .actuators
                .iter()
                .any(|actuator| actuator.name == action.actuator)
        {
            return Some(plugin);
        }

        self.plugin_for_actuator(plugins, &action.actuator)
    }

    fn plugin_by_name<'a>(
        &self,
        plugins: &'a [LoadedPlugin],
        plugin_name: &str,
    ) -> Option<&'a LoadedPlugin> {
        plugins
            .iter()
            .find(|plugin| plugin.manifest.name == plugin_name)
    }

    fn plugin_for_actuator<'a>(
        &self,
        plugins: &'a [LoadedPlugin],
        actuator_name: &str,
    ) -> Option<&'a LoadedPlugin> {
        plugins.iter().find(|plugin| {
            plugin
                .manifest
                .peas
                .actuators
                .iter()
                .any(|actuator| actuator.name == actuator_name)
        })
    }

    fn execute_filesystem_action(
        &self,
        plugin: &LoadedPlugin,
        workspace_dir: &str,
        action: &FilesystemActionPlan,
        permission_mode: PermissionMode,
    ) -> anyhow::Result<ActionOutcome> {
        let workspace_root = PathBuf::from(workspace_dir);
        let requested_path = if action.actuator == "filesystem_read" {
            action
                .file_path
                .as_deref()
                .or(action.path.as_deref())
                .unwrap_or(action.pattern.as_str())
        } else {
            action.path.as_deref().unwrap_or(".")
        };
        let target_dir = resolve_requested_path(&workspace_root, requested_path);

        if matches!(permission_mode, PermissionMode::Enforce)
            && !is_allowed_read_path(plugin, &workspace_root, &target_dir)
        {
            let sensor_output = format!(
                "sensor filesystem_command_error: actuator={} blocked; requested path '{}' is outside allowed read roots {:?}. Ask the user for explicit per-action approval.",
                action.actuator,
                target_dir.display(),
                plugin.permissions().read
            );
            return Ok(ActionOutcome {
                status: "blocked".to_string(),
                details: format!(
                    "filesystem action blocked for path '{}'; ask user for per-action approval",
                    target_dir.display()
                ),
                sensor_output,
            });
        }
        let limit = action.max_results.unwrap_or(200).clamp(1, 500);

        let (stdout, stderr, details, outcome_status) = match action.actuator.as_str() {
            "filesystem_grep" => {
                run_native_grep(&workspace_root, &target_dir, &action.pattern, limit)?
            }
            "filesystem_glob" => {
                run_native_glob(&workspace_root, &target_dir, &action.pattern, limit)?
            }
            "filesystem_read" => {
                let max_lines = action.max_lines.unwrap_or(250).clamp(1, 1000);
                run_native_read(&workspace_root, &target_dir, max_lines)?
            }
            other => {
                return Ok(ActionOutcome {
                    status: "skipped".to_string(),
                    details: format!("unsupported filesystem actuator '{other}'"),
                    sensor_output: format!(
                        "sensor filesystem_command_error: unsupported actuator '{other}'"
                    ),
                });
            }
        };

        let (stdout_capped, stdout_truncated) = truncate_text(&stdout, 12_000);
        let (stderr_capped, stderr_truncated) = truncate_text(&stderr, 4_000);

        let mut sensor_output = format!(
            "sensor filesystem_command_output: actuator={} path={} pattern={}\nstdout:\n{}",
            action.actuator,
            target_dir.display(),
            action.pattern,
            if stdout_capped.trim().is_empty() {
                "(empty)"
            } else {
                stdout_capped.as_str()
            }
        );

        if !stderr_capped.trim().is_empty() {
            sensor_output.push_str("\n\nstderr:\n");
            sensor_output.push_str(&stderr_capped);
        }
        if stdout_truncated || stderr_truncated {
            sensor_output.push_str("\n\n(truncated output)");
        }

        Ok(ActionOutcome {
            status: outcome_status,
            details,
            sensor_output,
        })
    }

    fn execute_plugin_actuator(
        &self,
        plugin: &LoadedPlugin,
        workspace_dir: &str,
        action: &PlannedAction,
    ) -> anyhow::Result<ActionOutcome> {
        let output = self.run_plugin_with_input::<PluginActuatorInput, PluginActuatorOutput>(
            plugin,
            &PluginActuatorInput {
                kind: "actuator_execute".to_string(),
                actuator: action.actuator.clone(),
                args: action.args.clone(),
                workspace_dir: workspace_dir.to_string(),
            },
        )?;

        let PluginActuatorOutput {
            status,
            details,
            sensor_output,
        } = output;

        let details = if details.trim().is_empty() {
            format!(
                "plugin actuator {} completed with status {}",
                action.actuator, status
            )
        } else {
            details
        };

        let sensor_output = sensor_output.unwrap_or_else(|| {
            format!(
                "sensor plugin_command_complete: plugin={} actuator={} status={}",
                plugin.manifest.name, action.actuator, status
            )
        });

        Ok(ActionOutcome {
            status,
            details,
            sensor_output,
        })
    }

    fn build_chat_system_prompt(
        &self,
        plugins: &[LoadedPlugin],
        plugin_system_prompt: Option<String>,
        workspace_dir: &str,
    ) -> String {
        let mut sections = Vec::new();

        if let Some(system_prompt) = plugin_system_prompt {
            if !system_prompt.trim().is_empty() {
                sections.push(system_prompt);
            }
        }

        let context = self.build_component_context(plugins);
        if !context.is_empty() {
            sections.push(format!(
                "PEAS plugin context (available for consideration during this chat):\n{context}\nUse these components when relevant, and do not claim to have used a component unless it was actually invoked."
            ));
        }

        let performance_prompt = build_performance_prompt(plugins);
        if !performance_prompt.is_empty() {
            sections.push(performance_prompt);
        }

        if let Some(soul) = load_soul_prompt(workspace_dir) {
            sections.push(soul);
        }

        sections.join("\n\n")
    }

    fn build_component_context(&self, plugins: &[LoadedPlugin]) -> String {
        let mut lines = Vec::new();
        for plugin in plugins {
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

    fn plugins_for_workspace(&self, workspace_dir: &str) -> anyhow::Result<Vec<LoadedPlugin>> {
        let all_plugins = self.plugins_with_registry(workspace_dir)?;
        Ok(all_plugins
            .into_iter()
            .filter_map(|(plugin, enabled, _)| if enabled { Some(plugin) } else { None })
            .collect())
    }

    fn plugins_with_registry(
        &self,
        workspace_dir: &str,
    ) -> anyhow::Result<Vec<(LoadedPlugin, bool, String)>> {
        let mut plugins = self.builtin_plugins.clone();

        let external_root = workspace_plugins_dir(workspace_dir);
        if external_root.exists() {
            let mut external_plugins = load_plugins(&external_root)?;
            plugins.append(&mut external_plugins);
        }

        plugins.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
        validate_unique_plugin_names(&plugins)?;

        let registry_doc = load_workspace_plugin_registry_doc(workspace_dir)?;
        let enabled_map = registry_doc
            .plugins
            .iter()
            .map(|entry| (entry.name.clone(), entry.enabled))
            .collect::<HashMap<_, _>>();
        let source_map = registry_doc
            .plugins
            .into_iter()
            .map(|entry| {
                (
                    entry.name,
                    entry.source.unwrap_or_else(|| "workspace".to_string()),
                )
            })
            .collect::<HashMap<_, _>>();

        let mut result = Vec::new();
        for plugin in plugins {
            let enabled = enabled_map
                .get(&plugin.manifest.name)
                .copied()
                .unwrap_or(true);
            let source = source_map
                .get(&plugin.manifest.name)
                .cloned()
                .unwrap_or_else(|| {
                    if self
                        .builtin_plugins
                        .iter()
                        .any(|builtin| builtin.manifest.name == plugin.manifest.name)
                    {
                        "builtin".to_string()
                    } else {
                        "workspace".to_string()
                    }
                });
            result.push((plugin, enabled, source));
        }

        Ok(result)
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

    fn take_pending_approvals(&self, session_id: &str) -> Vec<PendingApproval> {
        let Ok(mut guard) = self.pending_approvals.lock() else {
            return Vec::new();
        };
        guard.remove(session_id).unwrap_or_default()
    }

    fn set_pending_approvals(&self, session_id: &str, pending: Vec<PendingApproval>) {
        if let Ok(mut guard) = self.pending_approvals.lock() {
            if pending.is_empty() {
                guard.remove(session_id);
            } else {
                guard.insert(session_id.to_string(), pending);
            }
        }
    }

    fn push_pending_approval(&self, session_id: &str, pending: PendingApproval) {
        if let Ok(mut guard) = self.pending_approvals.lock() {
            guard
                .entry(session_id.to_string())
                .or_insert_with(Vec::new)
                .push(pending);
        }
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

fn validate_unique_plugin_names(plugins: &[LoadedPlugin]) -> anyhow::Result<()> {
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
    Ok(())
}

fn workspace_plugins_dir(workspace_dir: &str) -> PathBuf {
    Path::new(workspace_dir).join(".looper").join("plugins")
}

fn workspace_plugin_registry_path(workspace_dir: &str) -> PathBuf {
    Path::new(workspace_dir)
        .join(".looper")
        .join("plugin-registry.json")
}

fn load_workspace_plugin_registry_doc(
    workspace_dir: &str,
) -> anyhow::Result<WorkspacePluginRegistry> {
    let registry_path = workspace_plugin_registry_path(workspace_dir);
    if !registry_path.exists() {
        return Ok(WorkspacePluginRegistry::default());
    }

    let text = fs::read_to_string(&registry_path)
        .with_context(|| format!("failed to read {}", registry_path.display()))?;
    let mut registry = serde_json::from_str::<WorkspacePluginRegistry>(&text)
        .with_context(|| format!("invalid plugin registry {}", registry_path.display()))?;
    registry
        .plugins
        .retain(|plugin| !plugin.name.trim().is_empty());
    Ok(registry)
}

fn save_workspace_plugin_registry_doc(
    workspace_dir: &str,
    registry: &WorkspacePluginRegistry,
) -> anyhow::Result<()> {
    let registry_path = workspace_plugin_registry_path(workspace_dir);
    if let Some(parent) = registry_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(registry)
        .context("failed to serialize workspace plugin registry")?;
    fs::write(&registry_path, text)
        .with_context(|| format!("failed to write {}", registry_path.display()))
}

fn upsert_workspace_plugin_registry(
    workspace_dir: &str,
    plugin_name: &str,
    enabled: bool,
    source: Option<String>,
    version: Option<String>,
) -> anyhow::Result<()> {
    let mut registry = load_workspace_plugin_registry_doc(workspace_dir)?;
    if let Some(existing) = registry
        .plugins
        .iter_mut()
        .find(|entry| entry.name == plugin_name)
    {
        existing.enabled = enabled;
        if let Some(source) = source {
            existing.source = Some(source);
        }
        if let Some(version) = version {
            existing.version = Some(version);
        }
    } else {
        registry.plugins.push(WorkspacePluginState {
            name: plugin_name.to_string(),
            enabled,
            source,
            version,
        });
    }

    registry.plugins.sort_by(|a, b| a.name.cmp(&b.name));
    save_workspace_plugin_registry_doc(workspace_dir, &registry)
}

fn remove_workspace_plugin_registry_entry(
    workspace_dir: &str,
    plugin_name: &str,
) -> anyhow::Result<()> {
    let mut registry = load_workspace_plugin_registry_doc(workspace_dir)?;
    let before = registry.plugins.len();
    registry
        .plugins
        .retain(|entry| entry.name.as_str() != plugin_name);
    if registry.plugins.len() == before {
        return Ok(());
    }

    save_workspace_plugin_registry_doc(workspace_dir, &registry)
}

fn load_plugin_from_dir(path: &Path) -> anyhow::Result<LoadedPlugin> {
    let manifest_path = path.join("looper-plugin.json");
    if !manifest_path.exists() {
        bail!(
            "plugin source is missing looper-plugin.json at {}",
            manifest_path.display()
        );
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

    Ok(LoadedPlugin {
        manifest_path,
        root_dir: path.to_path_buf(),
        entry_path,
        manifest,
    })
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("failed to create {}", destination.display()))?;

    for entry in fs::read_dir(source)
        .with_context(|| format!("failed to read source directory {}", source.display()))?
    {
        let entry = entry.context("failed to read source plugin directory entry")?;
        let path = entry.path();
        let target = destination.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else {
            fs::copy(&path, &target).with_context(|| {
                format!(
                    "failed to copy plugin file from {} to {}",
                    path.display(),
                    target.display()
                )
            })?;
        }
    }

    Ok(())
}

fn append_deno_permissions(cmd: &mut Command, plugin: &LoadedPlugin) {
    append_deno_permission(
        cmd,
        "--allow-read",
        &plugin.permissions().read,
        &plugin.root_dir,
    );
    append_deno_permission(
        cmd,
        "--allow-run",
        &plugin.permissions().run,
        &plugin.root_dir,
    );
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

    fn actuator_executor(&self, actuator_name: &str) -> &str {
        if let Some(component_executor) = self
            .manifest
            .peas
            .actuators
            .iter()
            .find(|component| component.name == actuator_name)
            .and_then(|component| component.executor.as_deref())
            .filter(|executor| !executor.trim().is_empty())
        {
            return component_executor;
        }

        if let Some(default_executor) = self
            .manifest
            .peas
            .actuator_executor
            .as_deref()
            .filter(|executor| !executor.trim().is_empty())
        {
            return default_executor;
        }

        if actuator_name.starts_with("filesystem_") {
            return "native_filesystem";
        }

        "plugin_process"
    }
}

fn build_performance_prompt(plugins: &[LoadedPlugin]) -> String {
    let tracker = PerformanceScoreTracker::new(plugins);
    if tracker.measures_by_plugin.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "Performance measures (weighted) to optimize while planning and responding:".to_string(),
    ];
    for (plugin_name, measures) in tracker.measures_by_plugin {
        lines.push(format!("- plugin '{plugin_name}':"));
        for measure in measures {
            lines.push(format!(
                "  - {} (weight {:.2}, mode {}): {}",
                measure.name, measure.weight, measure.evaluation_mode, measure.description
            ));
            if !measure.success_criteria.is_empty() {
                lines.push(format!(
                    "    criteria: {}",
                    measure.success_criteria.join(" | ")
                ));
            }
        }
    }
    lines.join("\n")
}

impl PerformanceScoreTracker {
    fn new(plugins: &[LoadedPlugin]) -> Self {
        let mut measures_by_plugin: HashMap<String, Vec<PerformanceMeasureContext>> =
            HashMap::new();
        for plugin in plugins {
            let mut measures = Vec::new();
            for entry in &plugin.manifest.peas.performance {
                let reward_weight_sum: f64 = entry
                    .rewards
                    .iter()
                    .map(|reward| {
                        let _ = (&reward.name, &reward.when);
                        reward.weight.unwrap_or(0.0)
                    })
                    .sum();

                let weight = entry
                    .weight
                    .or(if reward_weight_sum > 0.0 {
                        Some(reward_weight_sum)
                    } else {
                        None
                    })
                    .unwrap_or(1.0)
                    .clamp(0.1, 10.0);

                let mut criteria = entry.success_criteria.clone();
                for reward in &entry.rewards {
                    if !reward.when.trim().is_empty() {
                        criteria.push(reward.when.clone());
                    }
                }

                measures.push(PerformanceMeasureContext {
                    name: entry.name.clone(),
                    description: entry.description.clone(),
                    weight,
                    evaluation_mode: entry
                        .evaluation_mode
                        .clone()
                        .unwrap_or_else(|| "balanced".to_string()),
                    success_criteria: criteria,
                });
            }

            if !measures.is_empty() {
                measures_by_plugin.insert(plugin.manifest.name.clone(), measures);
            }
        }

        Self {
            measures_by_plugin,
            total_score: 0.0,
            max_abs_score: 0.0,
            notes: Vec::new(),
        }
    }

    fn record(&mut self, plugin: &LoadedPlugin, action: &PlannedAction, outcome: &ActionOutcome) {
        let status_factor = match outcome.status.as_str() {
            "completed" => 1.0,
            "failed" => -1.0,
            "blocked" => -0.5,
            "skipped" => 0.0,
            _ => -0.25,
        };

        let measures = self
            .measures_by_plugin
            .get(&plugin.manifest.name)
            .cloned()
            .unwrap_or_default();

        let weight_sum: f64 = measures.iter().map(|measure| measure.weight).sum();
        if weight_sum <= 0.0 {
            return;
        }

        let delta = weight_sum * status_factor;
        self.total_score += delta;
        self.max_abs_score += weight_sum;
        self.notes.push(format!(
            "{}::{} => status={} score_delta={:+.2}",
            plugin.manifest.name, action.actuator, outcome.status, delta
        ));
    }

    fn summary(&self) -> String {
        if self.notes.is_empty() {
            return String::new();
        }

        let mut lines = Vec::new();
        lines.push(format!(
            "turn_score={:+.2} / max={:.2}",
            self.total_score, self.max_abs_score
        ));
        lines.extend(self.notes.iter().map(|entry| format!("- {entry}")));
        lines.join("\n")
    }

    fn event_payload(&self) -> Option<String> {
        if self.notes.is_empty() {
            return None;
        }

        let payload = serde_json::json!({
            "turn_score": self.total_score,
            "max_abs_score": self.max_abs_score,
            "notes": self.notes,
        });
        Some(payload.to_string())
    }
}

fn map_outcome_status(status: &str) -> PlannedActionStatus {
    match status {
        "completed" => PlannedActionStatus::Completed,
        "failed" => PlannedActionStatus::Failed,
        "blocked" => PlannedActionStatus::Blocked,
        "skipped" => PlannedActionStatus::Skipped,
        _ => PlannedActionStatus::Failed,
    }
}

fn parse_approval_decision(text: &str, pending: &[PendingApproval]) -> Option<ApprovalDecision> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let pending_ids = pending
        .iter()
        .map(|entry| entry.action.action_id.clone())
        .collect::<HashSet<_>>();

    let lowered = trimmed.to_ascii_lowercase();
    if lowered == "approve all" || lowered == "approve" {
        return Some(ApprovalDecision::Approve {
            action_ids: pending_ids,
        });
    }
    if lowered == "deny all" || lowered == "deny" {
        return Some(ApprovalDecision::Deny {
            action_ids: pending
                .iter()
                .map(|entry| entry.action.action_id.clone())
                .collect(),
        });
    }

    if pending.len() == 1 {
        if matches!(lowered.as_str(), "yes" | "y" | "ok" | "okay" | "sure") {
            return Some(ApprovalDecision::Approve {
                action_ids: pending
                    .iter()
                    .map(|entry| entry.action.action_id.clone())
                    .collect(),
            });
        }
        if matches!(lowered.as_str(), "no" | "n" | "cancel") {
            return Some(ApprovalDecision::Deny {
                action_ids: pending
                    .iter()
                    .map(|entry| entry.action.action_id.clone())
                    .collect(),
            });
        }
    }

    if lowered.starts_with("approve ") {
        let ids = parse_action_ids(&trimmed[8..], &pending_ids);
        if !ids.is_empty() {
            return Some(ApprovalDecision::Approve { action_ids: ids });
        }
    }

    if lowered.starts_with("deny ") {
        let ids = parse_action_ids(&trimmed[5..], &pending_ids);
        if !ids.is_empty() {
            return Some(ApprovalDecision::Deny { action_ids: ids });
        }
    }

    None
}

fn parse_action_ids(raw: &str, pending_ids: &HashSet<String>) -> HashSet<String> {
    raw.split([',', ' '])
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .filter(|token| pending_ids.contains(*token))
        .map(ToString::to_string)
        .collect()
}

fn format_pending_approval_prompt(pending: &[PendingApproval]) -> String {
    let mut lines = vec![
        "I need your approval to continue with actions outside current plugin read permissions."
            .to_string(),
        "Approve one or more actions with `approve <action_id>` (or `approve all`).".to_string(),
        "Deny with `deny <action_id>` (or `deny all`).".to_string(),
        String::new(),
        "Pending actions:".to_string(),
    ];

    for entry in pending {
        lines.push(format!(
            "- {} {} ({})",
            entry.action.action_id, entry.action.actuator, entry.reason
        ));
    }

    lines.join("\n")
}

fn load_soul_prompt(workspace_dir: &str) -> Option<String> {
    let soul_path = Path::new(workspace_dir).join("SOUL.md");
    let content = fs::read_to_string(&soul_path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (snippet, truncated) = truncate_text(trimmed, 10_000);
    let mut prompt = format!(
        "Optional workspace SOUL overlay (from SOUL.md):\n{}\nUse this as secondary style/ethics guidance. Prioritize active performance measures and explicit user instructions.",
        snippet
    );
    if truncated {
        prompt.push_str("\n(SOUL.md content truncated for prompt size limits.)");
    }
    Some(prompt)
}

fn resolve_requested_path(workspace_root: &Path, requested_path: &str) -> PathBuf {
    let requested = Path::new(requested_path.trim());
    if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        workspace_root.join(requested)
    }
}

fn is_allowed_read_path(plugin: &LoadedPlugin, workspace_root: &Path, target_path: &Path) -> bool {
    let read = &plugin.permissions().read;
    if read.iter().any(|entry| entry.trim() == ".") {
        let target_abs = canonicalize_for_check(target_path);
        let workspace_abs = canonicalize_for_check(workspace_root);
        return target_abs.starts_with(workspace_abs);
    }

    let target_abs = canonicalize_for_check(target_path);
    read.iter()
        .map(|entry| entry.trim())
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            if Path::new(entry).is_absolute() {
                PathBuf::from(entry)
            } else {
                workspace_root.join(entry)
            }
        })
        .map(|root| canonicalize_for_check(&root))
        .any(|allowed_root| target_abs.starts_with(allowed_root))
}

fn canonicalize_for_check(path: &Path) -> PathBuf {
    if let Ok(abs) = fs::canonicalize(path) {
        return abs;
    }

    if let Some(parent) = path.parent()
        && let Ok(parent_abs) = fs::canonicalize(parent)
    {
        if let Some(name) = path.file_name() {
            return parent_abs.join(name);
        }
        return parent_abs;
    }

    path.to_path_buf()
}

fn run_native_glob(
    workspace_root: &Path,
    target_dir: &Path,
    pattern: &str,
    limit: usize,
) -> anyhow::Result<(String, String, String, String)> {
    if !target_dir.exists() {
        return Ok((
            String::new(),
            format!("target path does not exist: {}", target_dir.display()),
            format!(
                "filesystem actuator filesystem_glob failed: target path {} does not exist",
                target_dir.display()
            ),
            "failed".to_string(),
        ));
    }

    let glob = Glob::new(pattern)
        .with_context(|| format!("invalid glob pattern '{pattern}'"))?
        .compile_matcher();

    let mut matches = Vec::new();
    for entry in WalkDir::new(target_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let rel_to_target = path
            .strip_prefix(target_dir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();

        if !glob.is_match(&rel_to_target) && !glob.is_match(file_name) {
            continue;
        }

        matches.push(display_path(workspace_root, path));
        if matches.len() >= limit {
            break;
        }
    }

    let stdout = matches.join("\n");
    let details = if matches.is_empty() {
        "filesystem actuator filesystem_glob completed with no matches".to_string()
    } else {
        format!(
            "filesystem actuator filesystem_glob completed with {} matches",
            matches.len()
        )
    };

    Ok((stdout, String::new(), details, "completed".to_string()))
}

fn run_native_grep(
    workspace_root: &Path,
    target_dir: &Path,
    pattern: &str,
    limit: usize,
) -> anyhow::Result<(String, String, String, String)> {
    if !target_dir.exists() {
        return Ok((
            String::new(),
            format!("target path does not exist: {}", target_dir.display()),
            format!(
                "filesystem actuator filesystem_grep failed: target path {} does not exist",
                target_dir.display()
            ),
            "failed".to_string(),
        ));
    }

    let regex =
        Regex::new(pattern).with_context(|| format!("invalid regex pattern '{pattern}'"))?;

    let mut matches = Vec::new();
    for entry in WalkDir::new(target_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        if bytes.contains(&0) {
            continue;
        }

        let text = String::from_utf8_lossy(&bytes);
        for (idx, line) in text.lines().enumerate() {
            if !regex.is_match(line) {
                continue;
            }

            matches.push(format!(
                "{}:{}:{}",
                display_path(workspace_root, path),
                idx + 1,
                line
            ));
            if matches.len() >= limit {
                break;
            }
        }

        if matches.len() >= limit {
            break;
        }
    }

    let stdout = matches.join("\n");
    let details = if matches.is_empty() {
        "filesystem actuator filesystem_grep completed with no matches".to_string()
    } else {
        format!(
            "filesystem actuator filesystem_grep completed with {} matches",
            matches.len()
        )
    };

    Ok((stdout, String::new(), details, "completed".to_string()))
}

fn run_native_read(
    workspace_root: &Path,
    target_path: &Path,
    max_lines: usize,
) -> anyhow::Result<(String, String, String, String)> {
    if !target_path.exists() {
        return Ok((
            String::new(),
            format!("target file does not exist: {}", target_path.display()),
            format!(
                "filesystem actuator filesystem_read failed: target file {} does not exist",
                target_path.display()
            ),
            "failed".to_string(),
        ));
    }

    if !target_path.is_file() {
        return Ok((
            String::new(),
            format!("target path is not a file: {}", target_path.display()),
            format!(
                "filesystem actuator filesystem_read failed: target path {} is not a file",
                target_path.display()
            ),
            "failed".to_string(),
        ));
    }

    let bytes = fs::read(target_path)
        .with_context(|| format!("failed to read file {}", target_path.display()))?;
    if bytes.contains(&0) {
        return Ok((
            String::new(),
            format!(
                "target file appears to be binary and cannot be read as text: {}",
                target_path.display()
            ),
            "filesystem actuator filesystem_read failed: file is not text".to_string(),
            "failed".to_string(),
        ));
    }

    let content = String::from_utf8_lossy(&bytes).to_string();
    let all_lines = content.lines().collect::<Vec<_>>();
    let truncated = all_lines.len() > max_lines;
    let displayed = all_lines
        .iter()
        .take(max_lines)
        .enumerate()
        .map(|(idx, line)| format!("{}: {}", idx + 1, line))
        .collect::<Vec<_>>()
        .join("\n");

    let stderr = if truncated {
        format!(
            "output truncated: showing {} of {} lines",
            max_lines,
            all_lines.len()
        )
    } else {
        String::new()
    };
    let details = format!(
        "filesystem actuator filesystem_read completed for {}",
        display_path(workspace_root, target_path)
    );

    Ok((displayed, stderr, details, "completed".to_string()))
}

fn display_path(workspace_root: &Path, path: &Path) -> String {
    path.strip_prefix(workspace_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn truncate_text(text: &str, max_chars: usize) -> (String, bool) {
    let chars = text.chars().count();
    if chars <= max_chars {
        return (text.to_string(), false);
    }

    let capped = text.chars().take(max_chars).collect::<String>();
    (capped, true)
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
