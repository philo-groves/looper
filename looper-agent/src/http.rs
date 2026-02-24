use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use fiddlesticks::{ProviderId, list_models_with_api_key};
use notify::{
    Config as NotifyConfig, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::select;
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::dto::{ActuatorCreateRequest, PluginImportRequest, SensorCreateRequest};
use crate::model::{
    Actuator, ActuatorType, AgentState, ExecutionResult, ModelProviderKind, ModelSelection,
    RateLimit, SensorIngressConfig, SensorRestFormat,
};
use crate::runtime::{
    ActuatorUpdate, LoopVisualizationSnapshot, LooperRuntime, ObservabilitySnapshot, SensorUpdate,
};
use crate::storage::{PersistedChatMessage, PersistedChatSession, PersistedIteration};

const DEFAULT_LOOP_INTERVAL_MS: u64 = 500;

const DEFAULT_SOUL_MARKDOWN: &str = r#"# Looper Soul

## Identity
Looper is a curious, practical, and upbeat general-purpose agent.
Looper likes solving real problems end-to-end and keeping momentum high.

## Personality
- Friendly and direct
- Creative when ideating, precise when executing
- Calm under uncertainty
- Enjoys pairing with humans and explaining tradeoffs

## Working Style
1. Understand intent, constraints, and definition of done.
2. Propose a clear approach before deep changes.
3. Execute in small, verifiable steps.
4. Validate with tests, checks, and concrete evidence.
5. Report outcomes, risks, and sensible next moves.

## Communication
- Keep responses concise by default.
- Use plain language.
- Surface assumptions and edge cases early.
- Be honest about uncertainty.

## Engineering Principles
- Prefer readable, maintainable solutions over clever shortcuts.
- Preserve existing conventions unless there is a strong reason to change.
- Treat reliability and safety as first-class requirements.
- Minimize blast radius for each change.

## Collaboration Promises
Looper will:
- Ask only truly blocking questions.
- Share progress frequently on larger work.
- Make it easy to review by referencing specific files/commands.
- Leave the codebase cleaner than it found it.

## Mission
Help people perform general tasks with confidence, speed, and a little delight."#;

/// Shared HTTP state for Looper API handlers.
#[derive(Clone)]
pub struct AppState {
    /// Shared runtime instance.
    pub runtime: Arc<Mutex<LooperRuntime>>,
    loop_control: Arc<Mutex<LoopControl>>,
    directory_watchers: Arc<Mutex<HashMap<String, DirectoryWatcherHandle>>>,
    loop_configuration: Arc<Mutex<LoopConfiguration>>,
    loop_configuration_path: PathBuf,
}

impl AppState {
    /// Creates application state from a runtime.
    pub fn new(runtime: LooperRuntime) -> Self {
        let workspace_root = runtime.workspace_root().to_path_buf();
        let loop_configuration_path = workspace_root.join("loop-configuration.json");
        let loop_configuration = load_loop_configuration(&loop_configuration_path)
            .unwrap_or_else(|_| LoopConfiguration::default());

        Self {
            runtime: Arc::new(Mutex::new(runtime)),
            loop_control: Arc::new(Mutex::new(LoopControl::default())),
            directory_watchers: Arc::new(Mutex::new(HashMap::new())),
            loop_configuration: Arc::new(Mutex::new(loop_configuration)),
            loop_configuration_path,
        }
    }
}

/// Builds API router for sensor/actuator registration and loop operations.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/sensors", post(add_sensor_handler))
        .route(
            "/api/sensors/{sensor_name}/percepts",
            post(add_sensor_percept_handler),
        )
        .route("/api/actuators", post(add_actuator_handler))
        .route("/api/plugins/import", post(import_plugin_handler))
        .route("/api/percepts/chat", post(add_chat_percept_handler))
        .route("/api/chats", get(list_chat_sessions_handler))
        .route(
            "/api/chats/{chat_id}/messages",
            get(list_chat_messages_handler),
        )
        .route("/api/config/keys", post(register_api_key_handler))
        .route("/api/config/models", post(configure_models_handler))
        .route("/api/metrics", get(metrics_handler))
        .route("/api/health", get(health_handler))
        .route("/api/loop/start", post(loop_start_handler))
        .route("/api/loop/stop", post(loop_stop_handler))
        .route("/api/loop/status", get(loop_status_handler))
        .route("/api/state", get(state_handler))
        .route("/api/dashboard", get(dashboard_handler))
        .route("/api/agent/identity", get(agent_identity_handler))
        .route("/api/agent/soul", post(save_soul_handler))
        .route("/api/agent/skills", post(save_skill_handler))
        .route(
            "/api/agent/skills/from-url",
            post(save_skill_from_url_handler),
        )
        .route("/api/agent/skills/delete", post(delete_skill_handler))
        .route("/api/agent/skills/get", post(get_skill_handler))
        .route("/api/ws", get(websocket_handler))
        .route("/api/iterations", get(list_iterations_handler))
        .route("/api/iterations/{id}", get(get_iteration_handler))
        .route("/api/approvals", get(list_approvals_handler))
        .route("/api/approvals/{id}/approve", post(approve_handler))
        .route("/api/approvals/{id}/deny", post(deny_handler))
        .with_state(state)
}

/// Basic health payload.
#[derive(Clone, Debug, Serialize)]
pub struct HealthResponse {
    /// Health status value.
    pub status: &'static str,
}

/// Returns process health.
pub async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

/// Handles sensor registration.
pub async fn add_sensor_handler(
    State(state): State<AppState>,
    Json(request): Json<SensorCreateRequest>,
) -> Result<(StatusCode, Json<MutationResponse>), (StatusCode, Json<ApiError>)> {
    let sensor = request.into_sensor();
    let name = sensor.name.clone();

    let mut runtime = state.runtime.lock().await;
    runtime
        .register_sensor(sensor)
        .map_err(|error| bad_request(error.to_string()))?;
    drop(runtime);

    sync_directory_watchers(&state)
        .await
        .map_err(|error| internal_error(error.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(MutationResponse {
            status: "ok".to_string(),
            name,
        }),
    ))
}

/// Handles actuator registration.
pub async fn add_actuator_handler(
    State(state): State<AppState>,
    Json(request): Json<ActuatorCreateRequest>,
) -> Result<(StatusCode, Json<MutationResponse>), (StatusCode, Json<ApiError>)> {
    let actuator = request
        .try_into_actuator()
        .map_err(|error| bad_request(error.to_string()))?;
    let name = actuator.name.clone();

    let mut runtime = state.runtime.lock().await;
    runtime
        .register_actuator(actuator)
        .map_err(|error| bad_request(error.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(MutationResponse {
            status: "ok".to_string(),
            name,
        }),
    ))
}

/// Imports a plugin package from disk and registers declared sensors/actuators.
pub async fn import_plugin_handler(
    State(state): State<AppState>,
    Json(request): Json<PluginImportRequest>,
) -> Result<(StatusCode, Json<MutationResponse>), (StatusCode, Json<ApiError>)> {
    let mut runtime = state.runtime.lock().await;
    let name = runtime
        .import_plugin_package(request.path)
        .map_err(|error| bad_request(error.to_string()))?;
    drop(runtime);

    sync_directory_watchers(&state)
        .await
        .map_err(|error| internal_error(error.to_string()))?;

    Ok((
        StatusCode::CREATED,
        Json(MutationResponse {
            status: "ok".to_string(),
            name,
        }),
    ))
}

/// Ingests a chat percept.
pub async fn add_chat_percept_handler(
    State(state): State<AppState>,
    Json(request): Json<ChatPerceptRequest>,
) -> Result<(StatusCode, Json<SimpleStatusResponse>), (StatusCode, Json<ApiError>)> {
    let mut runtime = state.runtime.lock().await;
    let chat_id = runtime
        .enqueue_chat_message(request.message, request.chat_id)
        .map_err(|error| bad_request(error.to_string()))?;

    Ok((
        StatusCode::ACCEPTED,
        Json(SimpleStatusResponse {
            status: format!("accepted:{chat_id}"),
        }),
    ))
}

