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
    chat_plugin_path: PathBuf,
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

        let chat_plugin_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins")
            .join("internal-chat")
            .join("main.ts");

        Ok(Self {
            agent_id,
            db_path,
            chat_plugin_path,
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

        let plan = runtime.run_chat_plugin(ChatPluginPerceptInput {
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
            if let Some(system_prompt) = plan.system_prompt.clone() {
                if !system_prompt.trim().is_empty() {
                    session = session.with_system_prompt(system_prompt);
                }
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
        input: ChatPluginPerceptInput,
    ) -> anyhow::Result<ChatPluginPerceptPlan> {
        if !self.chat_plugin_path.exists() {
            bail!(
                "internal chat plugin is missing at {}",
                self.chat_plugin_path.display()
            );
        }

        let mut child = Command::new("deno")
            .arg("run")
            .arg("--quiet")
            .arg(&self.chat_plugin_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to start deno for internal chat plugin")?;

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
