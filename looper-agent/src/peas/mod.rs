use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Command, Stdio};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use async_stream::try_stream;
use fiddlesticks::{
    ChatEvent, ChatSession, ChatTurnRequest, build_provider_from_api_key, chat_service,
    parse_provider_id,
};
use futures_util::{Stream, StreamExt};
use globset::Glob;
use looper_common::{
    Effect, Percept, PlannedAction, PlannedActionStatus, SessionOrigin,
};
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
    plugins: Vec<LoadedPlugin>,
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
struct FilesystemActionOutcome {
    status: String,
    details: String,
    sensor_output: String,
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
                                    action.details =
                                        Some("No executor available for approved action".to_string());
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

        let chat_plugin = runtime.chat_plugin()?;
        let plan = runtime.run_chat_plugin(chat_plugin, ChatPluginPerceptInput {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            text: text.clone(),
        })?;

        if plan.mode != "stream_chat" {
            bail!("unsupported chat plugin mode: {}", plan.mode);
        }

        let mut prompt = plan.user_prompt.unwrap_or(text);
        let mut pre_effects = Vec::new();
        let mut sensor_notes = Vec::new();
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

            match runtime.execute_planned_action(&workspace_dir, action, PermissionMode::Enforce)? {
                Some(outcome) => {
                    sensor_notes.push(outcome.sensor_output.clone());
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
            .with_context(|| format!("failed to start deno for plugin '{}'", plugin.manifest.name))?;

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
            bail!("plugin '{}' execution failed: {stderr}", plugin.manifest.name);
        }

        let stdout = String::from_utf8(output.stdout).context("plugin emitted invalid utf8")?;
        let trimmed = stdout.trim();
        if trimmed.is_empty() {
            bail!("plugin '{}' returned empty output", plugin.manifest.name);
        }

        Ok(serde_json::from_str::<TOutput>(trimmed)?)
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
        workspace_dir: &str,
        action: &PlannedAction,
        permission_mode: PermissionMode,
    ) -> anyhow::Result<Option<FilesystemActionOutcome>> {
        let Some(plugin) = self.resolve_action_plugin(action) else {
            return Ok(None);
        };

        if action.actuator.starts_with("filesystem_") {
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

        Ok(None)
    }

    fn resolve_action_plugin(&self, action: &PlannedAction) -> Option<&LoadedPlugin> {
        if action.plugin != "auto"
            && let Some(plugin) = self.plugin_by_name(&action.plugin)
            && plugin
                .manifest
                .peas
                .actuators
                .iter()
                .any(|actuator| actuator.name == action.actuator)
        {
            return Some(plugin);
        }

        self.plugin_for_actuator(&action.actuator)
    }

    fn plugin_by_name(&self, plugin_name: &str) -> Option<&LoadedPlugin> {
        self.plugins
            .iter()
            .find(|plugin| plugin.manifest.name == plugin_name)
    }

    fn plugin_for_actuator(&self, actuator_name: &str) -> Option<&LoadedPlugin> {
        self.plugins.iter().find(|plugin| {
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
    ) -> anyhow::Result<FilesystemActionOutcome> {
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
            return Ok(FilesystemActionOutcome {
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
            "filesystem_grep" => run_native_grep(&workspace_root, &target_dir, &action.pattern, limit)?,
            "filesystem_glob" => run_native_glob(&workspace_root, &target_dir, &action.pattern, limit)?,
            "filesystem_read" => {
                let max_lines = action.max_lines.unwrap_or(250).clamp(1, 1000);
                run_native_read(&workspace_root, &target_dir, max_lines)?
            }
            other => {
                return Ok(FilesystemActionOutcome {
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

        Ok(FilesystemActionOutcome {
            status: outcome_status,
            details,
            sensor_output,
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
            format!("filesystem actuator filesystem_glob failed: target path {} does not exist", target_dir.display()),
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
            format!("filesystem actuator filesystem_grep failed: target path {} does not exist", target_dir.display()),
            "failed".to_string(),
        ));
    }

    let regex = Regex::new(pattern)
        .with_context(|| format!("invalid regex pattern '{pattern}'"))?;

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
    path
        .strip_prefix(workspace_root)
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