/// Ingests one percept for a configured sensor endpoint.
pub async fn add_sensor_percept_handler(
    Path(sensor_name): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<SimpleStatusResponse>), (StatusCode, Json<ApiError>)> {
    let content_type = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let mut runtime = state.runtime.lock().await;
    let sensor = runtime
        .sensors()
        .into_iter()
        .find(|item| item.name == sensor_name)
        .ok_or_else(|| bad_request(format!("sensor '{}' not found", sensor_name)))?;

    let payload = match sensor.ingress {
        SensorIngressConfig::Internal => {
            return Err(bad_request(format!(
                "sensor '{}' is managed internally",
                sensor.name
            )));
        }
        SensorIngressConfig::Directory { .. } => {
            return Err(bad_request(format!(
                "sensor '{}' receives percepts from directory watcher",
                sensor.name
            )));
        }
        SensorIngressConfig::Plugin(_) => {
            return Err(bad_request(format!(
                "sensor '{}' receives percepts from plugin polling",
                sensor.name
            )));
        }
        SensorIngressConfig::RestApi {
            format: SensorRestFormat::Text,
        } => {
            if !content_type.is_empty()
                && !content_type.starts_with("text/plain")
                && !content_type.starts_with("text/markdown")
            {
                return Err(bad_request(
                    "expected text/plain or text/markdown content-type".to_string(),
                ));
            }
            let text = String::from_utf8(body.to_vec())
                .map_err(|_| bad_request("text payload must be valid UTF-8".to_string()))?;
            text.trim().to_string()
        }
        SensorIngressConfig::RestApi {
            format: SensorRestFormat::Json,
        } => {
            if !content_type.starts_with("application/json") {
                return Err(bad_request(
                    "expected application/json content-type".to_string(),
                ));
            }
            let value: serde_json::Value = serde_json::from_slice(&body)
                .map_err(|error| bad_request(format!("invalid json payload: {error}")))?;
            serde_json::to_string(&value)
                .map_err(|error| bad_request(format!("invalid json payload: {error}")))?
        }
    };

    runtime
        .enqueue_sensor_percept(&sensor_name, payload)
        .map_err(|error| bad_request(error.to_string()))?;

    Ok((
        StatusCode::ACCEPTED,
        Json(SimpleStatusResponse {
            status: "accepted".to_string(),
        }),
    ))
}

/// Query payload for listing chat sessions.
#[derive(Clone, Debug, Deserialize)]
pub struct ChatSessionListQuery {
    /// Maximum number of sessions to return.
    pub limit: Option<usize>,
}

/// Response payload for listing chat sessions.
#[derive(Clone, Debug, Serialize)]
pub struct ChatSessionsResponse {
    /// Ordered chat sessions.
    pub chats: Vec<PersistedChatSession>,
}

/// Lists chat sessions persisted by the agent.
pub async fn list_chat_sessions_handler(
    State(state): State<AppState>,
    Query(query): Query<ChatSessionListQuery>,
) -> Result<Json<ChatSessionsResponse>, (StatusCode, Json<ApiError>)> {
    let runtime = state.runtime.lock().await;
    let chats = runtime
        .list_chat_sessions(query.limit.unwrap_or(100).clamp(1, 500))
        .map_err(|error| internal_error(error.to_string()))?;
    Ok(Json(ChatSessionsResponse { chats }))
}

/// Query payload for listing chat messages.
#[derive(Clone, Debug, Deserialize)]
pub struct ChatMessageListQuery {
    /// Return messages with id greater than this value.
    pub after_id: Option<i64>,
    /// Maximum number of messages.
    pub limit: Option<usize>,
}

/// Response payload for listing chat messages.
#[derive(Clone, Debug, Serialize)]
pub struct ChatMessagesResponse {
    /// Ordered message list.
    pub messages: Vec<PersistedChatMessage>,
}

/// Lists chat messages for one session.
pub async fn list_chat_messages_handler(
    Path(chat_id): Path<String>,
    State(state): State<AppState>,
    Query(query): Query<ChatMessageListQuery>,
) -> Result<Json<ChatMessagesResponse>, (StatusCode, Json<ApiError>)> {
    let runtime = state.runtime.lock().await;
    let messages = runtime
        .list_chat_messages(
            &chat_id,
            query.after_id,
            query.limit.unwrap_or(200).clamp(1, 1000),
        )
        .map_err(|error| internal_error(error.to_string()))?;
    Ok(Json(ChatMessagesResponse { messages }))
}

/// Returns current runtime metrics.
pub async fn metrics_handler(
    State(state): State<AppState>,
) -> Result<Json<ObservabilitySnapshot>, (StatusCode, Json<ApiError>)> {
    let runtime = state.runtime.lock().await;
    Ok(Json(runtime.observability_snapshot()))
}

/// Registers a provider API key.
pub async fn register_api_key_handler(
    State(state): State<AppState>,
    Json(request): Json<ApiKeyRequest>,
) -> Result<Json<SimpleStatusResponse>, (StatusCode, Json<ApiError>)> {
    let mut runtime = state.runtime.lock().await;
    runtime
        .register_api_key(request.provider, request.api_key)
        .map_err(|error| bad_request(error.to_string()))?;
    Ok(Json(SimpleStatusResponse {
        status: "ok".to_string(),
    }))
}

/// Configures local and frontier model selections.
pub async fn configure_models_handler(
    State(state): State<AppState>,
    Json(request): Json<ModelConfigRequest>,
) -> Result<Json<SimpleStatusResponse>, (StatusCode, Json<ApiError>)> {
    {
        let mut runtime = state.runtime.lock().await;
        runtime
            .configure_models(request.local, request.frontier)
            .map_err(|error| bad_request(error.to_string()))?;
    }

    loop_start_impl(&state, configured_loop_interval_ms(&state).await)
        .await
        .map_err(|error| bad_request(error.to_string()))?;

    Ok(Json(SimpleStatusResponse {
        status: "ok".to_string(),
    }))
}

/// Starts the background loop when persisted configuration is already available.
pub async fn auto_start_loop_if_configured(
    state: &AppState,
) -> anyhow::Result<Option<LoopStatusResponse>> {
    let is_configured = {
        let runtime = state.runtime.lock().await;
        runtime.is_configured()
    };

    if !is_configured {
        return Ok(None);
    }

    let status = loop_start_impl(state, configured_loop_interval_ms(state).await).await?;
    Ok(Some(status))
}

/// Starts sensor directory watchers for configured directory-based sensors.
pub async fn initialize_sensor_ingress(state: &AppState) -> anyhow::Result<()> {
    sync_directory_watchers(state).await
}

/// Starts continuous loop execution.
pub async fn loop_start_handler(
    State(state): State<AppState>,
    Json(request): Json<LoopStartRequest>,
) -> Result<Json<LoopStatusResponse>, (StatusCode, Json<ApiError>)> {
    let interval_ms = request
        .interval_ms
        .unwrap_or(configured_loop_interval_ms(&state).await);
    let status = loop_start_impl(&state, interval_ms)
        .await
        .map_err(|error| {
            let message = error.to_string();
            bad_request(message)
        })?;
    Ok(Json(status))
}

/// Stops continuous loop execution.
pub async fn loop_stop_handler(
    State(state): State<AppState>,
) -> Result<Json<LoopStatusResponse>, (StatusCode, Json<ApiError>)> {
    let join_handle = {
        let mut loop_control = state.loop_control.lock().await;
        if !loop_control.running {
            return Ok(Json(loop_control.status_response()));
        }

        if let Some(stop_sender) = loop_control.stop_sender.take() {
            let _ = stop_sender.send(());
        }
        loop_control.running = false;
        loop_control.join_handle.take()
    };

    if let Some(handle) = join_handle {
        let _ = handle.await;
    }

    let mut runtime = state.runtime.lock().await;
    runtime.stop("manually stopped");

    let loop_control = state.loop_control.lock().await;
    Ok(Json(loop_control.status_response()))
}

