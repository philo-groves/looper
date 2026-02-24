//! Core runtime, DTOs, executors, and HTTP handlers for Looper.

pub mod dto;
pub mod executors;
pub mod http;
pub mod model;
pub mod models;
pub mod runtime;
pub mod storage;

pub use dto::{
    ActuatorCreateRequest, ActuatorRegistrationType, PluginImportRequest, SensorCreateRequest,
};
pub use executors::{
    ActuatorExecutor, ChatActuatorExecutor, GlobActuatorExecutor, GrepActuatorExecutor,
    ShellActuatorExecutor, WebSearchActuatorExecutor,
};
pub use http::{AppState, auto_start_loop_if_configured, build_router, initialize_sensor_ingress};
pub use model::{
    Action, Actuator, ActuatorType, AgentState, DenoPermissions, ExecutionResult,
    InternalActuatorKind, McpConnectionType, McpDetails, ModelProviderKind, ModelSelection,
    PendingApproval, Percept, PluginActuatorDefinition, PluginActuatorDetails, PluginManifest,
    PluginSensorDefinition, PluginSensorIngress, RateLimit, RateLimitPeriod, RecommendedAction,
    SafetyPolicy, Sensor, SensorIngressConfig, SensorRestFormat, WorkflowDetails,
};
pub use models::{
    FiddlesticksFrontierModel, FiddlesticksLocalModel, FrontierModel, FrontierModelRequest,
    FrontierModelResponse, LocalModel, LocalModelRequest, LocalModelResponse,
    RuleBasedFrontierModel, RuleBasedLocalModel,
};
pub use runtime::{
    FrontierLoopStep, IterationReport, LocalLoopStep, LoopPhase, LoopPhaseTransitionEvent,
    LoopRuntimePhase, LoopVisualizationSnapshot, LooperRuntime, Observability,
    ObservabilitySnapshot, default_agent_workspace_dir,
};
pub use storage::{PersistedChatMessage, PersistedChatSession, PersistedIteration, SqliteStore};
