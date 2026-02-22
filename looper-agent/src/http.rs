use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::select;
use tokio::sync::{Mutex, oneshot};

use crate::dto::{ActuatorCreateRequest, SensorCreateRequest};
use crate::model::{
    Actuator, ActuatorType, AgentState, ExecutionResult, ModelProviderKind, ModelSelection,
    RateLimit,
};
use crate::runtime::{LooperRuntime, ObservabilitySnapshot};
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

    {
        let mut runtime = state.runtime.lock().await;
        runtime
            .start()
            .map_err(|error| bad_request(error.to_string()))?;
    }

    {
        let loop_control = state.loop_control.lock().await;
        if loop_control.running {
            return Ok(Json(loop_control.status_response()));
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
                        tokio::time::sleep(Duration::from_millis(500)).await;
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
    Ok(Json(loop_control.status_response()))
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
    let loop_status = {
        let loop_control = state.loop_control.lock().await;
        loop_control.status_response()
    };

    let runtime = state.runtime.lock().await;
    let latest_iteration_id = runtime
        .latest_iteration_id()
        .map_err(|error| internal_error(error.to_string()))?;

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
            }
        })
        .collect();

    let actuators = runtime
        .actuators()
        .into_iter()
        .map(actuator_status)
        .collect();

    let loop_running = loop_status.running;

    Ok(Json(DashboardResponse {
        state: state_response,
        loop_status,
        observability: runtime.observability_snapshot(),
        local_model: model_process_status(local_selection, runtime_state, loop_running),
        frontier_model: model_process_status(frontier_selection, runtime_state, loop_running),
        sensors,
        actuators,
        pending_approval_count: runtime.pending_approvals().len(),
    }))
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
    }
}