/// Returns loop status.
pub async fn loop_status_handler(
    State(state): State<AppState>,
) -> Result<Json<LoopStatusResponse>, (StatusCode, Json<ApiError>)> {
    let loop_control = state.loop_control.lock().await;
    Ok(Json(loop_control.status_response()))
}

/// Returns high-level agent state.
pub async fn state_handler(
    State(state): State<AppState>,
) -> Result<Json<AgentStateResponse>, (StatusCode, Json<ApiError>)> {
    let runtime = state.runtime.lock().await;
    let latest_iteration_id = runtime
        .latest_iteration_id()
        .map_err(|error| internal_error(error.to_string()))?;
    Ok(Json(AgentStateResponse {
        state: runtime.state(),
        reason: runtime.stop_reason().map(str::to_string),
        configured: runtime.is_configured(),
        local_selection: runtime.local_selection().cloned(),
        frontier_selection: runtime.frontier_selection().cloned(),
        latest_iteration_id,
    }))
}

/// Returns dashboard snapshot for the web interface.
pub async fn dashboard_handler(
    State(state): State<AppState>,
) -> Result<Json<DashboardResponse>, (StatusCode, Json<ApiError>)> {
    let snapshot = dashboard_snapshot(&state)
        .await
        .map_err(|error| internal_error(error.to_string()))?;
    Ok(Json(snapshot))
}

/// Returns Soul markdown and available skills.
pub async fn agent_identity_handler(
    State(state): State<AppState>,
) -> Result<Json<AgentIdentityResponse>, (StatusCode, Json<ApiError>)> {
    let identity = {
        let runtime = state.runtime.lock().await;
        read_agent_identity(&runtime).map_err(|error| internal_error(error.to_string()))?
    };
    Ok(Json(identity))
}

/// Replaces the current Soul markdown.
pub async fn save_soul_handler(
    State(state): State<AppState>,
    Json(request): Json<SaveSoulRequest>,
) -> Result<Json<SimpleStatusResponse>, (StatusCode, Json<ApiError>)> {
    let runtime = state.runtime.lock().await;
    write_soul_markdown(&runtime, &request.markdown)
        .map_err(|error| bad_request(error.to_string()))?;
    Ok(Json(SimpleStatusResponse {
        status: "ok".to_string(),
    }))
}

/// Creates or updates one skill markdown file.
pub async fn save_skill_handler(
    State(state): State<AppState>,
    Json(request): Json<SaveSkillRequest>,
) -> Result<Json<SkillDocumentResponse>, (StatusCode, Json<ApiError>)> {
    let runtime = state.runtime.lock().await;
    let skill =
        save_skill_document(&runtime, request).map_err(|error| bad_request(error.to_string()))?;
    Ok(Json(skill))
}

/// Fetches skill markdown from URL and saves it.
pub async fn save_skill_from_url_handler(
    State(state): State<AppState>,
    Json(request): Json<SaveSkillFromUrlRequest>,
) -> Result<Json<SkillDocumentResponse>, (StatusCode, Json<ApiError>)> {
    let response = reqwest::get(&request.url)
        .await
        .map_err(|error| bad_request(format!("failed to fetch url: {error}")))?;
    if !response.status().is_success() {
        return Err(bad_request(format!(
            "failed to fetch url: status {}",
            response.status()
        )));
    }
    let markdown = response
        .text()
        .await
        .map_err(|error| bad_request(format!("failed to read url body: {error}")))?;

    let runtime = state.runtime.lock().await;
    let saved = save_skill_document(
        &runtime,
        SaveSkillRequest {
            id: None,
            name: request.name,
            markdown,
        },
    )
    .map_err(|error| bad_request(error.to_string()))?;
    Ok(Json(saved))
}

/// Deletes one skill markdown file.
pub async fn delete_skill_handler(
    State(state): State<AppState>,
    Json(request): Json<DeleteSkillRequest>,
) -> Result<Json<SimpleStatusResponse>, (StatusCode, Json<ApiError>)> {
    let runtime = state.runtime.lock().await;
    delete_skill_file(&runtime, &request.id).map_err(|error| bad_request(error.to_string()))?;
    Ok(Json(SimpleStatusResponse {
        status: "ok".to_string(),
    }))
}

/// Returns one skill markdown file.
pub async fn get_skill_handler(
    State(state): State<AppState>,
    Json(request): Json<GetSkillRequest>,
) -> Result<Json<SkillDocumentResponse>, (StatusCode, Json<ApiError>)> {
    let runtime = state.runtime.lock().await;
    let skill = get_skill_document(&runtime, &request.id)
        .map_err(|error| bad_request(error.to_string()))?;
    Ok(Json(skill))
}

/// Returns one persisted iteration.
pub async fn get_iteration_handler(
    Path(id): Path<i64>,
    State(state): State<AppState>,
) -> Result<Json<PersistedIteration>, (StatusCode, Json<ApiError>)> {
    let runtime = state.runtime.lock().await;
    let iteration = runtime
        .get_iteration(id)
        .map_err(|error| internal_error(error.to_string()))?;

    match iteration {
        Some(item) => Ok(Json(item)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: format!("iteration {id} was not found"),
            }),
        )),
    }
}

/// Query payload for listing iterations.
#[derive(Clone, Debug, Deserialize)]
pub struct IterationListQuery {
    /// Return iterations with id greater than this value.
    pub after_id: Option<i64>,
    /// Maximum number of iterations to return.
    pub limit: Option<usize>,
}

/// Response payload for listing iterations.
#[derive(Clone, Debug, Serialize)]
pub struct IterationsResponse {
    /// Ordered list of persisted iterations.
    pub iterations: Vec<PersistedIteration>,
}

/// Lists persisted iterations, optionally filtered by id.
pub async fn list_iterations_handler(
    State(state): State<AppState>,
    Query(query): Query<IterationListQuery>,
) -> Result<Json<IterationsResponse>, (StatusCode, Json<ApiError>)> {
    let limit = query.limit.unwrap_or(50).clamp(1, 500);
    let runtime = state.runtime.lock().await;
    let iterations = runtime
        .list_iterations_after(query.after_id, limit)
        .map_err(|error| internal_error(error.to_string()))?;

    Ok(Json(IterationsResponse { iterations }))
}

/// Lists currently pending approvals.
pub async fn list_approvals_handler(
    State(state): State<AppState>,
) -> Result<Json<ApprovalsResponse>, (StatusCode, Json<ApiError>)> {
    let runtime = state.runtime.lock().await;
    Ok(Json(ApprovalsResponse {
        approvals: runtime.pending_approvals(),
    }))
}

/// Approves a pending action.
pub async fn approve_handler(
    Path(id): Path<u64>,
    State(state): State<AppState>,
) -> Result<Json<ApprovalDecisionResponse>, (StatusCode, Json<ApiError>)> {
    let mut runtime = state.runtime.lock().await;
    let result = runtime
        .approve(id)
        .map_err(|error| internal_error(error.to_string()))?;

    match result {
        Some(execution_result) => Ok(Json(ApprovalDecisionResponse {
            status: "approved".to_string(),
            execution_result: Some(execution_result),
        })),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: format!("approval {id} was not found"),
            }),
        )),
    }
}

/// Denies a pending action.
pub async fn deny_handler(
    Path(id): Path<u64>,
    State(state): State<AppState>,
) -> Result<Json<ApprovalDecisionResponse>, (StatusCode, Json<ApiError>)> {
    let mut runtime = state.runtime.lock().await;
    if runtime.deny(id) {
        return Ok(Json(ApprovalDecisionResponse {
            status: "denied".to_string(),
            execution_result: None,
        }));
    }

    Err((
        StatusCode::NOT_FOUND,
        Json(ApiError {
            error: format!("approval {id} was not found"),
        }),
    ))
}

/// Request payload for posting one chat percept.
#[derive(Clone, Debug, Deserialize)]
pub struct ChatPerceptRequest {
    /// Chat message content.
    pub message: String,
    /// Optional chat session id.
    pub chat_id: Option<String>,
}

/// One skill file summary.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SkillSummary {
    /// Skill identifier (filename).
    pub id: String,
    /// Skill display name.
    pub name: String,
    /// Last modified timestamp in unix millis.
    pub updated_at_unix_ms: i64,
}

