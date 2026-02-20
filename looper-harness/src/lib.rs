//! Core runtime, DTOs, executors, and HTTP handlers for Looper.

pub mod dto;
pub mod executors;
pub mod http;
pub mod model;
pub mod models;
pub mod runtime;
pub mod storage;

pub use dto::{ActuatorCreateRequest, ActuatorRegistrationType, SensorCreateRequest};
pub use executors::{
    ActuatorExecutor, ChatActuatorExecutor, GlobActuatorExecutor, GrepActuatorExecutor,
    ShellActuatorExecutor, WebSearchActuatorExecutor,
};
pub use http::{AppState, build_router};
pub use model::{
    Action, Actuator, ActuatorType, ExecutionResult, InternalActuatorKind, McpConnectionType,
    McpDetails, PendingApproval, Percept, RateLimit, RateLimitPeriod, RecommendedAction,
    SafetyPolicy, Sensor, WorkflowDetails,
};
pub use models::{
    FiddlesticksFrontierModel, FiddlesticksLocalModel, FrontierModel, FrontierModelRequest,
    FrontierModelResponse, LocalModel, LocalModelRequest, LocalModelResponse,
    RuleBasedFrontierModel, RuleBasedLocalModel,
};
pub use runtime::{
    IterationReport, LoopPhase, LooperRuntime, Observability, ObservabilitySnapshot,
};
pub use storage::{PersistedIteration, SqliteStore};
