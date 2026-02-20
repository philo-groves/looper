use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::select;
use tokio::sync::{Mutex, oneshot};

use crate::dto::{ActuatorCreateRequest, SensorCreateRequest};
use crate::model::ExecutionResult;
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
        .route("/api/metrics", get(metrics_handler))
        .route("/api/loop/start", post(loop_start_handler))
        .route("/api/loop/stop", post(loop_stop_handler))
        .route("/api/loop/status", get(loop_status_handler))
        .route("/api/iterations/{id}", get(get_iteration_handler))
        .route("/api/approvals", get(list_approvals_handler))
        .route("/api/approvals/{id}/approve", post(approve_handler))
        .route("/api/approvals/{id}/deny", post(deny_handler))
        .with_state(state)
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

/// Starts continuous loop execution.
pub async fn loop_start_handler(
    State(state): State<AppState>,
    Json(request): Json<LoopStartRequest>,
) -> Result<Json<LoopStatusResponse>, (StatusCode, Json<ApiError>)> {
    let interval_ms = request.interval_ms.unwrap_or(200);

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