/// Agent identity payload containing soul markdown and skills.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentIdentityResponse {
    /// Current Soul markdown content.
    pub soul_markdown: String,
    /// Available skill files.
    pub skills: Vec<SkillSummary>,
}

/// Request payload for replacing Soul markdown.
#[derive(Clone, Debug, Deserialize)]
pub struct SaveSoulRequest {
    /// New soul markdown content.
    pub markdown: String,
}

/// Request payload for creating/updating a skill.
#[derive(Clone, Debug, Deserialize)]
pub struct SaveSkillRequest {
    /// Optional existing skill id for updates.
    pub id: Option<String>,
    /// Optional skill name for creates.
    pub name: Option<String>,
    /// Skill markdown content.
    pub markdown: String,
}

/// Request payload for creating a skill from URL.
#[derive(Clone, Debug, Deserialize)]
pub struct SaveSkillFromUrlRequest {
    /// Source URL to fetch markdown from.
    pub url: String,
    /// Optional skill name override.
    pub name: Option<String>,
}

/// Request payload for deleting a skill.
#[derive(Clone, Debug, Deserialize)]
pub struct DeleteSkillRequest {
    /// Skill id (filename).
    pub id: String,
}

/// Request payload for fetching one skill.
#[derive(Clone, Debug, Deserialize)]
pub struct GetSkillRequest {
    /// Skill id (filename).
    pub id: String,
}

/// Response payload for one full skill document.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SkillDocumentResponse {
    /// Skill identifier (filename).
    pub id: String,
    /// Skill display name.
    pub name: String,
    /// Skill markdown content.
    pub markdown: String,
}

/// Request payload for loop start.
#[derive(Clone, Debug, Deserialize)]
pub struct LoopStartRequest {
    /// Optional delay between loop iterations.
    pub interval_ms: Option<u64>,
}

/// Response payload for loop configuration settings.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LoopConfigurationResponse {
    /// Default loop interval in milliseconds.
    pub interval_ms: u64,
}

/// Response payload for loop status.
#[derive(Clone, Debug, Serialize)]
pub struct LoopStatusResponse {
    /// Whether the loop task is running.
    pub running: bool,
    /// Current loop interval in milliseconds.
    pub interval_ms: u64,
}

/// Request payload for registering an API key.
#[derive(Clone, Debug, Deserialize)]
pub struct ApiKeyRequest {
    /// Provider receiving the key.
    pub provider: ModelProviderKind,
    /// API key value.
    pub api_key: String,
}

/// Request payload for configuring local/frontier models.
#[derive(Clone, Debug, Deserialize)]
pub struct ModelConfigRequest {
    /// Local model selection.
    pub local: ModelSelection,
    /// Frontier model selection.
    pub frontier: ModelSelection,
}

/// Agent state response payload.
#[derive(Clone, Debug, Serialize)]
pub struct AgentStateResponse {
    /// Current agent state.
    pub state: AgentState,
    /// Optional stop reason.
    pub reason: Option<String>,
    /// Whether required model config is present.
    pub configured: bool,
    /// Current local selection.
    pub local_selection: Option<ModelSelection>,
    /// Current frontier selection.
    pub frontier_selection: Option<ModelSelection>,
    /// Latest persisted iteration id.
    pub latest_iteration_id: Option<i64>,
}

/// Dashboard payload for the web interface.
#[derive(Clone, Debug, Serialize)]
pub struct DashboardResponse {
    /// Runtime state details.
    pub state: AgentStateResponse,
    /// Loop task details.
    pub loop_status: LoopStatusResponse,
    /// Runtime observability metrics.
    pub observability: ObservabilitySnapshot,
    /// Current loop step and branch state for visualization.
    pub loop_visualization: LoopVisualizationSnapshot,
    /// Current local model process details.
    pub local_model: ModelProcessStatus,
    /// Current frontier model process details.
    pub frontier_model: ModelProcessStatus,
    /// Registered sensors and queue health.
    pub sensors: Vec<SensorStatus>,
    /// Registered actuators and policy details.
    pub actuators: Vec<ActuatorStatus>,
    /// Pending human approvals.
    pub pending_approval_count: usize,
}

/// One model process status for the dashboard.
#[derive(Clone, Debug, Serialize)]
pub struct ModelProcessStatus {
    /// Whether a model selection is configured.
    pub configured: bool,
    /// Optional provider name.
    pub provider: Option<ModelProviderKind>,
    /// Optional model identifier.
    pub model: Option<String>,
    /// Derived lifecycle status for display.
    pub process_state: String,
}

/// Sensor details displayed on the dashboard.
#[derive(Clone, Debug, Serialize)]
pub struct SensorStatus {
    /// Sensor name.
    pub name: String,
    /// Sensor description.
    pub description: String,
    /// Whether this sensor is enabled.
    pub enabled: bool,
    /// Surprise sensitivity score (0-100).
    pub sensitivity_score: u8,
    /// Number of percepts retained.
    pub queued_percepts: usize,
    /// Number of unread percepts.
    pub unread_percepts: usize,
    /// Singular percept item name.
    pub percept_singular_name: String,
    /// Plural percept item name.
    pub percept_plural_name: String,
    /// Sensor ingress configuration.
    pub ingress: SensorIngressConfig,
}

/// Actuator details displayed on the dashboard.
#[derive(Clone, Debug, Serialize)]
pub struct ActuatorStatus {
    /// Actuator name.
    pub name: String,
    /// Actuator description.
    pub description: String,
    /// Actuator kind label.
    pub kind: String,
    /// Whether this actuator requires human approval.
    pub require_hitl: bool,
    /// Whether this actuator is sandboxed.
    pub sandboxed: bool,
    /// Number of allowlisted action keywords.
    pub allowlist_count: usize,
    /// Number of denylisted action keywords.
    pub denylist_count: usize,
    /// Optional rate limit policy.
    pub rate_limit: Option<RateLimit>,
    /// Singular action item name.
    pub action_singular_name: String,
    /// Plural action item name.
    pub action_plural_name: String,
}

/// Successful mutation response payload.
#[derive(Clone, Debug, Serialize)]
pub struct MutationResponse {
    /// Status indicator.
    pub status: String,
    /// Resource name.
    pub name: String,
}

/// Simple status response payload.
#[derive(Clone, Debug, Serialize)]
pub struct SimpleStatusResponse {
    /// Status indicator.
    pub status: String,
}

/// Approval list response payload.
#[derive(Clone, Debug, Serialize)]
pub struct ApprovalsResponse {
    /// Pending approvals.
    pub approvals: Vec<crate::model::PendingApproval>,
}

/// Approval decision response payload.
#[derive(Clone, Debug, Serialize)]
pub struct ApprovalDecisionResponse {
    /// Decision status.
    pub status: String,
    /// Optional execution result for approvals.
    pub execution_result: Option<ExecutionResult>,
}

/// Error response payload.
#[derive(Clone, Debug, Serialize)]
pub struct ApiError {
    /// Human-readable error message.
    pub error: String,
}

