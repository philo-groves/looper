use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use fiddlesticks::{ProviderId, list_models_with_api_key};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::select;
use tokio::sync::{Mutex, oneshot};

use crate::dto::{ActuatorCreateRequest, SensorCreateRequest};
use crate::model::{
    Actuator, ActuatorType, AgentState, ExecutionResult, ModelProviderKind, ModelSelection,
    RateLimit,
};
use crate::runtime::{LoopVisualizationSnapshot, LooperRuntime, ObservabilitySnapshot};
use crate::storage::PersistedIteration;

/// Shared HTTP state for Looper API handlers.
#[derive(Clone)]
pub struct AppState {
    /// Shared runtime instance.
    pub runtime: Arc<Mutex<LooperRuntime>>,
    loop_control: Arc<Mutex<LoopControl>>,
}

impl AppState {
    /// Creates application state from a runtime.
    pub fn new(runtime: LooperRuntime) -> Self {
        Self {
            runtime: Arc::new(Mutex::new(runtime)),
            loop_control: Arc::new(Mutex::new(LoopControl::default())),
        }
    }
}

/// Builds API router for sensor/actuator registration and loop operations.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/sensors", post(add_sensor_handler))
        .route("/api/actuators", post(add_actuator_handler))
        .route("/api/percepts/chat", post(add_chat_percept_handler))
        .route("/api/config/keys", post(register_api_key_handler))
        .route("/api/config/models", post(configure_models_handler))
        .route("/api/metrics", get(metrics_handler))
        .route("/api/health", get(health_handler))
        .route("/api/loop/start", post(loop_start_handler))
        .route("/api/loop/stop", post(loop_stop_handler))
        .route("/api/loop/status", get(loop_status_handler))
        .route("/api/state", get(state_handler))
        .route("/api/dashboard", get(dashboard_handler))
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
    runtime.add_sensor(sensor);

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
    runtime.add_actuator(actuator);

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
    runtime
        .enqueue_chat_message(request.message)
        .map_err(|error| bad_request(error.to_string()))?;

    Ok((
        StatusCode::ACCEPTED,
        Json(SimpleStatusResponse {
            status: "accepted".to_string(),
        }),
    ))
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
    let mut runtime = state.runtime.lock().await;
    runtime
        .configure_models(request.local, request.frontier)
        .map_err(|error| bad_request(error.to_string()))?;

    Ok(Json(SimpleStatusResponse {
        status: "ok".to_string(),
    }))
}

/// Starts continuous loop execution.
pub async fn loop_start_handler(
    State(state): State<AppState>,
    Json(request): Json<LoopStartRequest>,
) -> Result<Json<LoopStatusResponse>, (StatusCode, Json<ApiError>)> {
    let interval_ms = request.interval_ms.unwrap_or(200);
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
}

/// Request payload for loop start.
#[derive(Clone, Debug, Deserialize)]
pub struct LoopStartRequest {
    /// Optional delay between loop iterations.
    pub interval_ms: Option<u64>,
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
            let status = loop_start_impl(state, payload.interval_ms.unwrap_or(200))
                .await
                .map_err(|error| error.to_string())?;
            Ok(serde_json::to_value(status).map_err(|error| error.to_string())?)
        }
        "enqueue_chat_message" => {
            let payload: ChatPerceptRequest =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let mut runtime = state.runtime.lock().await;
            runtime
                .enqueue_chat_message(payload.message)
                .map_err(|error| error.to_string())?;
            Ok(serde_json::json!({ "status": "accepted" }))
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
            let mut runtime = state.runtime.lock().await;
            runtime
                .configure_models(payload.local, payload.frontier)
                .map_err(|error| error.to_string())?;
            Ok(serde_json::json!({ "status": "ok" }))
        }
        "update_sensor" => {
            let payload: WsUpdateSensorParams =
                serde_json::from_value(params).map_err(|error| error.to_string())?;
            let mut runtime = state.runtime.lock().await;
            runtime
                .update_sensor(
                    &payload.name,
                    payload.enabled,
                    payload.sensitivity_score,
                    payload.description,
                    payload.percept_singular_name,
                    payload.percept_plural_name,
                )
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