#[derive(Debug, Deserialize)]
struct WsRequestMessage {
    id: Option<u64>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum WsServerMessage {
    #[serde(rename = "response")]
    Response {
        id: Option<u64>,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    #[serde(rename = "event")]
    Event { event: &'static str, data: Value },
}

#[derive(Clone, Debug, Deserialize)]
struct WsLoopStartParams {
    interval_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
struct WsUpdateLoopConfigurationParams {
    interval_ms: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct WsChatMessagesParams {
    chat_id: String,
    after_id: Option<i64>,
    limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
struct WsListProviderModelsParams {
    provider: ModelProviderKind,
    api_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct WsOllamaModelVersionsParams {
    model: String,
}

#[derive(Clone, Debug, Deserialize)]
struct WsUpdateSensorParams {
    name: String,
    enabled: Option<bool>,
    sensitivity_score: Option<u8>,
    description: Option<String>,
    percept_singular_name: Option<String>,
    percept_plural_name: Option<String>,
    ingress: Option<SensorIngressConfig>,
}

#[derive(Clone, Debug, Deserialize)]
struct WsUpdateActuatorParams {
    name: String,
    description: Option<String>,
    require_hitl: Option<bool>,
    sandboxed: Option<bool>,
    rate_limit: Option<Option<RateLimit>>,
    action_singular_name: Option<String>,
    action_plural_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct WsImportPluginParams {
    path: String,
}

/// Upgrades to a websocket session for realtime bidirectional updates.
pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| websocket_session(socket, state))
}

async fn websocket_session(mut socket: WebSocket, state: AppState) {
    let mut snapshot_ticker = tokio::time::interval(Duration::from_millis(1000));
    let mut phase_ticker = tokio::time::interval(Duration::from_millis(150));
    let mut last_phase_sequence = {
        let runtime = state.runtime.lock().await;
        runtime.latest_phase_event_sequence()
    };

    if let Ok(snapshot) = dashboard_snapshot(&state).await {
        let payload = WsServerMessage::Event {
            event: "dashboard_snapshot",
            data: serde_json::to_value(snapshot).unwrap_or(Value::Null),
        };
        if send_ws_message(&mut socket, payload).await.is_err() {
            return;
        }
    }

    loop {
        select! {
            _ = snapshot_ticker.tick() => {
                match dashboard_snapshot(&state).await {
                    Ok(snapshot) => {
                        let payload = WsServerMessage::Event {
                            event: "dashboard_snapshot",
                            data: serde_json::to_value(snapshot).unwrap_or(Value::Null),
                        };
                        if send_ws_message(&mut socket, payload).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        break;
                    }
                }
            }
            _ = phase_ticker.tick() => {
                let phase_events = {
                    let runtime = state.runtime.lock().await;
                    runtime.loop_phase_events_since(last_phase_sequence)
                };

                for event in phase_events {
                    last_phase_sequence = event.sequence;
                    let payload = WsServerMessage::Event {
                        event: "loop_phase_transition",
                        data: serde_json::to_value(event).unwrap_or(Value::Null),
                    };
                    if send_ws_message(&mut socket, payload).await.is_err() {
                        return;
                    }
                }
            }
            incoming = socket.recv() => {
                let Some(Ok(message)) = incoming else {
                    break;
                };

                let Message::Text(text) = message else {
                    continue;
                };

                let response = match serde_json::from_str::<WsRequestMessage>(&text) {
                    Ok(request) => ws_dispatch_request(&state, request).await,
                    Err(error) => WsServerMessage::Response {
                        id: None,
                        ok: false,
                        result: None,
                        error: Some(format!("invalid request: {error}")),
                    },
                };

                if send_ws_message(&mut socket, response).await.is_err() {
                    break;
                }
            }
        }
    }
}

async fn send_ws_message(socket: &mut WebSocket, payload: WsServerMessage) -> anyhow::Result<()> {
    let encoded = serde_json::to_string(&payload)?;
    socket.send(Message::Text(encoded.into())).await?;
    Ok(())
}

async fn ws_dispatch_request(state: &AppState, request: WsRequestMessage) -> WsServerMessage {
    let result = ws_handle_request(state, &request.method, request.params).await;
    match result {
        Ok(value) => WsServerMessage::Response {
            id: request.id,
            ok: true,
            result: Some(value),
            error: None,
        },
        Err(error) => WsServerMessage::Response {
            id: request.id,
            ok: false,
            result: None,
            error: Some(error),
        },
    }
}

async fn ws_handle_request(state: &AppState, method: &str, params: Value) -> Result<Value, String> {
    match method {
        "health" => Ok(serde_json::json!({ "status": "ok" })),
        "state" => {
            let runtime = state.runtime.lock().await;
            let latest_iteration_id = runtime
                .latest_iteration_id()
                .map_err(|error| error.to_string())?;
            Ok(serde_json::to_value(AgentStateResponse {
                state: runtime.state(),
                reason: runtime.stop_reason().map(str::to_string),
                configured: runtime.is_configured(),
                local_selection: runtime.local_selection().cloned(),
                frontier_selection: runtime.frontier_selection().cloned(),
                latest_iteration_id,
            })
            .map_err(|error| error.to_string())?)
        }
        "metrics" => {
            let runtime = state.runtime.lock().await;
            Ok(serde_json::to_value(runtime.observability_snapshot())
                .map_err(|error| error.to_string())?)
        }
        "loop_status" => {
            let loop_control = state.loop_control.lock().await;
            Ok(serde_json::to_value(loop_control.status_response())
                .map_err(|error| error.to_string())?)
        }
        "loop_start" => {
            let payload: WsLoopStartParams =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let status = loop_start_impl(
                state,
                payload
                    .interval_ms
                    .unwrap_or(configured_loop_interval_ms(state).await),
            )
                .await
                .map_err(|error| error.to_string())?;
            Ok(serde_json::to_value(status).map_err(|error| error.to_string())?)
        }
        "enqueue_chat_message" => {
            let payload: ChatPerceptRequest =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let mut runtime = state.runtime.lock().await;
            let chat_id = runtime
                .enqueue_chat_message(payload.message, payload.chat_id)
                .map_err(|error| error.to_string())?;
            Ok(serde_json::json!({ "status": "accepted", "chat_id": chat_id }))
        }
        "register_api_key" => {
            let payload: ApiKeyRequest =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let mut runtime = state.runtime.lock().await;
            runtime
                .register_api_key(payload.provider, payload.api_key)
                .map_err(|error| error.to_string())?;
            Ok(serde_json::json!({ "status": "ok" }))
        }
        "configure_models" => {
            let payload: ModelConfigRequest =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            {
                let mut runtime = state.runtime.lock().await;
                runtime
                    .configure_models(payload.local, payload.frontier)
                    .map_err(|error| error.to_string())?;
            }
            loop_start_impl(state, configured_loop_interval_ms(state).await)
                .await
                .map_err(|error| error.to_string())?;
            Ok(serde_json::json!({ "status": "ok" }))
        }
        "get_loop_configuration" => {
            let config = state.loop_configuration.lock().await;
            Ok(serde_json::to_value(LoopConfigurationResponse {
                interval_ms: config.interval_ms,
            })
            .map_err(|error| error.to_string())?)
        }
        "update_loop_configuration" => {
            let payload: WsUpdateLoopConfigurationParams =
                serde_json::from_value(params).map_err(|error| error.to_string())?;

            let updated_interval = normalize_loop_interval_ms(payload.interval_ms);

            {
                let mut config = state.loop_configuration.lock().await;
                config.interval_ms = updated_interval;
                persist_loop_configuration(&state.loop_configuration_path, &config)
                    .map_err(|error| error.to_string())?;
            }

            Ok(serde_json::to_value(LoopConfigurationResponse {
                interval_ms: updated_interval,
            })
            .map_err(|error| error.to_string())?)
        }
        "update_sensor" => {
            let payload: WsUpdateSensorParams =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let mut runtime = state.runtime.lock().await;
            runtime
                .update_sensor(
                    &payload.name,
                    SensorUpdate {
                        enabled: payload.enabled,
                        sensitivity_score: payload.sensitivity_score,
                        description: payload.description,
                        percept_singular_name: payload.percept_singular_name,
                        percept_plural_name: payload.percept_plural_name,
                        ingress: payload.ingress,
                    },
                )
                .map_err(|error| error.to_string())?;
            drop(runtime);
            sync_directory_watchers(state)
                .await
                .map_err(|error| error.to_string())?;
            Ok(serde_json::json!({ "status": "ok" }))
        }
        "update_actuator" => {
            let payload: WsUpdateActuatorParams =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let mut runtime = state.runtime.lock().await;
            runtime
                .update_actuator(
                    &payload.name,
                    ActuatorUpdate {
                        description: payload.description,
                        require_hitl: payload.require_hitl,
                        sandboxed: payload.sandboxed,
                        rate_limit: payload.rate_limit,
                        action_singular_name: payload.action_singular_name,
                        action_plural_name: payload.action_plural_name,
                    },
                )
                .map_err(|error| error.to_string())?;
            Ok(serde_json::json!({ "status": "ok" }))
        }
        "import_plugin" => {
            let payload: WsImportPluginParams =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            {
                let mut runtime = state.runtime.lock().await;
                runtime
                    .import_plugin_package(payload.path)
                    .map_err(|error| error.to_string())?;
            }
            sync_directory_watchers(state)
                .await
                .map_err(|error| error.to_string())?;
            Ok(serde_json::json!({ "status": "ok" }))
        }
        "list_provider_models" => {
            let payload: WsListProviderModelsParams =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let models = list_provider_models(payload.provider, payload.api_key)
                .await
                .map_err(|error| error.to_string())?;
            Ok(serde_json::json!({ "models": models }))
        }
        "list_ollama_base_models" => {
            let models = scrape_ollama_library_base_models()
                .await
                .map_err(|error| error.to_string())?;
            Ok(serde_json::json!({ "models": models }))
        }
        "list_ollama_model_versions" => {
            let payload: WsOllamaModelVersionsParams =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let versions = scrape_ollama_model_versions(&payload.model)
                .await
                .map_err(|error| error.to_string())?;
            Ok(serde_json::json!({ "versions": versions }))
        }
        "list_iterations" => {
            let payload: IterationListQuery =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let limit = payload.limit.unwrap_or(50).clamp(1, 500);
            let runtime = state.runtime.lock().await;
            let iterations = runtime
                .list_iterations_after(payload.after_id, limit)
                .map_err(|error| error.to_string())?;
            Ok(serde_json::to_value(IterationsResponse { iterations })
                .map_err(|error| error.to_string())?)
        }
        "list_chat_sessions" => {
            let payload: ChatSessionListQuery =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let runtime = state.runtime.lock().await;
            let chats = runtime
                .list_chat_sessions(payload.limit.unwrap_or(100).clamp(1, 500))
                .map_err(|error| error.to_string())?;
            Ok(serde_json::to_value(ChatSessionsResponse { chats })
                .map_err(|error| error.to_string())?)
        }
        "list_chat_messages" => {
            let payload: WsChatMessagesParams =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let runtime = state.runtime.lock().await;
            let messages = runtime
                .list_chat_messages(
                    &payload.chat_id,
                    payload.after_id,
                    payload.limit.unwrap_or(200).clamp(1, 1000),
                )
                .map_err(|error| error.to_string())?;
            Ok(serde_json::to_value(ChatMessagesResponse { messages })
                .map_err(|error| error.to_string())?)
        }
        "get_agent_identity" => {
            let runtime = state.runtime.lock().await;
            let identity = read_agent_identity(&runtime).map_err(|error| error.to_string())?;
            Ok(serde_json::to_value(identity).map_err(|error| error.to_string())?)
        }
        "save_soul_markdown" => {
            let payload: SaveSoulRequest =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let runtime = state.runtime.lock().await;
            write_soul_markdown(&runtime, &payload.markdown).map_err(|error| error.to_string())?;
            Ok(serde_json::json!({ "status": "ok" }))
        }
        "save_skill" => {
            let payload: SaveSkillRequest =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let runtime = state.runtime.lock().await;
            let skill =
                save_skill_document(&runtime, payload).map_err(|error| error.to_string())?;
            Ok(serde_json::to_value(skill).map_err(|error| error.to_string())?)
        }
        "save_skill_from_url" => {
            let payload: SaveSkillFromUrlRequest =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let response = reqwest::get(&payload.url)
                .await
                .map_err(|error| format!("failed to fetch url: {error}"))?;
            if !response.status().is_success() {
                return Err(format!("failed to fetch url: status {}", response.status()));
            }
            let markdown = response
                .text()
                .await
                .map_err(|error| format!("failed to read url body: {error}"))?;
            let runtime = state.runtime.lock().await;
            let skill = save_skill_document(
                &runtime,
                SaveSkillRequest {
                    id: None,
                    name: payload.name,
                    markdown,
                },
            )
            .map_err(|error| error.to_string())?;
            Ok(serde_json::to_value(skill).map_err(|error| error.to_string())?)
        }
        "delete_skill" => {
            let payload: DeleteSkillRequest =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let runtime = state.runtime.lock().await;
            delete_skill_file(&runtime, &payload.id).map_err(|error| error.to_string())?;
            Ok(serde_json::json!({ "status": "ok" }))
        }
        "get_skill" => {
            let payload: GetSkillRequest =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let runtime = state.runtime.lock().await;
            let skill =
                get_skill_document(&runtime, &payload.id).map_err(|error| error.to_string())?;
            Ok(serde_json::to_value(skill).map_err(|error| error.to_string())?)
        }
        _ => Err(format!("unsupported method '{method}'")),
    }
}

#[derive(Default)]
struct LoopControl {
    running: bool,
    interval_ms: u64,
    stop_sender: Option<oneshot::Sender<()>>,
    join_handle: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LoopConfiguration {
    interval_ms: u64,
}

impl Default for LoopConfiguration {
    fn default() -> Self {
        Self {
            interval_ms: DEFAULT_LOOP_INTERVAL_MS,
        }
    }
}

fn normalize_loop_interval_ms(raw: u64) -> u64 {
    raw.clamp(1, 60_000)
}

fn load_loop_configuration(path: &PathBuf) -> anyhow::Result<LoopConfiguration> {
    if !path.exists() {
        return Ok(LoopConfiguration::default());
    }

    let raw = fs::read_to_string(path)?;
    let parsed = serde_json::from_str::<LoopConfigurationResponse>(&raw)?;
    Ok(LoopConfiguration {
        interval_ms: normalize_loop_interval_ms(parsed.interval_ms),
    })
}

fn persist_loop_configuration(path: &PathBuf, config: &LoopConfiguration) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let payload = LoopConfigurationResponse {
        interval_ms: normalize_loop_interval_ms(config.interval_ms),
    };
    let encoded = serde_json::to_string_pretty(&payload)?;
    fs::write(path, encoded)?;
    Ok(())
}

async fn configured_loop_interval_ms(state: &AppState) -> u64 {
    let config = state.loop_configuration.lock().await;
    normalize_loop_interval_ms(config.interval_ms)
}

impl LoopControl {
    fn status_response(&self) -> LoopStatusResponse {
        LoopStatusResponse {
            running: self.running,
            interval_ms: self.interval_ms,
        }
    }
}

async fn loop_start_impl(state: &AppState, interval_ms: u64) -> anyhow::Result<LoopStatusResponse> {
    {
        let mut runtime = state.runtime.lock().await;
        runtime.start()?;
    }

    {
        let loop_control = state.loop_control.lock().await;
        if loop_control.running {
            return Ok(loop_control.status_response());
        }
    }

    let runtime = state.runtime.clone();
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    let state_for_task = state.clone();

    let join_handle = tokio::spawn(async move {
        loop {
            select! {
                _ = &mut stop_rx => {
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(interval_ms)) => {
                    let run_result = {
                        let mut runtime_guard = runtime.lock().await;
                        runtime_guard.run_iteration().await
                    };

                    if run_result.is_err() {
                        tokio::time::sleep(Duration::from_millis(0)).await;
                    }
                }
            }
        }

        let mut loop_control = state_for_task.loop_control.lock().await;
        loop_control.running = false;
        loop_control.join_handle = None;
        loop_control.stop_sender = None;
    });

    let mut loop_control = state.loop_control.lock().await;
    loop_control.running = true;
    loop_control.interval_ms = interval_ms;
    loop_control.stop_sender = Some(stop_tx);
    loop_control.join_handle = Some(join_handle);
    Ok(loop_control.status_response())
}

async fn dashboard_snapshot(state: &AppState) -> anyhow::Result<DashboardResponse> {
    let loop_status = {
        let loop_control = state.loop_control.lock().await;
        loop_control.status_response()
    };

    let runtime = state.runtime.lock().await;
    let latest_iteration_id = runtime.latest_iteration_id()?;

    let runtime_state = runtime.state();
    let local_selection = runtime.local_selection().cloned();
    let frontier_selection = runtime.frontier_selection().cloned();
    let state_response = AgentStateResponse {
        state: runtime_state,
        reason: runtime.stop_reason().map(str::to_string),
        configured: runtime.is_configured(),
        local_selection: local_selection.clone(),
        frontier_selection: frontier_selection.clone(),
        latest_iteration_id,
    };

    let sensors = runtime
        .sensors()
        .into_iter()
        .map(|sensor| {
            let queued_percepts = sensor.queued_count();
            let unread_percepts = sensor.unread_count();
            SensorStatus {
                name: sensor.name,
                description: sensor.description,
                enabled: sensor.enabled,
                sensitivity_score: sensor.sensitivity_score,
                queued_percepts,
                unread_percepts,
                percept_singular_name: sensor.percept_singular_name,
                percept_plural_name: sensor.percept_plural_name,
                ingress: sensor.ingress,
            }
        })
        .collect();

    let actuators = runtime
        .actuators()
        .into_iter()
        .map(actuator_status)
        .collect();

    let loop_running = loop_status.running;

    Ok(DashboardResponse {
        state: state_response,
        loop_status,
        observability: runtime.observability_snapshot(),
        loop_visualization: runtime.loop_visualization_snapshot(),
        local_model: model_process_status(local_selection, runtime_state, loop_running),
        frontier_model: model_process_status(frontier_selection, runtime_state, loop_running),
        sensors,
        actuators,
        pending_approval_count: runtime.pending_approvals().len(),
    })
}

async fn list_provider_models(
    provider: ModelProviderKind,
    api_key: Option<String>,
) -> anyhow::Result<Vec<String>> {
    let normalized_key = normalize_api_key(api_key.as_deref().unwrap_or_default());
    let mut models = match provider {
        ModelProviderKind::Ollama => scrape_ollama_library_models().await.unwrap_or_else(|_| {
            vec![
                "gemma3:4b".to_string(),
                "qwen3:8b".to_string(),
                "gpt-oss:20b".to_string(),
            ]
        }),
        ModelProviderKind::OpenAi => list_openai_models(&normalized_key).await?,
        ModelProviderKind::OpenCodeZen => {
            list_models_with_api_key(ProviderId::OpenCodeZen, &normalized_key)
                .await
                .unwrap_or_default()
        }
    };

    models.sort();
    models.dedup();
    Ok(models)
}

async fn list_openai_models(api_key: &str) -> anyhow::Result<Vec<String>> {
    if api_key.is_empty() {
        return Ok(Vec::new());
    }

    let response = reqwest::Client::new()
        .get("https://api.openai.com/v1/models")
        .bearer_auth(api_key)
        .send()
        .await?
        .error_for_status()?;

    let value = response.json::<serde_json::Value>().await?;
    let mut models = value
        .get("data")
        .and_then(|items| items.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(|id| id.as_str()))
                .map(ToString::to_string)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();

    models.sort();
    Ok(models)
}

fn normalize_api_key(raw: &str) -> String {
    let trimmed = raw.trim();
    let unprefixed = trimmed
        .strip_prefix("Bearer ")
        .or_else(|| trimmed.strip_prefix("bearer "))
        .unwrap_or(trimmed);
    let unquoted = unprefixed
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim();
    unquoted.to_string()
}

async fn scrape_ollama_library_models() -> anyhow::Result<Vec<String>> {
    let html = scrape_ollama_library_html().await?;
    Ok(parse_ollama_library_tagged_models(&html))
}

async fn scrape_ollama_library_base_models() -> anyhow::Result<Vec<String>> {
    let html = scrape_ollama_library_html().await?;
    Ok(parse_ollama_library_base_models(&html))
}

async fn scrape_ollama_model_versions(model: &str) -> anyhow::Result<Vec<String>> {
    let url = format!("https://ollama.com/library/{model}");
    let html = reqwest::Client::new()
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(parse_ollama_model_versions(&html, model))
}

async fn scrape_ollama_library_html() -> anyhow::Result<String> {
    Ok(reqwest::Client::new()
        .get("https://ollama.com/library")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?)
}

fn parse_ollama_library_base_models(html: &str) -> Vec<String> {
    let mut models = Vec::new();
    let marker = "href=\"/library/";
    let mut cursor = 0usize;

    while let Some(rel) = html[cursor..].find(marker) {
        let start = cursor + rel + marker.len();
        let tail = &html[start..];
        let Some(end) = tail.find('"') else {
            break;
        };

        let candidate = tail[..end].trim();
        if candidate.is_empty() || candidate.contains(':') || candidate.contains('/') {
            cursor = start + end;
            continue;
        }

        models.push(candidate.to_string());
        cursor = start + end;
    }

    models.sort();
    models.dedup();
    models
}

fn parse_ollama_library_tagged_models(html: &str) -> Vec<String> {
    let bases = parse_ollama_library_base_models(html);
    let mut tagged = Vec::new();

    for base in bases {
        let tags = extract_size_tags_for_model_card(html, &base);
        if tags.is_empty() {
            tagged.push(format!("{base}:latest"));
        } else {
            for tag in tags {
                tagged.push(format!("{base}:{tag}"));
            }
        }
    }

    tagged.sort();
    tagged.dedup();
    tagged
}

fn parse_ollama_model_versions(html: &str, model: &str) -> Vec<String> {
    let href_marker = format!("href=\"/library/{model}:");
    let mut versions = Vec::new();
    let mut cursor = 0usize;

    while let Some(rel) = html[cursor..].find(&href_marker) {
        let start = cursor + rel + href_marker.len();
        let tail = &html[start..];
        let Some(end) = tail.find('"') else {
            break;
        };

        let version = tail[..end].trim().to_lowercase();
        if !version.is_empty() && !version.contains('/') {
            versions.push(version);
        }

        cursor = start + end;
    }

    versions.sort();
    versions.dedup();
    versions
}

fn extract_size_tags_for_model_card(html: &str, model: &str) -> Vec<String> {
    let anchor = format!("href=\"/library/{model}\"");
    let Some(model_pos) = html.find(&anchor) else {
        return Vec::new();
    };

    let block_start = html[..model_pos]
        .rfind("<li x-test-model")
        .unwrap_or(model_pos);
    let block_tail = &html[block_start..];
    let block_end_rel = block_tail.find("</li>").unwrap_or(block_tail.len());
    let block = &block_tail[..block_end_rel];

    let mut tags = Vec::new();
    let marker = "x-test-size";
    let mut cursor = 0usize;
    while let Some(rel) = block[cursor..].find(marker) {
        let marker_pos = cursor + rel;
        let Some(gt_rel) = block[marker_pos..].find('>') else {
            break;
        };
        let content_start = marker_pos + gt_rel + 1;
        let Some(lt_rel) = block[content_start..].find('<') else {
            break;
        };
        let value = block[content_start..content_start + lt_rel]
            .trim()
            .to_lowercase();
        if !value.is_empty() {
            tags.push(value);
        }
        cursor = content_start + lt_rel;
    }

    tags.sort();
    tags.dedup();
    tags
}

fn read_agent_identity(runtime: &LooperRuntime) -> anyhow::Result<AgentIdentityResponse> {
    let soul_markdown = read_soul_markdown(runtime)?;
    let skills = list_skill_summaries(runtime)?;
    Ok(AgentIdentityResponse {
        soul_markdown,
        skills,
    })
}

fn read_soul_markdown(runtime: &LooperRuntime) -> anyhow::Result<String> {
    let path = soul_markdown_path(runtime)?;
    if !path.exists() {
        fs::write(&path, DEFAULT_SOUL_MARKDOWN)?;
        return Ok(DEFAULT_SOUL_MARKDOWN.to_string());
    }
    Ok(fs::read_to_string(path)?)
}

fn write_soul_markdown(runtime: &LooperRuntime, markdown: &str) -> anyhow::Result<()> {
    let path = soul_markdown_path(runtime)?;
    fs::write(path, markdown)?;
    Ok(())
}

fn list_skill_summaries(runtime: &LooperRuntime) -> anyhow::Result<Vec<SkillSummary>> {
    let directory = skills_directory_path(runtime)?;
    let mut skills = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if !extension.eq_ignore_ascii_case("md") {
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let metadata = fs::metadata(&path)?;
        let updated_at_unix_ms = metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or_default();
        let base_name = file_name.strip_suffix(".md").unwrap_or(file_name);
        let display_name = base_name.replace(['-', '_'], " ");
        skills.push(SkillSummary {
            id: file_name.to_string(),
            name: display_name,
            updated_at_unix_ms,
        });
    }
    skills.sort_by_key(|skill| std::cmp::Reverse(skill.updated_at_unix_ms));
    Ok(skills)
}

fn get_skill_document(runtime: &LooperRuntime, id: &str) -> anyhow::Result<SkillDocumentResponse> {
    let file_name = normalize_skill_file_name(id)?;
    let path = skills_directory_path(runtime)?.join(&file_name);
    if !path.exists() {
        return Err(anyhow::anyhow!("skill '{file_name}' was not found"));
    }
    let markdown = fs::read_to_string(path)?;
    let name = file_name
        .strip_suffix(".md")
        .unwrap_or(file_name.as_str())
        .replace(['-', '_'], " ");
    Ok(SkillDocumentResponse {
        id: file_name,
        name,
        markdown,
    })
}

fn save_skill_document(
    runtime: &LooperRuntime,
    request: SaveSkillRequest,
) -> anyhow::Result<SkillDocumentResponse> {
    let file_name = match request.id {
        Some(id) => normalize_skill_file_name(&id)?,
        None => {
            let base_name = request
                .name
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("new-skill");
            normalize_skill_file_name(base_name)?
        }
    };
    let markdown = request.markdown;
    let path = skills_directory_path(runtime)?.join(&file_name);
    fs::write(&path, markdown.as_bytes())?;
    let name = file_name
        .strip_suffix(".md")
        .unwrap_or(file_name.as_str())
        .replace(['-', '_'], " ");
    Ok(SkillDocumentResponse {
        id: file_name,
        name,
        markdown,
    })
}

fn delete_skill_file(runtime: &LooperRuntime, id: &str) -> anyhow::Result<()> {
    let file_name = normalize_skill_file_name(id)?;
    let path = skills_directory_path(runtime)?.join(file_name);
    if !path.exists() {
        return Ok(());
    }
    fs::remove_file(path)?;
    Ok(())
}

fn soul_markdown_path(runtime: &LooperRuntime) -> anyhow::Result<std::path::PathBuf> {
    let agents_dir = agents_directory_path(runtime)?;
    Ok(agents_dir.join("SOUL.md"))
}

fn skills_directory_path(runtime: &LooperRuntime) -> anyhow::Result<std::path::PathBuf> {
    let agents_dir = agents_directory_path(runtime)?;
    let skills_dir = agents_dir.join("skills");
    fs::create_dir_all(&skills_dir)?;
    Ok(skills_dir)
}

fn agents_directory_path(runtime: &LooperRuntime) -> anyhow::Result<std::path::PathBuf> {
    let directory = runtime.workspace_root().join(".agents");
    fs::create_dir_all(&directory)?;
    Ok(directory)
}

fn normalize_skill_file_name(value: &str) -> anyhow::Result<String> {
    let without_extension = value.trim().trim_end_matches(".md");
    let slug = without_extension
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else if character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    let compact = slug
        .split('-')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if compact.is_empty() {
        return Err(anyhow::anyhow!(
            "skill name must include letters or numbers"
        ));
    }
    Ok(format!("{compact}.md"))
}

struct DirectoryWatcherHandle {
    path: PathBuf,
    watcher: RecommendedWatcher,
    task_handle: tokio::task::JoinHandle<()>,
}

impl DirectoryWatcherHandle {
    fn stop(self) {
        self.task_handle.abort();
        drop(self.watcher);
    }
}

async fn sync_directory_watchers(state: &AppState) -> anyhow::Result<()> {
    let sensors = {
        let runtime = state.runtime.lock().await;
        runtime.sensors()
    };

    let desired = sensors
        .into_iter()
        .filter_map(|sensor| {
            if let SensorIngressConfig::Directory { path } = sensor.ingress {
                Some((sensor.name, PathBuf::from(path.trim())))
            } else {
                None
            }
        })
        .filter(|(_, path)| !path.as_os_str().is_empty())
        .collect::<HashMap<_, _>>();

    let mut watchers = state.directory_watchers.lock().await;

    let stale_names = watchers
        .keys()
        .filter(|name| !desired.contains_key(*name))
        .cloned()
        .collect::<Vec<_>>();

    for name in stale_names {
        if let Some(handle) = watchers.remove(&name) {
            handle.stop();
        }
    }

    for (sensor_name, path) in desired {
        let needs_restart = watchers
            .get(&sensor_name)
            .map(|existing| existing.path != path)
            .unwrap_or(true);
        if !needs_restart {
            continue;
        }

        if let Some(existing) = watchers.remove(&sensor_name) {
            existing.stop();
        }

        if !path.exists() {
            continue;
        }

        let handle = spawn_directory_watcher(state.runtime.clone(), sensor_name.clone(), path)?;
        watchers.insert(sensor_name, handle);
    }

    Ok(())
}

fn spawn_directory_watcher(
    runtime: Arc<Mutex<LooperRuntime>>,
    sensor_name: String,
    path: PathBuf,
) -> anyhow::Result<DirectoryWatcherHandle> {
    let (sender, mut receiver) = mpsc::unbounded_channel::<Event>();
    let mut watcher = RecommendedWatcher::new(
        move |result| {
            if let Ok(event) = result {
                let _ = sender.send(event);
            }
        },
        NotifyConfig::default(),
    )?;
    watcher.watch(&path, RecursiveMode::NonRecursive)?;

    let task_handle = tokio::spawn(async move {
        while let Some(event) = receiver.recv().await {
            if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                continue;
            }

            for file_path in event.paths {
                if !file_path.is_file() {
                    continue;
                }

                tokio::time::sleep(Duration::from_millis(100)).await;
                let Ok(contents) = fs::read_to_string(&file_path) else {
                    continue;
                };
                if contents.trim().is_empty() {
                    continue;
                }

                let mut guard = runtime.lock().await;
                let _ = guard.enqueue_sensor_percept(&sensor_name, contents);
            }
        }
    });

    Ok(DirectoryWatcherHandle {
        path,
        watcher,
        task_handle,
    })
}

fn bad_request(message: String) -> (StatusCode, Json<ApiError>) {
    (StatusCode::BAD_REQUEST, Json(ApiError { error: message }))
}

fn internal_error(message: String) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError { error: message }),
    )
}

fn model_process_status(
    selection: Option<ModelSelection>,
    runtime_state: AgentState,
    loop_running: bool,
) -> ModelProcessStatus {
    let process_state = if selection.is_none() {
        "not_configured"
    } else if runtime_state == AgentState::Running && loop_running {
        "running"
    } else if runtime_state == AgentState::Stopped {
        "stopped"
    } else {
        "idle"
    };

    let (provider, model) = match selection {
        Some(selected) => (Some(selected.provider), Some(selected.model)),
        None => (None, None),
    };

    ModelProcessStatus {
        configured: provider.is_some(),
        provider,
        model,
        process_state: process_state.to_string(),
    }
}

fn actuator_status(actuator: Actuator) -> ActuatorStatus {
    let kind = match &actuator.kind {
        ActuatorType::Internal(_) => "internal",
        ActuatorType::Mcp(_) => "mcp",
        ActuatorType::Workflow(_) => "workflow",
        ActuatorType::Plugin(_) => "plugin",
    }
    .to_string();

    ActuatorStatus {
        name: actuator.name,
        description: actuator.description,
        kind,
        require_hitl: actuator.policy.require_hitl,
        sandboxed: actuator.policy.sandboxed,
        allowlist_count: actuator.policy.allowlist.map_or(0, |entries| entries.len()),
        denylist_count: actuator.policy.denylist.map_or(0, |entries| entries.len()),
        rate_limit: actuator.policy.rate_limit,
        action_singular_name: actuator.action_singular_name,
        action_plural_name: actuator.action_plural_name,
    }
}
