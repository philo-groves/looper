use std::collections::{HashMap, VecDeque};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::executors::{
    ActuatorExecutor, ChatActuatorExecutor, GlobActuatorExecutor, GrepActuatorExecutor,
    ShellActuatorExecutor, WebSearchActuatorExecutor,
};
use crate::model::{
    Action, Actuator, ActuatorType, AgentState, ExecutionResult, InternalActuatorKind,
    ModelProviderKind, ModelSelection, PendingApproval, Percept, PluginManifest,
    PluginRequirements, PluginSensorIngress, RecommendedAction, SafetyPolicy, Sensor,
    SensorIngressConfig, SensorRestFormat,
};
use crate::models::{
    FiddlesticksFrontierModel, FiddlesticksLocalModel, FrontierModel, FrontierModelRequest,
    LocalModel, LocalModelRequest, RuleBasedFrontierModel, RuleBasedLocalModel, SkillContext,
};
use crate::plugin_contract::parse_plugin_route_signal;
use crate::storage::{PersistedChatMessage, PersistedChatSession, PersistedIteration, SqliteStore};

const FORCE_SURPRISE_SENSITIVITY_THRESHOLD: u8 = 90;
const DEFAULT_SOUL_MARKDOWN: &str =
    "# Looper Soul\n\nLooper is a curious, practical, and upbeat general-purpose agent.";

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PersistedAgentSettings {
    #[serde(default)]
    local_provider: Option<ModelProviderKind>,
    #[serde(default)]
    local_model: Option<String>,
    #[serde(default)]
    frontier_provider: Option<ModelProviderKind>,
    #[serde(default)]
    frontier_model: Option<String>,
    #[serde(default)]
    sensors: Vec<PersistedSensorSettings>,
    #[serde(default)]
    actuators: Vec<PersistedActuatorSettings>,
    #[serde(default)]
    plugin_enabled_overrides: HashMap<String, bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PersistedSensorSettings {
    name: String,
    description: String,
    enabled: bool,
    sensitivity_score: u8,
    percept_singular_name: String,
    percept_plural_name: String,
    #[serde(default = "default_sensor_ingress")]
    ingress: SensorIngressConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PersistedActuatorSettings {
    name: String,
    description: String,
    #[serde(default)]
    kind: Option<PersistedActuatorKind>,
    policy: SafetyPolicy,
    action_singular_name: String,
    action_plural_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PersistedActuatorKind {
    Internal {
        kind: String,
    },
    Mcp {
        details: crate::model::McpDetails,
    },
    Workflow {
        details: crate::model::WorkflowDetails,
    },
    Plugin {
        details: Box<crate::model::PluginActuatorDetails>,
    },
}

/// Partial update payload for mutable sensor settings.
#[derive(Clone, Debug, Default)]
pub struct SensorUpdate {
    /// Optional new enabled state.
    pub enabled: Option<bool>,
    /// Optional new sensitivity score.
    pub sensitivity_score: Option<u8>,
    /// Optional new percept description.
    pub description: Option<String>,
    /// Optional new percept singular display name.
    pub percept_singular_name: Option<String>,
    /// Optional new percept plural display name.
    pub percept_plural_name: Option<String>,
    /// Optional ingress configuration update.
    pub ingress: Option<SensorIngressConfig>,
}

/// Partial update payload for mutable actuator settings.
#[derive(Clone, Debug, Default)]
pub struct ActuatorUpdate {
    /// Optional new action description.
    pub description: Option<String>,
    /// Optional new human-in-the-loop requirement.
    pub require_hitl: Option<bool>,
    /// Optional new sandboxed execution setting.
    pub sandboxed: Option<bool>,
    /// Optional new rate-limit policy. `Some(None)` clears rate limiting.
    pub rate_limit: Option<Option<crate::model::RateLimit>>,
    /// Optional new singular display name for actions.
    pub action_singular_name: Option<String>,
    /// Optional new plural display name for actions.
    pub action_plural_name: Option<String>,
}

/// Phases of a loop iteration.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum LoopPhase {
    SurpriseDetection,
    Reasoning,
    PerformActions,
}

/// Local model loop step for dashboard visualization.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalLoopStep {
    GatherNewPercepts,
    CheckForSurprises,
    NoSurprise,
    SurpriseFound,
}

/// Frontier model loop step for dashboard visualization.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrontierLoopStep {
    DeeperPerceptInvestigation,
    PlanActions,
    PerformingActions,
}

/// High-level current phase of the loop runtime.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoopRuntimePhase {
    GatherNewPercepts,
    CheckForSurprises,
    DeeperPerceptInvestigation,
    PlanActions,
    ExecuteActions,
    Idle,
}

/// Serialization-friendly loop state payload for dashboard rendering.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LoopVisualizationSnapshot {
    /// Current local loop step.
    pub local_current_step: LocalLoopStep,
    /// Current frontier loop step, if the frontier loop is active.
    pub frontier_current_step: Option<FrontierLoopStep>,
    /// Whether the latest local check found a surprise.
    pub surprise_found: bool,
    /// Whether the latest frontier plan requires actions.
    pub action_required: bool,
    /// Total local loop count.
    pub local_loop_count: u64,
    /// Total frontier loop count.
    pub frontier_loop_count: u64,
    /// Current runtime phase.
    pub current_phase: LoopRuntimePhase,
    /// Unix timestamp in milliseconds when the current phase started.
    pub current_phase_started_at_unix_ms: i64,
}

/// Phase transition event for websocket consumers.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LoopPhaseTransitionEvent {
    /// Monotonic event sequence.
    pub sequence: u64,
    /// Active runtime phase after this transition.
    pub phase: LoopRuntimePhase,
    /// Loop visualization snapshot at transition time.
    pub loop_visualization: LoopVisualizationSnapshot,
    /// Unix timestamp in milliseconds when this event was emitted.
    pub emitted_at_unix_ms: i64,
}

#[derive(Clone, Copy, Debug)]
struct LoopVisualizationState {
    local_current_step: LocalLoopStep,
    frontier_current_step: Option<FrontierLoopStep>,
    surprise_found: bool,
    action_required: bool,
    local_loop_count: u64,
    frontier_loop_count: u64,
    current_phase: LoopRuntimePhase,
    current_phase_started_at_unix_ms: i64,
}

impl Default for LoopVisualizationState {
    fn default() -> Self {
        Self {
            local_current_step: LocalLoopStep::GatherNewPercepts,
            frontier_current_step: None,
            surprise_found: false,
            action_required: false,
            local_loop_count: 0,
            frontier_loop_count: 0,
            current_phase: LoopRuntimePhase::Idle,
            current_phase_started_at_unix_ms: now_unix_ms(),
        }
    }
}

impl LoopVisualizationState {
    fn snapshot(self) -> LoopVisualizationSnapshot {
        LoopVisualizationSnapshot {
            local_current_step: self.local_current_step,
            frontier_current_step: self.frontier_current_step,
            surprise_found: self.surprise_found,
            action_required: self.action_required,
            local_loop_count: self.local_loop_count,
            frontier_loop_count: self.frontier_loop_count,
            current_phase: self.current_phase,
            current_phase_started_at_unix_ms: self.current_phase_started_at_unix_ms,
        }
    }
}

impl LoopPhase {
    fn as_key(self) -> &'static str {
        match self {
            Self::SurpriseDetection => "surprise_detection",
            Self::Reasoning => "reasoning",
            Self::PerformActions => "perform_actions",
        }
    }
}

/// Observability counters for loop health.
#[derive(Clone, Debug)]
pub struct Observability {
    pub phase_execution_counts: HashMap<LoopPhase, u64>,
    pub local_model_tokens: u64,
    pub frontier_model_tokens: u64,
    pub false_positive_surprises: u64,
    pub failed_tool_executions: u64,
    pub total_iterations: u64,
    start: Instant,
    started_at_unix: i64,
}

impl Default for Observability {
    fn default() -> Self {
        Self {
            phase_execution_counts: HashMap::new(),
            local_model_tokens: 0,
            frontier_model_tokens: 0,
            false_positive_surprises: 0,
            failed_tool_executions: 0,
            total_iterations: 0,
            start: Instant::now(),
            started_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
        }
    }
}

impl Observability {
    pub fn bump_phase(&mut self, phase: LoopPhase) {
        *self.phase_execution_counts.entry(phase).or_insert(0) += 1;
    }

    pub fn loops_per_minute(&self) -> f64 {
        let elapsed_secs = self.start.elapsed().as_secs_f64();
        if elapsed_secs <= f64::EPSILON {
            return 0.0;
        }
        (self.total_iterations as f64 / elapsed_secs) * 60.0
    }

    pub fn failed_tool_execution_percent(&self) -> f64 {
        if self.total_iterations == 0 {
            return 0.0;
        }
        (self.failed_tool_executions as f64 / self.total_iterations as f64) * 100.0
    }

    pub fn false_positive_surprise_percent(&self) -> f64 {
        if self.total_iterations == 0 {
            return 0.0;
        }
        (self.false_positive_surprises as f64 / self.total_iterations as f64) * 100.0
    }

    pub fn snapshot(&self) -> ObservabilitySnapshot {
        let mut phase_execution_counts = HashMap::new();
        for (phase, count) in &self.phase_execution_counts {
            phase_execution_counts.insert(phase.as_key().to_string(), *count);
        }

        ObservabilitySnapshot {
            phase_execution_counts,
            local_model_tokens: self.local_model_tokens,
            frontier_model_tokens: self.frontier_model_tokens,
            false_positive_surprises: self.false_positive_surprises,
            false_positive_surprise_percent: self.false_positive_surprise_percent(),
            failed_tool_executions: self.failed_tool_executions,
            failed_tool_execution_percent: self.failed_tool_execution_percent(),
            total_iterations: self.total_iterations,
            loops_per_minute: self.loops_per_minute(),
            started_at_unix: self.started_at_unix,
        }
    }
}

/// Serialization-friendly observability payload.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ObservabilitySnapshot {
    pub phase_execution_counts: HashMap<String, u64>,
    pub local_model_tokens: u64,
    pub frontier_model_tokens: u64,
    pub false_positive_surprises: u64,
    pub false_positive_surprise_percent: f64,
    pub failed_tool_executions: u64,
    pub failed_tool_execution_percent: f64,
    pub total_iterations: u64,
    pub loops_per_minute: f64,
    pub started_at_unix: i64,
}

/// Output of a completed loop iteration.
#[derive(Clone, Debug)]
pub struct IterationReport {
    pub iteration_id: Option<i64>,
    pub sensed_percepts: Vec<Percept>,
    pub surprising_percepts: Vec<Percept>,
    pub planned_actions: Vec<RecommendedAction>,
    pub action_results: Vec<ExecutionResult>,
    pub ended_after_surprise_detection: bool,
    pub ended_after_reasoning: bool,
}

/// Runtime status for one bundled internal plugin package.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InternalPluginStatus {
    /// Stable plugin id from manifest name when available.
    pub id: String,
    /// Plugin display name.
    pub name: String,
    /// Plugin version string.
    pub version: String,
    /// Absolute plugin directory path.
    pub path: String,
    /// Whether this plugin package has been imported into runtime registries.
    pub imported: bool,
    /// Whether runtime requirements are currently satisfied.
    pub enabled: bool,
    /// Whether the plugin is user-enabled.
    pub user_enabled: bool,
    /// Optional reason for disabled or errored status.
    pub status_message: Option<String>,
    /// Declared sensor names for this plugin.
    pub sensors: Vec<String>,
    /// Declared actuator names for this plugin.
    pub actuators: Vec<String>,
}

/// Runtime for Looper sensory loop.
pub struct LooperRuntime {
    sensors: HashMap<String, Sensor>,
    actuators: HashMap<String, Actuator>,
    internal_executors: HashMap<InternalActuatorKind, Box<dyn ActuatorExecutor>>,
    local_model: Option<Box<dyn LocalModel>>,
    frontier_model: Option<Box<dyn FrontierModel>>,
    local_selection: Option<ModelSelection>,
    frontier_selection: Option<ModelSelection>,
    provider_api_keys: HashMap<ModelProviderKind, String>,
    agent_state: AgentState,
    stop_reason: Option<String>,
    observability: Observability,
    executions_per_actuator: HashMap<String, u32>,
    pending_approvals: HashMap<u64, PendingApproval>,
    next_approval_id: u64,
    workspace_root: PathBuf,
    store: Option<SqliteStore>,
    loop_visualization: LoopVisualizationState,
    phase_events: VecDeque<LoopPhaseTransitionEvent>,
    next_phase_event_sequence: u64,
    plugin_enabled_overrides: HashMap<String, bool>,
}

impl LooperRuntime {
    pub fn new() -> Self {
        Self {
            sensors: HashMap::new(),
            actuators: HashMap::new(),
            internal_executors: HashMap::new(),
            local_model: None,
            frontier_model: None,
            local_selection: None,
            frontier_selection: None,
            provider_api_keys: HashMap::new(),
            agent_state: AgentState::Setup,
            stop_reason: None,
            observability: Observability::default(),
            executions_per_actuator: HashMap::new(),
            pending_approvals: HashMap::new(),
            next_approval_id: 1,
            workspace_root: default_agent_workspace_dir(),
            store: None,
            loop_visualization: LoopVisualizationState::default(),
            phase_events: VecDeque::new(),
            next_phase_event_sequence: 1,
            plugin_enabled_overrides: HashMap::new(),
        }
    }

    pub fn with_internal_defaults() -> Result<Self> {
        let workspace_root = std::env::current_dir()?;
        Self::with_internal_defaults_for_workspace(workspace_root)
    }

    /// Builds a runtime with default sensors/actuators for a fixed workspace root.
    pub fn with_internal_defaults_for_workspace(
        workspace_root: impl Into<PathBuf>,
    ) -> Result<Self> {
        let mut runtime = Self::new();
        let mut chat_sensor = Sensor::with_sensitivity_score(
            "chat",
            "Conversational messages that should always be considered surprising.",
            100,
        );
        chat_sensor.percept_singular_name = "Incoming Message".to_string();
        chat_sensor.percept_plural_name = "Incoming Messages".to_string();
        chat_sensor.ingress = SensorIngressConfig::Internal;
        runtime.add_sensor(chat_sensor);

        let mut chat_actuator = Actuator::internal(
            "chat",
            "Used for responding to chat requests.",
            InternalActuatorKind::Chat,
            SafetyPolicy::default(),
        )?;
        chat_actuator.action_singular_name = "Outgoing Message".to_string();
        chat_actuator.action_plural_name = "Outgoing Messages".to_string();
        runtime.add_actuator(chat_actuator);
        runtime.add_actuator(Actuator::internal(
            "grep",
            "Searches text-based file contents",
            InternalActuatorKind::Grep,
            SafetyPolicy::default(),
        )?);
        runtime.add_actuator(Actuator::internal(
            "glob",
            "Searches directories for files",
            InternalActuatorKind::Glob,
            SafetyPolicy::default(),
        )?);
        runtime.add_actuator(Actuator::internal(
            "shell",
            "Performs command line operations",
            InternalActuatorKind::Shell,
            SafetyPolicy::default(),
        )?);
        runtime.add_actuator(Actuator::internal(
            "web_search",
            "Searches the internet for up-to-date information",
            InternalActuatorKind::WebSearch,
            SafetyPolicy::default(),
        )?);

        let workspace_root = workspace_root.into();
        runtime.workspace_root = workspace_root.clone();
        runtime.register_internal_executor(
            InternalActuatorKind::Chat,
            Box::<ChatActuatorExecutor>::default(),
        );
        runtime.register_internal_executor(
            InternalActuatorKind::Grep,
            Box::new(GrepActuatorExecutor::new(&workspace_root)),
        );
        runtime.register_internal_executor(
            InternalActuatorKind::Glob,
            Box::new(GlobActuatorExecutor::new(&workspace_root)),
        );
        runtime.register_internal_executor(
            InternalActuatorKind::Shell,
            Box::new(ShellActuatorExecutor::new(&workspace_root)),
        );
        runtime.register_internal_executor(
            InternalActuatorKind::WebSearch,
            Box::<WebSearchActuatorExecutor>::default(),
        );

        runtime.attach_store(SqliteStore::new(default_store_path())?);
        runtime.load_persisted_api_keys()?;
        runtime.import_bundled_internal_plugins();
        runtime.load_persisted_settings()?;
        Ok(runtime)
    }

    pub fn register_api_key(
        &mut self,
        provider: ModelProviderKind,
        api_key: impl Into<String>,
    ) -> Result<()> {
        let value = normalize_api_key_value(&api_key.into());
        if value.is_empty() {
            return Err(anyhow!("api key cannot be empty"));
        }
        self.provider_api_keys.insert(provider, value);
        self.persist_api_keys()?;
        self.log_state("register_api_key", format!("provider={provider:?}"));
        Ok(())
    }

    pub fn configure_models(
        &mut self,
        local: ModelSelection,
        frontier: ModelSelection,
    ) -> Result<()> {
        let local_model = self.build_local_model(&local)?;
        let frontier_model = self.build_frontier_model(&frontier)?;

        self.local_selection = Some(local);
        self.frontier_selection = Some(frontier);
        self.local_model = Some(local_model);
        self.frontier_model = Some(frontier_model);
        if self.agent_state == AgentState::Setup {
            self.agent_state = AgentState::Stopped;
            self.stop_reason = None;
        }
        self.persist_settings()?;
        self.log_state(
            "configure_models",
            format!(
                "local={:?}:{}, frontier={:?}:{}",
                self.local_selection.as_ref().map(|item| item.provider),
                self.local_selection
                    .as_ref()
                    .map(|item| item.model.as_str())
                    .unwrap_or(""),
                self.frontier_selection.as_ref().map(|item| item.provider),
                self.frontier_selection
                    .as_ref()
                    .map(|item| item.model.as_str())
                    .unwrap_or("")
            ),
        );
        Ok(())
    }

    pub fn use_rule_models_for_testing(&mut self) {
        self.local_model = Some(Box::new(RuleBasedLocalModel));
        self.frontier_model = Some(Box::new(RuleBasedFrontierModel));
    }

    pub fn start(&mut self) -> Result<()> {
        if !self.is_configured() {
            return Err(anyhow!(
                "runtime is not configured: select local/frontier models and required API keys"
            ));
        }
        self.agent_state = AgentState::Running;
        self.stop_reason = None;
        self.log_state("start", "runtime started");
        Ok(())
    }

    pub fn stop(&mut self, reason: impl Into<String>) {
        self.agent_state = AgentState::Stopped;
        let reason = reason.into();
        self.stop_reason = Some(reason.clone());
        self.log_state("stop", reason);
    }

    pub fn state(&self) -> AgentState {
        self.agent_state
    }

    pub fn stop_reason(&self) -> Option<&str> {
        self.stop_reason.as_deref()
    }

    pub fn is_configured(&self) -> bool {
        self.local_model.is_some() && self.frontier_model.is_some()
    }

    pub fn local_selection(&self) -> Option<&ModelSelection> {
        self.local_selection.as_ref()
    }

    pub fn frontier_selection(&self) -> Option<&ModelSelection> {
        self.frontier_selection.as_ref()
    }

    pub fn attach_store(&mut self, store: SqliteStore) {
        self.store = Some(store);
    }

    pub fn disable_store(&mut self) {
        self.store = None;
    }

    pub fn get_iteration(&self, id: i64) -> Result<Option<PersistedIteration>> {
        match &self.store {
            Some(store) => store.get_iteration(id),
            None => Ok(None),
        }
    }

    /// Lists persisted iterations after an optional id.
    pub fn list_iterations_after(
        &self,
        after_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<PersistedIteration>> {
        match &self.store {
            Some(store) => store.list_iterations_after(after_id, limit),
            None => Ok(Vec::new()),
        }
    }

    /// Returns the latest persisted iteration id.
    pub fn latest_iteration_id(&self) -> Result<Option<i64>> {
        match &self.store {
            Some(store) => store.latest_iteration_id(),
            None => Ok(None),
        }
    }

    /// Returns the configured workspace root directory.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Enables or disables an imported plugin package by id.
    pub fn set_plugin_enabled(&mut self, plugin_id: &str, enabled: bool) -> Result<()> {
        let plugin_id = plugin_id.trim();
        if plugin_id.is_empty() {
            return Err(anyhow!("plugin id cannot be empty"));
        }

        let exists = self.sensors.values().any(|sensor| {
            matches!(&sensor.ingress, SensorIngressConfig::Plugin(details) if details.plugin == plugin_id)
        }) || self.actuators.values().any(|actuator| {
            matches!(&actuator.kind, ActuatorType::Plugin(details) if details.plugin == plugin_id)
        });

        if !exists {
            return Err(anyhow!("plugin '{}' is not configured", plugin_id));
        }

        self.plugin_enabled_overrides
            .insert(plugin_id.to_string(), enabled);
        self.persist_settings()?;
        self.log_state(
            "set_plugin_enabled",
            format!("plugin={}, enabled={enabled}", plugin_id),
        );
        Ok(())
    }

    /// Returns whether a plugin is user-enabled when configured.
    pub fn plugin_user_enabled(&self, plugin_id: &str) -> Option<bool> {
        let plugin_id = plugin_id.trim();
        if plugin_id.is_empty() {
            return None;
        }

        let exists = self.sensors.values().any(|sensor| {
            matches!(&sensor.ingress, SensorIngressConfig::Plugin(details) if details.plugin == plugin_id)
        }) || self.actuators.values().any(|actuator| {
            matches!(&actuator.kind, ActuatorType::Plugin(details) if details.plugin == plugin_id)
        });

        if !exists {
            return None;
        }

        Some(
            self.plugin_enabled_overrides
                .get(plugin_id)
                .copied()
                .unwrap_or(true),
        )
    }

    /// Returns status for bundled internal plugin packages.
    pub fn internal_plugin_statuses(&self) -> Vec<InternalPluginStatus> {
        let base_dir = bundled_internal_plugins_dir();
        let entries = match fs::read_dir(&base_dir) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };

        let mut plugin_dirs = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        plugin_dirs.sort();

        let mut statuses = Vec::new();
        for plugin_dir in plugin_dirs {
            statuses.push(self.internal_plugin_status_for_dir(&plugin_dir));
        }
        statuses
    }

    fn internal_plugin_status_for_dir(&self, plugin_dir: &Path) -> InternalPluginStatus {
        let fallback_id = plugin_dir
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("unknown")
            .to_string();
        let manifest_path = plugin_dir.join("looper-plugin.json");
        let path_text = plugin_dir.to_string_lossy().to_string();

        let raw = match fs::read_to_string(&manifest_path) {
            Ok(raw) => raw,
            Err(error) => {
                return InternalPluginStatus {
                    id: fallback_id.clone(),
                    name: fallback_id,
                    version: "unknown".to_string(),
                    path: path_text,
                    imported: false,
                    enabled: false,
                    user_enabled: false,
                    status_message: Some(format!("missing or unreadable manifest: {}", error)),
                    sensors: Vec::new(),
                    actuators: Vec::new(),
                };
            }
        };

        let manifest = match serde_json::from_str::<PluginManifest>(&raw) {
            Ok(manifest) => manifest,
            Err(error) => {
                return InternalPluginStatus {
                    id: fallback_id.clone(),
                    name: fallback_id,
                    version: "unknown".to_string(),
                    path: path_text,
                    imported: false,
                    enabled: false,
                    user_enabled: false,
                    status_message: Some(format!("invalid manifest JSON: {}", error)),
                    sensors: Vec::new(),
                    actuators: Vec::new(),
                };
            }
        };

        if let Err(error) = manifest.validate() {
            return InternalPluginStatus {
                id: manifest.name.clone(),
                name: manifest.name,
                version: manifest.version,
                path: path_text,
                imported: false,
                enabled: false,
                user_enabled: false,
                status_message: Some(format!("invalid manifest: {}", error)),
                sensors: Vec::new(),
                actuators: Vec::new(),
            };
        }

        let plugin_name = manifest.name.trim().to_string();
        let sensors = manifest
            .sensors
            .iter()
            .map(|item| format!("{}:{}", plugin_name, item.name.trim()))
            .collect::<Vec<_>>();
        let actuators = manifest
            .actuators
            .iter()
            .map(|item| format!("{}:{}", plugin_name, item.name.trim()))
            .collect::<Vec<_>>();
        let imported = sensors.iter().all(|name| self.sensors.contains_key(name))
            && actuators
                .iter()
                .all(|name| self.actuators.contains_key(name));
        let user_enabled = self
            .plugin_enabled_overrides
            .get(&plugin_name)
            .copied()
            .unwrap_or(true);
        let status_message = self.plugin_disabled_reason(&plugin_name, &manifest.requirements);
        let enabled = status_message.is_none();

        InternalPluginStatus {
            id: plugin_name.clone(),
            name: plugin_name,
            version: manifest.version,
            path: path_text,
            imported,
            enabled,
            user_enabled,
            status_message,
            sensors,
            actuators,
        }
    }

    fn plugin_disabled_reason(
        &self,
        plugin_name: &str,
        requirements: &PluginRequirements,
    ) -> Option<String> {
        let user_enabled = self
            .plugin_enabled_overrides
            .get(plugin_name)
            .copied()
            .unwrap_or(true);
        plugin_disabled_reason_with_user(user_enabled, requirements)
    }

    pub fn add_sensor(&mut self, sensor: Sensor) {
        self.sensors.insert(sensor.name.clone(), sensor);
    }

    /// Registers a new sensor and persists settings.
    pub fn register_sensor(&mut self, mut sensor: Sensor) -> Result<()> {
        if sensor.name == "chat" {
            sensor.ingress = SensorIngressConfig::Internal;
        }
        if let SensorIngressConfig::Directory { path } = &sensor.ingress
            && path.trim().is_empty()
        {
            return Err(anyhow!("directory path cannot be empty"));
        }
        if let SensorIngressConfig::Plugin(details) = &sensor.ingress {
            details.validate()?;
        }

        self.add_sensor(sensor);
        self.persist_settings()
    }

    /// Updates mutable sensor configuration fields for an existing sensor.
    pub fn update_sensor(&mut self, name: &str, update: SensorUpdate) -> Result<()> {
        if name == "chat" && matches!(update.enabled, Some(false)) {
            return Err(anyhow!("chat sensor cannot be disabled"));
        }

        let (enabled_now, sensitivity_now, singular_now, plural_now) = {
            let sensor = self
                .sensors
                .get_mut(name)
                .ok_or_else(|| anyhow!("sensor '{name}' not found"))?;

            if let Some(value) = update.enabled {
                sensor.enabled = value;
            }
            if let Some(value) = update.sensitivity_score {
                sensor.sensitivity_score = value.min(100);
            }
            if let Some(value) = update.description {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    return Err(anyhow!("description cannot be empty"));
                }
                sensor.description = trimmed.to_string();
            }
            if let Some(value) = update.percept_singular_name {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    return Err(anyhow!("percept singular name cannot be empty"));
                }
                sensor.percept_singular_name = trimmed.to_lowercase();
            }
            if let Some(value) = update.percept_plural_name {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    return Err(anyhow!("percept plural name cannot be empty"));
                }
                sensor.percept_plural_name = trimmed.to_lowercase();
            }
            if let Some(value) = update.ingress {
                if name == "chat" && value != SensorIngressConfig::Internal {
                    return Err(anyhow!("chat sensor ingress is managed internally"));
                }

                match &value {
                    SensorIngressConfig::Directory { path } => {
                        if path.trim().is_empty() {
                            return Err(anyhow!("directory path cannot be empty"));
                        }
                    }
                    SensorIngressConfig::Plugin(details) => {
                        details.validate()?;
                    }
                    SensorIngressConfig::Internal | SensorIngressConfig::RestApi { .. } => {}
                }

                sensor.ingress = value;
            }

            (
                sensor.enabled,
                sensor.sensitivity_score,
                sensor.percept_singular_name.clone(),
                sensor.percept_plural_name.clone(),
            )
        };

        self.log_state(
            "update_sensor",
            format!(
                "name={name}, enabled={}, sensitivity={}, singular='{}', plural='{}'",
                enabled_now, sensitivity_now, singular_now, plural_now
            ),
        );

        self.persist_settings()?;

        Ok(())
    }

    /// Enqueues one percept for a configured non-chat sensor.
    pub fn enqueue_sensor_percept(
        &mut self,
        sensor_name: &str,
        content: impl Into<String>,
    ) -> Result<()> {
        if sensor_name == "chat" {
            return Err(anyhow!("chat sensor is managed internally"));
        }

        let sensor = self
            .sensors
            .get_mut(sensor_name)
            .ok_or_else(|| anyhow!("sensor '{sensor_name}' not found"))?;

        let trimmed = content.into().trim().to_string();
        if trimmed.is_empty() {
            return Err(anyhow!("percept content cannot be empty"));
        }

        sensor.enqueue(trimmed);
        Ok(())
    }

    /// Returns all configured sensors ordered by name.
    pub fn sensors(&self) -> Vec<Sensor> {
        let mut sensors = self.sensors.values().cloned().collect::<Vec<_>>();
        sensors.sort_by(|left, right| left.name.cmp(&right.name));
        sensors
    }

    pub fn add_actuator(&mut self, actuator: Actuator) {
        self.actuators.insert(actuator.name.clone(), actuator);
    }

    /// Registers a new actuator and persists settings.
    pub fn register_actuator(&mut self, actuator: Actuator) -> Result<()> {
        self.add_actuator(actuator);
        self.persist_settings()
    }

    /// Imports a plugin package and registers its declared sensors and actuators.
    pub fn import_plugin_package(&mut self, plugin_path: impl AsRef<Path>) -> Result<String> {
        self.import_plugin_package_with_options(plugin_path, true)
    }

    fn import_plugin_package_with_options(
        &mut self,
        plugin_path: impl AsRef<Path>,
        persist_settings: bool,
    ) -> Result<String> {
        let plugin_root = normalize_plugin_root(plugin_path.as_ref(), &self.workspace_root)?;
        let manifest_path = plugin_root.join("looper-plugin.json");
        let raw = fs::read_to_string(&manifest_path).with_context(|| {
            format!(
                "failed to read plugin manifest at '{}'",
                manifest_path.to_string_lossy()
            )
        })?;
        let manifest = serde_json::from_str::<PluginManifest>(&raw).with_context(|| {
            format!(
                "failed to parse plugin manifest at '{}'",
                manifest_path.to_string_lossy()
            )
        })?;
        manifest.validate()?;
        let entry_path = plugin_root.join(manifest.entry.trim());
        if !entry_path.exists() {
            return Err(anyhow!(
                "plugin entry '{}' does not exist",
                entry_path.to_string_lossy()
            ));
        }

        let plugin_name = manifest.name.trim().to_string();
        let plugin_root_text = plugin_root.to_string_lossy().to_string();

        for sensor_def in manifest.sensors {
            let sensor_name = format!("{}:{}", plugin_name, sensor_def.name.trim());
            let mut sensor = Sensor::new(sensor_name, sensor_def.description.trim());
            sensor.ingress = SensorIngressConfig::Plugin(Box::new(PluginSensorIngress {
                plugin: plugin_name.clone(),
                root: plugin_root_text.clone(),
                entry: manifest.entry.clone(),
                sensor: sensor_def.name.trim().to_string(),
                permissions: manifest.permissions.clone(),
                requirements: manifest.requirements.clone(),
            }));
            self.add_sensor(sensor);
        }

        for actuator_def in manifest.actuators {
            let actuator_name = format!("{}:{}", plugin_name, actuator_def.name.trim());
            let actuator = Actuator::plugin(
                actuator_name,
                actuator_def.description.trim(),
                crate::model::PluginActuatorDetails {
                    plugin: plugin_name.clone(),
                    root: plugin_root_text.clone(),
                    entry: manifest.entry.clone(),
                    actuator: actuator_def.name.trim().to_string(),
                    permissions: manifest.permissions.clone(),
                    requirements: manifest.requirements.clone(),
                },
                SafetyPolicy::default(),
            )?;
            self.add_actuator(actuator);
        }

        if persist_settings {
            self.persist_settings()?;
        }
        Ok(plugin_name)
    }

    fn import_bundled_internal_plugins(&mut self) {
        let base_dir = bundled_internal_plugins_dir();
        let entries = match fs::read_dir(&base_dir) {
            Ok(entries) => entries,
            Err(_) => return,
        };

        let mut plugin_dirs = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        plugin_dirs.sort();

        for plugin_dir in plugin_dirs {
            if let Err(error) = self.import_plugin_package_with_options(&plugin_dir, false) {
                self.log_state(
                    "import_bundled_internal_plugins.error",
                    format!("path='{}', error={error}", plugin_dir.to_string_lossy()),
                );
            }
        }
    }

    /// Updates mutable actuator configuration fields for an existing actuator.
    pub fn update_actuator(&mut self, name: &str, update: ActuatorUpdate) -> Result<()> {
        let (require_hitl_now, sandboxed_now, singular_now, plural_now) = {
            let actuator = self
                .actuators
                .get_mut(name)
                .ok_or_else(|| anyhow!("actuator '{name}' not found"))?;

            if let Some(value) = update.description {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    return Err(anyhow!("description cannot be empty"));
                }
                actuator.description = trimmed.to_string();
            }

            if let Some(value) = update.require_hitl {
                actuator.policy.require_hitl = value;
            }

            if let Some(value) = update.sandboxed {
                actuator.policy.sandboxed = value;
            }

            if let Some(value) = update.rate_limit {
                actuator.policy.rate_limit = value;
            }

            if let Some(value) = update.action_singular_name {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    return Err(anyhow!("action singular name cannot be empty"));
                }
                actuator.action_singular_name = trimmed.to_lowercase();
            }

            if let Some(value) = update.action_plural_name {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    return Err(anyhow!("action plural name cannot be empty"));
                }
                actuator.action_plural_name = trimmed.to_lowercase();
            }

            actuator.policy.validate()?;

            (
                actuator.policy.require_hitl,
                actuator.policy.sandboxed,
                actuator.action_singular_name.clone(),
                actuator.action_plural_name.clone(),
            )
        };

        self.log_state(
            "update_actuator",
            format!(
                "name={name}, require_hitl={}, sandboxed={}, singular='{}', plural='{}'",
                require_hitl_now, sandboxed_now, singular_now, plural_now
            ),
        );

        self.persist_settings()?;

        Ok(())
    }

    /// Returns all configured actuators ordered by name.
    pub fn actuators(&self) -> Vec<Actuator> {
        let mut actuators = self.actuators.values().cloned().collect::<Vec<_>>();
        actuators.sort_by(|left, right| left.name.cmp(&right.name));
        actuators
    }

    pub fn register_internal_executor(
        &mut self,
        kind: InternalActuatorKind,
        executor: Box<dyn ActuatorExecutor>,
    ) {
        self.internal_executors.insert(kind, executor);
    }

    /// Enqueues one chat message for processing and persists it.
    pub fn enqueue_chat_message(
        &mut self,
        message: impl Into<String>,
        chat_id: Option<String>,
    ) -> Result<String> {
        let message = message.into();
        let chat_id = normalize_chat_id(chat_id);
        let (unread, queued, sensor_enabled, auto_enabled) = {
            let sensor = self
                .sensors
                .get_mut("chat")
                .ok_or_else(|| anyhow!("chat sensor is not configured"))?;
            let mut auto_enabled = false;
            if !sensor.enabled {
                sensor.enabled = true;
                auto_enabled = true;
            }
            sensor.enqueue_with_chat_id(message.clone(), chat_id.clone());
            (
                sensor.unread_count(),
                sensor.queued_count(),
                sensor.enabled,
                auto_enabled,
            )
        };

        if auto_enabled {
            self.log_state("enqueue_chat_message", "chat sensor auto-enabled");
        }
        self.log_state(
            "enqueue_chat_message",
            format!(
                "chat_id={}, len={}, unread={}, queued={}, sensor_enabled={}",
                chat_id,
                message.len(),
                unread,
                queued,
                sensor_enabled
            ),
        );

        if let Some(store) = &self.store {
            store.insert_chat_message(&chat_id, "me", &message, None)?;
        }

        Ok(chat_id)
    }

    /// Lists persisted chat sessions.
    pub fn list_chat_sessions(&self, limit: usize) -> Result<Vec<PersistedChatSession>> {
        match &self.store {
            Some(store) => store.list_chat_sessions(limit),
            None => Ok(Vec::new()),
        }
    }

    /// Lists persisted chat messages for one chat.
    pub fn list_chat_messages(
        &self,
        chat_id: &str,
        after_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<PersistedChatMessage>> {
        match &self.store {
            Some(store) => store.list_chat_messages(chat_id, after_id, limit),
            None => Ok(Vec::new()),
        }
    }

    pub fn pending_approvals(&self) -> Vec<PendingApproval> {
        let mut approvals = self.pending_approvals.values().cloned().collect::<Vec<_>>();
        approvals.sort_by_key(|approval| approval.id);
        approvals
    }

    pub fn approve(&mut self, approval_id: u64) -> Result<Option<ExecutionResult>> {
        let approval = match self.pending_approvals.remove(&approval_id) {
            Some(approval) => approval,
            None => return Ok(None),
        };
        let result = self.execute_recommendation(&approval.recommendation, true)?;
        Ok(Some(result))
    }

    pub fn deny(&mut self, approval_id: u64) -> bool {
        self.pending_approvals.remove(&approval_id).is_some()
    }

    pub fn observability(&self) -> &Observability {
        &self.observability
    }

    pub fn observability_snapshot(&self) -> ObservabilitySnapshot {
        self.observability.snapshot()
    }

    /// Returns the latest loop visualization state for dashboard rendering.
    pub fn loop_visualization_snapshot(&self) -> LoopVisualizationSnapshot {
        self.loop_visualization.snapshot()
    }

    /// Returns phase transition events newer than `after_sequence`.
    pub fn loop_phase_events_since(&self, after_sequence: u64) -> Vec<LoopPhaseTransitionEvent> {
        self.phase_events
            .iter()
            .filter(|event| event.sequence > after_sequence)
            .cloned()
            .collect()
    }

    /// Returns latest emitted phase transition sequence.
    pub fn latest_phase_event_sequence(&self) -> u64 {
        self.next_phase_event_sequence.saturating_sub(1)
    }

    pub async fn run_iteration(&mut self) -> Result<IterationReport> {
        if self.agent_state != AgentState::Running {
            return Err(anyhow!("runtime is not running"));
        }

        self.observability.total_iterations += 1;
        self.loop_visualization.local_loop_count =
            self.loop_visualization.local_loop_count.saturating_add(1);
        self.loop_visualization.local_current_step = LocalLoopStep::GatherNewPercepts;
        self.loop_visualization.frontier_current_step = None;
        self.loop_visualization.surprise_found = false;
        self.loop_visualization.action_required = false;
        self.transition_phase(LoopRuntimePhase::GatherNewPercepts);

        self.loop_visualization.local_current_step = LocalLoopStep::CheckForSurprises;
        self.transition_phase(LoopRuntimePhase::CheckForSurprises);
        self.observability.bump_phase(LoopPhase::SurpriseDetection);

        let sensed = self.collect_new_percepts();
        self.log_state("run_iteration.sensed", format!("count={}", sensed.len()));
        let prior_windows = self.latest_history_windows()?;
        let soul_markdown = self.read_soul_markdown_for_models();
        let local_model = self
            .local_model
            .as_ref()
            .ok_or_else(|| anyhow!("local model is not configured"))?;
        let surprise_response = local_model
            .detect_surprises(LocalModelRequest {
                latest_percepts: sensed.clone(),
                previous_windows: prior_windows,
                soul_markdown: soul_markdown.clone(),
            })
            .await?;
        self.observability.local_model_tokens += surprise_response.token_usage;

        let surprising = surprise_response
            .surprising_indices
            .into_iter()
            .filter_map(|index| sensed.get(index).cloned())
            .collect::<Vec<_>>();

        let high_sensitivity_surprises = sensed
            .iter()
            .filter(|percept| {
                self.sensors
                    .get(&percept.sensor_name)
                    .map(|sensor| sensor.sensitivity_score >= FORCE_SURPRISE_SENSITIVITY_THRESHOLD)
                    .unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>();

        let mut surprising = surprising;
        for percept in high_sensitivity_surprises {
            if !surprising.contains(&percept) {
                surprising.push(percept);
            }
        }

        self.log_state(
            "run_iteration.surprises",
            format!("surprising_count={}", surprising.len()),
        );

        if surprising.is_empty() {
            self.loop_visualization.local_current_step = LocalLoopStep::NoSurprise;
            self.transition_phase(LoopRuntimePhase::Idle);
            let mut report = IterationReport {
                iteration_id: None,
                sensed_percepts: sensed,
                surprising_percepts: Vec::new(),
                planned_actions: Vec::new(),
                action_results: Vec::new(),
                ended_after_surprise_detection: true,
                ended_after_reasoning: false,
            };
            self.persist_iteration(&mut report)?;
            return Ok(report);
        }

        self.loop_visualization.local_current_step = LocalLoopStep::SurpriseFound;
        self.loop_visualization.surprise_found = true;
        self.loop_visualization.frontier_loop_count = self
            .loop_visualization
            .frontier_loop_count
            .saturating_add(1);
        self.loop_visualization.frontier_current_step =
            Some(FrontierLoopStep::DeeperPerceptInvestigation);
        self.transition_phase(LoopRuntimePhase::DeeperPerceptInvestigation);

        self.observability.bump_phase(LoopPhase::Reasoning);
        self.loop_visualization.frontier_current_step = Some(FrontierLoopStep::PlanActions);
        self.transition_phase(LoopRuntimePhase::PlanActions);
        let (mut planned_actions, frontier_surprises) =
            deterministic_actions_from_percepts(&surprising);

        if !frontier_surprises.is_empty() {
            let frontier_model = self
                .frontier_model
                .as_ref()
                .ok_or_else(|| anyhow!("frontier model is not configured"))?;
            let relevant_skills = self.relevant_enabled_skills_for_surprises(&frontier_surprises);
            let plan_response = match frontier_model
                .plan_actions(FrontierModelRequest {
                    surprising_percepts: frontier_surprises,
                    soul_markdown,
                    relevant_skills,
                })
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    if is_frontier_communication_issue(&error) {
                        self.stop(format!("frontier communication failure: {error}"));
                    }
                    return Err(error);
                }
            };

            self.observability.frontier_model_tokens += plan_response.token_usage;
            planned_actions.extend(plan_response.actions);
        }

        if planned_actions.is_empty() {
            self.observability.false_positive_surprises += 1;
            self.log_state("run_iteration.plan", "planned_actions=0");
            self.transition_phase(LoopRuntimePhase::Idle);
            let mut report = IterationReport {
                iteration_id: None,
                sensed_percepts: sensed,
                surprising_percepts: surprising,
                planned_actions,
                action_results: Vec::new(),
                ended_after_surprise_detection: false,
                ended_after_reasoning: true,
            };
            self.persist_iteration(&mut report)?;
            return Ok(report);
        }

        self.observability.bump_phase(LoopPhase::PerformActions);
        self.loop_visualization.action_required = true;
        self.loop_visualization.frontier_current_step = Some(FrontierLoopStep::PerformingActions);
        self.log_state(
            "run_iteration.plan",
            format!("planned_actions={}", planned_actions.len()),
        );
        self.transition_phase(LoopRuntimePhase::ExecuteActions);
        let mut action_results = Vec::with_capacity(planned_actions.len());
        for recommendation in &planned_actions {
            let result = match self.execute_recommendation(recommendation, false) {
                Ok(result) => result,
                Err(error) => {
                    self.log_state(
                        "run_iteration.execute_error",
                        format!(
                            "actuator='{}', action='{}', error={} ",
                            recommendation.actuator_name,
                            recommendation.action.keyword(),
                            error
                        ),
                    );
                    ExecutionResult::Denied(format!(
                        "execution failed for actuator '{}': {}",
                        recommendation.actuator_name, error
                    ))
                }
            };
            if matches!(result, ExecutionResult::Denied(_)) {
                self.observability.failed_tool_executions += 1;
            }
            action_results.push(result);
        }

        self.log_state(
            "run_iteration.execute_results",
            format!("count={}", action_results.len()),
        );

        let mut report = IterationReport {
            iteration_id: None,
            sensed_percepts: sensed,
            surprising_percepts: surprising,
            planned_actions,
            action_results,
            ended_after_surprise_detection: false,
            ended_after_reasoning: false,
        };
        self.persist_iteration(&mut report)?;
        self.transition_phase(LoopRuntimePhase::Idle);
        Ok(report)
    }

    fn transition_phase(&mut self, phase: LoopRuntimePhase) {
        self.loop_visualization.current_phase = phase;
        self.loop_visualization.current_phase_started_at_unix_ms = now_unix_ms();
        self.log_state("phase", format!("{:?}", phase));

        let sequence = self.next_phase_event_sequence;
        self.next_phase_event_sequence = self.next_phase_event_sequence.saturating_add(1);

        self.phase_events.push_back(LoopPhaseTransitionEvent {
            sequence,
            phase,
            loop_visualization: self.loop_visualization.snapshot(),
            emitted_at_unix_ms: now_unix_ms(),
        });

        while self.phase_events.len() > 512 {
            let _ = self.phase_events.pop_front();
        }
    }

    fn collect_new_percepts(&mut self) -> Vec<Percept> {
        let mut all = Vec::new();
        let plugin_overrides = self.plugin_enabled_overrides.clone();
        for sensor in self.sensors.values_mut() {
            if sensor.enabled
                && let SensorIngressConfig::Plugin(details) = &sensor.ingress
            {
                let user_enabled = plugin_overrides
                    .get(&details.plugin)
                    .copied()
                    .unwrap_or(true);
                if plugin_disabled_reason_with_user(user_enabled, &details.requirements).is_none() {
                    let poll_result = poll_plugin_sensor(&self.workspace_root, details);
                    if let Ok(payloads) = poll_result {
                        for payload in payloads {
                            let trimmed = payload.trim();
                            if !trimmed.is_empty() {
                                sensor.enqueue(trimmed.to_string());
                            }
                        }
                    }
                }
            }

            if sensor.enabled {
                if let SensorIngressConfig::Plugin(details) = &sensor.ingress {
                    let user_enabled = plugin_overrides
                        .get(&details.plugin)
                        .copied()
                        .unwrap_or(true);
                    if plugin_disabled_reason_with_user(user_enabled, &details.requirements)
                        .is_some()
                    {
                        continue;
                    }
                }
                all.extend(sensor.sense_unread());
            }
        }
        all
    }

    fn execute_recommendation(
        &mut self,
        recommendation: &RecommendedAction,
        bypass_hitl: bool,
    ) -> Result<ExecutionResult> {
        let requested_name = recommendation.actuator_name.as_str();
        let actuator = self
            .actuators
            .get(requested_name)
            .or_else(|| {
                self.actuators.iter().find_map(|(name, actuator)| {
                    if name.eq_ignore_ascii_case(requested_name) {
                        Some(actuator)
                    } else {
                        None
                    }
                })
            })
            .or_else(|| {
                let required_kind = recommendation.action.internal_kind();
                self.actuators.values().find(|candidate| {
                    matches!(candidate.kind, ActuatorType::Internal(kind) if kind == required_kind)
                })
            })
            .ok_or_else(|| anyhow!("actuator '{}' is not configured", requested_name))?;

        if actuator.policy.require_hitl && !bypass_hitl {
            let approval_id = self.next_approval_id;
            self.next_approval_id = self.next_approval_id.saturating_add(1);
            self.pending_approvals.insert(
                approval_id,
                PendingApproval {
                    id: approval_id,
                    recommendation: recommendation.clone(),
                },
            );
            return Ok(ExecutionResult::RequiresHitl { approval_id });
        }

        let action_keyword = recommendation.action.keyword();
        if let Some(denylist) = &actuator.policy.denylist
            && denylist.iter().any(|entry| entry == action_keyword)
        {
            return Ok(ExecutionResult::Denied(format!(
                "action '{}' denied by policy",
                action_keyword
            )));
        }

        if let Some(allowlist) = &actuator.policy.allowlist
            && !allowlist.iter().any(|entry| entry == action_keyword)
        {
            return Ok(ExecutionResult::Denied(format!(
                "action '{}' not on allowlist",
                action_keyword
            )));
        }

        if let Some(limit) = &actuator.policy.rate_limit {
            let key = actuator.name.clone();
            let current = self.executions_per_actuator.get(&key).copied().unwrap_or(0);
            if current >= limit.max {
                return Ok(ExecutionResult::Denied(format!(
                    "rate limit exceeded for actuator '{}'",
                    actuator.name
                )));
            }
        }

        let output = match &actuator.kind {
            ActuatorType::Internal(kind) => {
                if *kind != recommendation.action.internal_kind() {
                    return Ok(ExecutionResult::Denied(format!(
                        "action '{}' incompatible with actuator '{}'",
                        action_keyword, actuator.name
                    )));
                }

                let executor = self.internal_executors.get(kind).ok_or_else(|| {
                    anyhow!("no executor registered for internal actuator '{:?}'", kind)
                })?;
                executor.execute(&recommendation.action)?
            }
            ActuatorType::Mcp(details) => format!(
                "MCP actuator '{}' queued request to {} ({:?})",
                actuator.name, details.url, details.connection
            ),
            ActuatorType::Workflow(details) => {
                format!(
                    "workflow '{}' accepted {} cells",
                    details.name,
                    details.cells.len()
                )
            }
            ActuatorType::Plugin(details) => {
                if let Some(reason) =
                    self.plugin_disabled_reason(&details.plugin, &details.requirements)
                {
                    return Ok(ExecutionResult::Denied(format!(
                        "plugin '{}' is disabled: {}",
                        details.plugin, reason
                    )));
                }
                execute_plugin_actuator(&self.workspace_root, details, &recommendation.action)?
            }
        };

        *self
            .executions_per_actuator
            .entry(actuator.name.clone())
            .or_insert(0) += 1;
        Ok(ExecutionResult::Executed { output })
    }

    fn latest_history_windows(&self) -> Result<Vec<Vec<String>>> {
        match &self.store {
            Some(store) => store.latest_percept_windows(10),
            None => Ok(Vec::new()),
        }
    }

    fn read_soul_markdown_for_models(&self) -> String {
        let path = self.workspace_root.join(".agents").join("SOUL.md");
        if let Ok(contents) = fs::read_to_string(path)
            && !contents.trim().is_empty()
        {
            return contents;
        }
        DEFAULT_SOUL_MARKDOWN.to_string()
    }

    fn relevant_enabled_skills_for_surprises(&self, surprising: &[Percept]) -> Vec<SkillContext> {
        let skills_dir = self.workspace_root.join(".agents").join("skills");
        let entries = match fs::read_dir(skills_dir) {
            Ok(entries) => entries,
            Err(_) => return Vec::new(),
        };

        let terms = surprising
            .iter()
            .flat_map(|percept| percept.content.split_whitespace())
            .map(|token| {
                token
                    .trim_matches(|ch: char| !ch.is_ascii_alphanumeric())
                    .to_ascii_lowercase()
            })
            .filter(|token| token.len() >= 3)
            .collect::<Vec<_>>();

        let mut scored = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if !path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                continue;
            }

            let Some(id) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };

            let markdown = match fs::read_to_string(&path) {
                Ok(markdown) => markdown,
                Err(_) => continue,
            };
            if markdown.trim().is_empty() {
                continue;
            }
            if !skill_is_enabled(&markdown) {
                continue;
            }

            let lower_markdown = markdown.to_ascii_lowercase();
            let score = terms
                .iter()
                .filter(|term| lower_markdown.contains(term.as_str()))
                .count();

            if score > 0 {
                scored.push((
                    score,
                    SkillContext {
                        id: id.to_string(),
                        markdown,
                    },
                ));
            }
        }

        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        scored
            .into_iter()
            .take(5)
            .map(|(_, context)| context)
            .collect()
    }

    fn persist_iteration(&self, report: &mut IterationReport) -> Result<()> {
        let Some(store) = &self.store else {
            return Ok(());
        };

        let persisted = PersistedIteration {
            id: 0,
            created_at_unix: SqliteStore::now_unix(),
            sensed_percepts: report.sensed_percepts.clone(),
            surprising_percepts: report.surprising_percepts.clone(),
            planned_actions: report.planned_actions.clone(),
            action_results: report.action_results.clone(),
        };
        let iteration_id = store.insert_iteration(&persisted)?;
        report.iteration_id = Some(iteration_id);

        if let Some(chat_id) = report
            .sensed_percepts
            .iter()
            .rev()
            .find(|percept| percept.sensor_name == "chat")
            .and_then(|percept| percept.chat_id.as_deref())
        {
            for result in &report.action_results {
                let content = match result {
                    ExecutionResult::Executed { output } if !output.trim().is_empty() => {
                        output.trim().to_string()
                    }
                    ExecutionResult::Denied(reason) => format!("action denied ({reason})"),
                    ExecutionResult::RequiresHitl { approval_id } => {
                        format!("action requires HITL (approval id: {approval_id})")
                    }
                    _ => continue,
                };

                store.insert_chat_message(chat_id, "looper", &content, Some(iteration_id))?;
            }
        }
        Ok(())
    }

    fn build_local_model(&self, selection: &ModelSelection) -> Result<Box<dyn LocalModel>> {
        Ok(Box::new(FiddlesticksLocalModel::from_provider(
            selection.provider,
            selection.model.clone(),
            self.key_for(selection.provider),
        )?))
    }

    fn build_frontier_model(&self, selection: &ModelSelection) -> Result<Box<dyn FrontierModel>> {
        Ok(Box::new(FiddlesticksFrontierModel::from_provider(
            selection.provider,
            selection.model.clone(),
            self.key_for(selection.provider),
        )?))
    }

    fn key_for(&self, provider: ModelProviderKind) -> Option<&str> {
        if provider == ModelProviderKind::Ollama {
            return None;
        }
        self.provider_api_keys.get(&provider).map(String::as_str)
    }

    fn persist_api_keys(&self) -> Result<()> {
        let path = self.api_key_store_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let encoded = serde_json::to_string_pretty(&self.provider_api_keys)?;
        fs::write(path, encoded)?;
        Ok(())
    }

    fn persist_settings(&self) -> Result<()> {
        let path = self.settings_store_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut sensors = self
            .sensors
            .values()
            .map(|sensor| PersistedSensorSettings {
                name: sensor.name.clone(),
                description: sensor.description.clone(),
                enabled: sensor.enabled,
                sensitivity_score: sensor.sensitivity_score,
                percept_singular_name: sensor.percept_singular_name.clone(),
                percept_plural_name: sensor.percept_plural_name.clone(),
                ingress: sensor.ingress.clone(),
            })
            .collect::<Vec<_>>();
        sensors.sort_by(|left, right| left.name.cmp(&right.name));

        let mut actuators = self
            .actuators
            .values()
            .map(|actuator| PersistedActuatorSettings {
                name: actuator.name.clone(),
                description: actuator.description.clone(),
                kind: Some(persisted_kind_from_actuator(&actuator.kind)),
                policy: actuator.policy.clone(),
                action_singular_name: actuator.action_singular_name.clone(),
                action_plural_name: actuator.action_plural_name.clone(),
            })
            .collect::<Vec<_>>();
        actuators.sort_by(|left, right| left.name.cmp(&right.name));

        let encoded = serde_json::to_string_pretty(&PersistedAgentSettings {
            local_provider: self.local_selection.as_ref().map(|item| item.provider),
            local_model: self.local_selection.as_ref().map(|item| item.model.clone()),
            frontier_provider: self.frontier_selection.as_ref().map(|item| item.provider),
            frontier_model: self
                .frontier_selection
                .as_ref()
                .map(|item| item.model.clone()),
            sensors,
            actuators,
            plugin_enabled_overrides: self.plugin_enabled_overrides.clone(),
        })?;
        fs::write(path, encoded)?;
        Ok(())
    }

    fn load_persisted_api_keys(&mut self) -> Result<()> {
        let path = self.api_key_store_path();
        if !path.exists() {
            self.log_state("load_persisted_api_keys", "no keys file found");
            return Ok(());
        }

        let raw = fs::read_to_string(path)?;
        let parsed = serde_json::from_str::<HashMap<ModelProviderKind, String>>(&raw)?;
        self.provider_api_keys = parsed
            .into_iter()
            .filter_map(|(provider, key)| {
                let normalized = normalize_api_key_value(&key);
                if normalized.is_empty() {
                    None
                } else {
                    Some((provider, normalized))
                }
            })
            .collect();
        self.log_state(
            "load_persisted_api_keys",
            format!("loaded_keys={}", self.provider_api_keys.len()),
        );
        Ok(())
    }

    fn load_persisted_settings(&mut self) -> Result<()> {
        let path = self.settings_store_path();
        if !path.exists() {
            self.log_state("load_persisted_settings", "no settings file found");
            return Ok(());
        }

        let raw = fs::read_to_string(path)?;
        let parsed = serde_json::from_str::<PersistedAgentSettings>(&raw)?;
        self.plugin_enabled_overrides = parsed.plugin_enabled_overrides.clone();
        if let (
            Some(local_provider),
            Some(local_model),
            Some(frontier_provider),
            Some(frontier_model),
        ) = (
            parsed.local_provider,
            parsed.local_model,
            parsed.frontier_provider,
            parsed.frontier_model,
        ) {
            let local = ModelSelection {
                provider: local_provider,
                model: local_model,
            };
            let frontier = ModelSelection {
                provider: frontier_provider,
                model: frontier_model,
            };

            if self.configure_models(local, frontier).is_err() {
                self.local_selection = None;
                self.frontier_selection = None;
                self.local_model = None;
                self.frontier_model = None;
                self.log_state(
                    "load_persisted_settings",
                    "failed to apply persisted model settings",
                );
            }
        }

        for persisted in parsed.sensors {
            let name = persisted.name.trim();
            if name.is_empty() {
                continue;
            }

            let enabled = if name == "chat" {
                true
            } else {
                persisted.enabled
            };
            let sensitivity_score = persisted.sensitivity_score.min(100);
            let description = persisted.description.trim().to_string();
            let percept_singular_name = persisted.percept_singular_name.trim().to_lowercase();
            let percept_plural_name = persisted.percept_plural_name.trim().to_lowercase();
            let ingress = if name == "chat" {
                SensorIngressConfig::Internal
            } else {
                persisted.ingress
            };

            if let Some(existing) = self.sensors.get_mut(name) {
                existing.enabled = enabled;
                existing.sensitivity_score = sensitivity_score;
                if !description.is_empty() {
                    existing.description = description.clone();
                }
                if !percept_singular_name.is_empty() {
                    existing.percept_singular_name = percept_singular_name.clone();
                }
                if !percept_plural_name.is_empty() {
                    existing.percept_plural_name = percept_plural_name.clone();
                }
                existing.ingress = ingress.clone();
                continue;
            }

            if description.is_empty() {
                continue;
            }

            let mut sensor = Sensor::with_sensitivity_score(name, description, sensitivity_score);
            sensor.enabled = enabled;
            if !percept_singular_name.is_empty() {
                sensor.percept_singular_name = percept_singular_name;
            }
            if !percept_plural_name.is_empty() {
                sensor.percept_plural_name = percept_plural_name;
            }
            sensor.ingress = ingress;
            self.sensors.insert(sensor.name.clone(), sensor);
        }

        for persisted in parsed.actuators {
            let name = persisted.name.trim();
            if name.is_empty() {
                continue;
            }

            if persisted.policy.validate().is_err() {
                continue;
            }

            let description = persisted.description.trim();
            if description.is_empty() {
                continue;
            }

            let kind = persisted.kind.or_else(|| infer_legacy_actuator_kind(name));
            let Some(kind) = kind else {
                continue;
            };

            let Ok(mut actuator) =
                actuator_from_persisted_kind(name, description, kind, persisted.policy)
            else {
                continue;
            };

            let singular = persisted.action_singular_name.trim();
            if !singular.is_empty() {
                actuator.action_singular_name = singular.to_lowercase();
            }

            let plural = persisted.action_plural_name.trim();
            if !plural.is_empty() {
                actuator.action_plural_name = plural.to_lowercase();
            }

            self.actuators.insert(actuator.name.clone(), actuator);
        }

        self.log_state("load_persisted_settings", "persisted settings applied");

        Ok(())
    }

    fn api_key_store_path(&self) -> PathBuf {
        self.workspace_root.join("keys.json")
    }

    fn settings_store_path(&self) -> PathBuf {
        self.workspace_root.join("agent-settings.json")
    }

    fn state_log_path(&self) -> PathBuf {
        self.workspace_root.join("agent-states.log")
    }

    fn log_state(&self, event: &str, detail: impl AsRef<str>) {
        let path = self.state_log_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{}\t{}\t{}", now_unix_ms(), event, detail.as_ref());
        }
    }
}

impl Default for LooperRuntime {
    fn default() -> Self {
        Self::new()
    }
}

fn default_store_path() -> PathBuf {
    looper_user_dir().join("looper.db")
}

/// Returns the default storage directory used by the agent.
pub fn default_agent_workspace_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("LOOPER_WORKSPACE_ROOT") {
        return PathBuf::from(path);
    }

    looper_user_dir().join("workspace")
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn normalize_chat_id(chat_id: Option<String>) -> String {
    let trimmed = chat_id.unwrap_or_default().trim().to_string();
    if !trimmed.is_empty() {
        return trimmed;
    }

    format!("chat-{}", now_unix_ms())
}

fn default_sensor_ingress() -> SensorIngressConfig {
    SensorIngressConfig::RestApi {
        format: SensorRestFormat::Text,
    }
}

fn skill_is_enabled(markdown: &str) -> bool {
    let first_lines = markdown.lines().take(16).map(str::trim).collect::<Vec<_>>();
    for line in first_lines {
        if line.eq_ignore_ascii_case("enabled: false") {
            return false;
        }
    }
    true
}

fn looper_user_dir() -> PathBuf {
    if let Some(home) = user_home_dir() {
        return home.join(".looper");
    }

    std::env::temp_dir().join(".looper")
}

fn user_home_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .or_else(|| {
                let drive = std::env::var_os("HOMEDRIVE")?;
                let path = std::env::var_os("HOMEPATH")?;
                Some(PathBuf::from(format!(
                    "{}{}",
                    drive.to_string_lossy(),
                    path.to_string_lossy()
                )))
            })
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
    }

    std::env::var_os("HOME").map(PathBuf::from)
}

fn normalize_api_key_value(raw: &str) -> String {
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

fn is_frontier_communication_issue(error: &anyhow::Error) -> bool {
    let lower = format!("{error:#}").to_lowercase();
    lower.contains("rate")
        || lower.contains("token")
        || lower.contains("timeout")
        || lower.contains("network")
        || lower.contains("transport")
        || lower.contains("429")
}

fn deterministic_actions_from_percepts(
    percepts: &[Percept],
) -> (Vec<RecommendedAction>, Vec<Percept>) {
    let mut actions = Vec::new();
    let mut remaining = Vec::new();

    for percept in percepts {
        if let Some(action) = deterministic_action_from_percept(percept) {
            actions.push(action);
        } else {
            remaining.push(percept.clone());
        }
    }

    (actions, remaining)
}

fn deterministic_action_from_percept(percept: &Percept) -> Option<RecommendedAction> {
    let signal = parse_plugin_route_signal(&percept.content)?;

    Some(RecommendedAction {
        actuator_name: signal.actuator_name,
        action: Action::ChatResponse {
            message: signal.action_message,
        },
    })
}

fn persisted_kind_from_actuator(kind: &ActuatorType) -> PersistedActuatorKind {
    match kind {
        ActuatorType::Internal(internal) => PersistedActuatorKind::Internal {
            kind: match internal {
                InternalActuatorKind::Chat => "chat",
                InternalActuatorKind::Grep => "grep",
                InternalActuatorKind::Glob => "glob",
                InternalActuatorKind::Shell => "shell",
                InternalActuatorKind::WebSearch => "web_search",
            }
            .to_string(),
        },
        ActuatorType::Mcp(details) => PersistedActuatorKind::Mcp {
            details: details.clone(),
        },
        ActuatorType::Workflow(details) => PersistedActuatorKind::Workflow {
            details: details.clone(),
        },
        ActuatorType::Plugin(details) => PersistedActuatorKind::Plugin {
            details: details.clone(),
        },
    }
}

fn actuator_from_persisted_kind(
    name: &str,
    description: &str,
    kind: PersistedActuatorKind,
    policy: SafetyPolicy,
) -> Result<Actuator> {
    match kind {
        PersistedActuatorKind::Internal { kind } => {
            let internal = match kind.trim().to_ascii_lowercase().as_str() {
                "chat" => InternalActuatorKind::Chat,
                "grep" => InternalActuatorKind::Grep,
                "glob" => InternalActuatorKind::Glob,
                "shell" => InternalActuatorKind::Shell,
                "web_search" => InternalActuatorKind::WebSearch,
                _ => return Err(anyhow!("unknown internal actuator kind")),
            };
            Actuator::internal(name, description, internal, policy)
        }
        PersistedActuatorKind::Mcp { details } => Actuator::mcp(name, description, details, policy),
        PersistedActuatorKind::Workflow { details } => {
            Actuator::workflow(name, description, details, policy)
        }
        PersistedActuatorKind::Plugin { details } => {
            Actuator::plugin(name, description, *details, policy)
        }
    }
}

fn infer_legacy_actuator_kind(name: &str) -> Option<PersistedActuatorKind> {
    let kind = match name.trim().to_ascii_lowercase().as_str() {
        "chat" => "chat",
        "grep" => "grep",
        "glob" => "glob",
        "shell" => "shell",
        "web_search" => "web_search",
        _ => return None,
    };
    Some(PersistedActuatorKind::Internal {
        kind: kind.to_string(),
    })
}

fn normalize_plugin_root(requested: &Path, workspace_root: &Path) -> Result<PathBuf> {
    let root = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        workspace_root.join(requested)
    };
    if !root.exists() {
        return Err(anyhow!(
            "plugin directory '{}' does not exist",
            root.to_string_lossy()
        ));
    }
    if !root.is_dir() {
        return Err(anyhow!(
            "plugin path '{}' is not a directory",
            root.to_string_lossy()
        ));
    }
    Ok(root)
}

fn bundled_internal_plugins_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("looper-plugins")
        .join("internal")
}

fn plugin_disabled_reason_with_user(
    user_enabled: bool,
    requirements: &PluginRequirements,
) -> Option<String> {
    let mut reasons = Vec::new();
    if !user_enabled {
        reasons.push("disabled in settings".to_string());
    }
    if let Some(reason) = plugin_requirements_disabled_reason(requirements) {
        reasons.push(reason);
    }
    if reasons.is_empty() {
        None
    } else {
        Some(reasons.join("; "))
    }
}

fn plugin_requirements_disabled_reason(requirements: &PluginRequirements) -> Option<String> {
    let mut reasons = Vec::new();

    for command in &requirements.command_all {
        if !command_available(command) {
            reasons.push(format!("missing command '{}'", command));
        }
    }

    if !requirements.command_any.is_empty()
        && !requirements
            .command_any
            .iter()
            .any(|command| command_available(command))
    {
        reasons.push(format!(
            "requires one of: {}",
            requirements.command_any.join(", ")
        ));
    }

    for env_key in &requirements.env_all {
        let configured = std::env::var_os(env_key)
            .map(|value| !value.to_string_lossy().trim().is_empty())
            .unwrap_or(false);
        if !configured {
            reasons.push(format!("set environment variable '{}'", env_key));
        }
    }

    if reasons.is_empty() {
        return None;
    }

    let guidance = requirements
        .message
        .as_ref()
        .map(|message| message.trim())
        .filter(|message| !message.is_empty())
        .map(ToString::to_string);

    Some(match guidance {
        Some(message) => format!("{} ({})", message, reasons.join("; ")),
        None => reasons.join("; "),
    })
}

fn command_available(name: &str) -> bool {
    if name.trim().is_empty() {
        return false;
    }

    Command::new(name)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

fn poll_plugin_sensor(workspace_root: &Path, details: &PluginSensorIngress) -> Result<Vec<String>> {
    details.validate()?;

    let payload = serde_json::json!({
        "mode": "sensor",
        "plugin": details.plugin,
        "sensor": details.sensor,
        "workspace_root": workspace_root.to_string_lossy().to_string(),
    });

    let output = run_deno_plugin(
        workspace_root,
        &details.root,
        &details.entry,
        &details.permissions,
        &payload,
    )?;

    if let Some(items) = output.as_array() {
        let percepts = items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| item.to_string())
            })
            .collect::<Vec<_>>();
        return Ok(percepts);
    }

    if let Some(items) = output.get("percepts").and_then(|value| value.as_array()) {
        let percepts = items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| item.to_string())
            })
            .collect::<Vec<_>>();
        return Ok(percepts);
    }

    Ok(Vec::new())
}

fn execute_plugin_actuator(
    workspace_root: &Path,
    details: &crate::model::PluginActuatorDetails,
    action: &crate::model::Action,
) -> Result<String> {
    details.validate()?;

    let payload = serde_json::json!({
        "mode": "actuator",
        "plugin": details.plugin,
        "actuator": details.actuator,
        "action": action,
        "workspace_root": workspace_root.to_string_lossy().to_string(),
    });

    let output = run_deno_plugin(
        workspace_root,
        &details.root,
        &details.entry,
        &details.permissions,
        &payload,
    )?;

    if let Some(text) = output.get("output").and_then(|value| value.as_str()) {
        return Ok(text.to_string());
    }
    Ok(output.to_string())
}

fn run_deno_plugin(
    workspace_root: &Path,
    plugin_root_raw: &str,
    entry_raw: &str,
    permissions: &crate::model::DenoPermissions,
    payload: &serde_json::Value,
) -> Result<serde_json::Value> {
    let plugin_root = normalize_plugin_root(Path::new(plugin_root_raw), workspace_root)?;
    let entry_path = plugin_root.join(entry_raw.trim());
    if !entry_path.exists() {
        return Err(anyhow!(
            "plugin entry '{}' does not exist",
            entry_path.to_string_lossy()
        ));
    }

    let payload_text = serde_json::to_string(payload)?;
    let mut command = std::process::Command::new("deno");
    command.arg("run").arg("--quiet");
    for arg in deno_permission_args(permissions) {
        command.arg(arg);
    }
    command
        .arg(entry_path.to_string_lossy().to_string())
        .arg("--looper-payload")
        .arg(payload_text)
        .current_dir(plugin_root);

    let output = command.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(anyhow!("plugin command failed: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        return Ok(serde_json::Value::Null);
    }

    serde_json::from_str::<serde_json::Value>(&stdout)
        .with_context(|| format!("plugin output is not valid JSON: {stdout}"))
}

fn deno_permission_args(permissions: &crate::model::DenoPermissions) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(flag) = permission_flag("allow-read", &permissions.read) {
        args.push(flag);
    }
    if let Some(flag) = permission_flag("allow-write", &permissions.write) {
        args.push(flag);
    }
    if let Some(flag) = permission_flag("allow-net", &permissions.net) {
        args.push(flag);
    }
    if let Some(flag) = permission_flag("allow-env", &permissions.env) {
        args.push(flag);
    }
    if let Some(flag) = permission_flag("allow-run", &permissions.run) {
        args.push(flag);
    }
    args
}

fn permission_flag(name: &str, values: &[String]) -> Option<String> {
    let joined = values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(",");
    if joined.is_empty() {
        None
    } else {
        Some(format!("--{name}={joined}"))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::model::{Action, ActuatorType, RateLimit, RateLimitPeriod, SensorIngressConfig};

    use super::*;

    fn unique_test_workspace(prefix: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "looper-test-{prefix}-{}-{}-{sequence}",
            std::process::id(),
            now_unix_ms()
        ))
    }

    #[tokio::test]
    async fn chat_sensor_is_always_surprising_when_sensitivity_is_high() {
        let mut runtime = LooperRuntime::with_internal_defaults().expect("defaults should build");
        runtime.use_rule_models_for_testing();
        runtime.disable_store();
        runtime.start().expect("start should succeed");
        runtime
            .enqueue_chat_message("routine status update", None)
            .expect("chat sensor should exist");

        let report = runtime
            .run_iteration()
            .await
            .expect("iteration should complete");
        assert!(!report.ended_after_surprise_detection);
        assert!(!report.surprising_percepts.is_empty());
    }

    #[tokio::test]
    async fn surprising_percept_executes_real_web_search_executor() {
        let mut runtime = LooperRuntime::with_internal_defaults().expect("defaults should build");
        runtime.use_rule_models_for_testing();
        runtime.disable_store();
        runtime.start().expect("start should succeed");
        runtime
            .enqueue_chat_message("please search docs for model guidance", None)
            .expect("chat sensor should exist");

        let report = runtime
            .run_iteration()
            .await
            .expect("iteration should complete");
        assert!(!report.ended_after_surprise_detection);
        assert_eq!(report.planned_actions.len(), 1);
        assert!(matches!(
            report.action_results[0],
            ExecutionResult::Executed { .. }
        ));
    }

    #[tokio::test]
    async fn denied_action_counts_as_failed_execution() {
        let mut runtime = LooperRuntime::with_internal_defaults().expect("defaults should build");
        runtime.use_rule_models_for_testing();
        runtime.disable_store();
        runtime.start().expect("start should succeed");
        let shell = Actuator::internal(
            "shell",
            "shell",
            InternalActuatorKind::Shell,
            SafetyPolicy {
                denylist: Some(vec!["shell".to_string()]),
                ..SafetyPolicy::default()
            },
        )
        .expect("policy should be valid");
        runtime.add_actuator(shell);
        runtime
            .enqueue_chat_message("run cargo test", None)
            .expect("chat sensor should exist");

        let report = runtime
            .run_iteration()
            .await
            .expect("iteration should complete");
        assert!(matches!(
            report.action_results.first(),
            Some(ExecutionResult::Denied(_))
        ));
        assert_eq!(runtime.observability().failed_tool_executions, 1);
    }

    #[tokio::test]
    async fn loop_visualization_tracks_no_surprise_outcome() {
        let mut runtime = LooperRuntime::with_internal_defaults().expect("defaults should build");
        runtime.use_rule_models_for_testing();
        runtime.disable_store();
        runtime.start().expect("start should succeed");

        let report = runtime
            .run_iteration()
            .await
            .expect("iteration should complete");
        assert!(report.ended_after_surprise_detection);

        let snapshot = runtime.loop_visualization_snapshot();
        assert_eq!(snapshot.local_current_step, LocalLoopStep::NoSurprise);
        assert_eq!(snapshot.frontier_current_step, None);
        assert!(!snapshot.surprise_found);
        assert!(!snapshot.action_required);
        assert_eq!(snapshot.local_loop_count, 1);
        assert_eq!(snapshot.frontier_loop_count, 0);
    }

    #[tokio::test]
    async fn loop_visualization_tracks_frontier_action_outcome() {
        let mut runtime = LooperRuntime::with_internal_defaults().expect("defaults should build");
        runtime.use_rule_models_for_testing();
        runtime.disable_store();
        runtime.start().expect("start should succeed");
        runtime
            .enqueue_chat_message("please search docs for model guidance", None)
            .expect("chat sensor should exist");

        let report = runtime
            .run_iteration()
            .await
            .expect("iteration should complete");
        assert!(!report.action_results.is_empty());

        let snapshot = runtime.loop_visualization_snapshot();
        assert_eq!(snapshot.local_current_step, LocalLoopStep::SurpriseFound);
        assert!(matches!(
            snapshot.frontier_current_step,
            Some(FrontierLoopStep::PlanActions | FrontierLoopStep::PerformingActions)
        ));
        assert!(snapshot.surprise_found);
        assert!(snapshot.action_required);
        assert_eq!(snapshot.local_loop_count, 1);
        assert_eq!(snapshot.frontier_loop_count, 1);
    }

    #[tokio::test]
    async fn chat_response_actuator_name_alias_executes_chat_actuator() {
        let mut runtime = LooperRuntime::with_internal_defaults().expect("defaults should build");
        runtime.disable_store();

        let result = runtime
            .execute_recommendation(
                &RecommendedAction {
                    actuator_name: "ChatResponse".to_string(),
                    action: Action::ChatResponse {
                        message: "Hello".to_string(),
                    },
                },
                false,
            )
            .expect("chat alias should execute");

        assert!(matches!(result, ExecutionResult::Executed { .. }));
    }

    #[tokio::test]
    async fn deterministic_plugin_signal_routes_without_frontier_planning() {
        let workspace = unique_test_workspace("deterministic-plugin-route");
        fs::create_dir_all(&workspace).expect("temp workspace should be created");
        fs::write(workspace.join("mod.ts"), "console.log('{}');")
            .expect("plugin entry should be created");

        let mut runtime = LooperRuntime::with_internal_defaults_for_workspace(&workspace)
            .expect("defaults should build");
        runtime.disable_store();
        runtime.use_rule_models_for_testing();
        runtime
            .register_actuator(
                Actuator::plugin(
                    "git_commit_guard:desktop_notify_secrets",
                    "routes desktop notifications",
                    crate::model::PluginActuatorDetails {
                        plugin: "git_commit_guard".to_string(),
                        root: workspace.to_string_lossy().to_string(),
                        entry: "mod.ts".to_string(),
                        actuator: "desktop_notify_secrets".to_string(),
                        permissions: crate::model::DenoPermissions::default(),
                        requirements: crate::model::PluginRequirements::default(),
                    },
                    SafetyPolicy {
                        require_hitl: true,
                        ..SafetyPolicy::default()
                    },
                )
                .expect("plugin actuator should be valid"),
            )
            .expect("plugin actuator should register");
        runtime.start().expect("runtime should start");

        runtime
            .enqueue_chat_message(
                r#"{"looper_signal":"plugin_route_v1","event":"new_risky_commit","route_to_actuator":"git_commit_guard:desktop_notify_secrets","action_message":"Notify desktop about risky commit abc1234."}"#,
                None,
            )
            .expect("chat sensor should exist");

        let report = runtime
            .run_iteration()
            .await
            .expect("iteration should complete");
        assert_eq!(report.planned_actions.len(), 1);
        assert_eq!(
            report.planned_actions[0].actuator_name,
            "git_commit_guard:desktop_notify_secrets"
        );
        assert!(matches!(
            report.action_results.first(),
            Some(ExecutionResult::RequiresHitl { .. })
        ));
    }

    #[test]
    fn update_sensor_rejects_empty_description() {
        let workspace = unique_test_workspace("sensor-validation");
        fs::create_dir_all(&workspace).expect("temp workspace should be created");

        let mut runtime = LooperRuntime::with_internal_defaults_for_workspace(&workspace)
            .expect("defaults should build");
        runtime.disable_store();

        let error = runtime
            .update_sensor(
                "chat",
                SensorUpdate {
                    description: Some("   ".to_string()),
                    ..SensorUpdate::default()
                },
            )
            .expect_err("empty description should be rejected");
        assert!(error.to_string().contains("description cannot be empty"));
    }

    #[test]
    fn sensor_settings_persist_between_runtime_instances() {
        let workspace = unique_test_workspace("sensor-settings");
        fs::create_dir_all(&workspace).expect("temp workspace should be created");

        let mut runtime = LooperRuntime::with_internal_defaults_for_workspace(&workspace)
            .expect("defaults should build");
        runtime.disable_store();
        runtime
            .register_sensor(Sensor::with_sensitivity_score("inbox", "incoming mail", 30))
            .expect("sensor should register");
        runtime
            .update_sensor(
                "inbox",
                SensorUpdate {
                    enabled: Some(false),
                    sensitivity_score: Some(88),
                    description: Some("Email alerts from monitored inboxes".to_string()),
                    percept_singular_name: Some("Email Alert".to_string()),
                    percept_plural_name: Some("Email Alerts".to_string()),
                    ingress: Some(SensorIngressConfig::Directory {
                        path: "C:/tmp/inbox".to_string(),
                    }),
                },
            )
            .expect("sensor should update");

        let reloaded = LooperRuntime::with_internal_defaults_for_workspace(&workspace)
            .expect("defaults should reload");
        let sensor = reloaded
            .sensors()
            .into_iter()
            .find(|item| item.name == "inbox")
            .expect("persisted sensor should exist");

        assert!(!sensor.enabled);
        assert_eq!(sensor.sensitivity_score, 88);
        assert_eq!(sensor.description, "Email alerts from monitored inboxes");
        assert_eq!(sensor.percept_singular_name, "email alert");
        assert_eq!(sensor.percept_plural_name, "email alerts");
        assert_eq!(
            sensor.ingress,
            SensorIngressConfig::Directory {
                path: "C:/tmp/inbox".to_string()
            }
        );
    }

    #[test]
    fn update_actuator_rejects_empty_description() {
        let workspace = unique_test_workspace("actuator-validation");
        fs::create_dir_all(&workspace).expect("temp workspace should be created");

        let mut runtime = LooperRuntime::with_internal_defaults_for_workspace(&workspace)
            .expect("defaults should build");
        runtime.disable_store();

        let error = runtime
            .update_actuator(
                "web_search",
                ActuatorUpdate {
                    description: Some("   ".to_string()),
                    ..ActuatorUpdate::default()
                },
            )
            .expect_err("empty description should be rejected");
        assert!(error.to_string().contains("description cannot be empty"));
    }

    #[test]
    fn actuator_settings_persist_between_runtime_instances() {
        let workspace = unique_test_workspace("actuator-settings");
        fs::create_dir_all(&workspace).expect("temp workspace should be created");

        let mut runtime = LooperRuntime::with_internal_defaults_for_workspace(&workspace)
            .expect("defaults should build");
        runtime.disable_store();
        runtime
            .update_actuator(
                "web_search",
                ActuatorUpdate {
                    description: Some(
                        "Searches external documentation with restrictions".to_string(),
                    ),
                    require_hitl: Some(true),
                    sandboxed: Some(true),
                    rate_limit: Some(Some(RateLimit {
                        max: 3,
                        per: RateLimitPeriod::Hour,
                    })),
                    action_singular_name: Some("External Search".to_string()),
                    action_plural_name: Some("External Searches".to_string()),
                },
            )
            .expect("actuator should update");

        let reloaded = LooperRuntime::with_internal_defaults_for_workspace(&workspace)
            .expect("defaults should reload");
        let actuator = reloaded
            .actuators()
            .into_iter()
            .find(|item| item.name == "web_search")
            .expect("persisted actuator should exist");

        assert_eq!(
            actuator.description,
            "Searches external documentation with restrictions"
        );
        assert!(actuator.policy.require_hitl);
        assert!(actuator.policy.sandboxed);
        assert_eq!(
            actuator.policy.rate_limit,
            Some(RateLimit {
                max: 3,
                per: RateLimitPeriod::Hour,
            })
        );
        assert_eq!(actuator.action_singular_name, "external search");
        assert_eq!(actuator.action_plural_name, "external searches");
    }

    #[test]
    fn plugin_import_registers_and_persists_sensor_and_actuator() {
        let workspace = unique_test_workspace("plugin-import");
        let plugin_dir = workspace.join("plugins").join("demo-plugin");
        fs::create_dir_all(&plugin_dir).expect("plugin dir should be created");
        fs::write(plugin_dir.join("mod.ts"), "console.log('{}');")
            .expect("entry file should be written");
        fs::write(
            plugin_dir.join("looper-plugin.json"),
            r#"{
  "name": "demo",
  "version": "0.1.0",
  "entry": "mod.ts",
  "permissions": {
    "read": ["."],
    "net": []
  },
  "sensors": [
    { "name": "alerts", "description": "Demo alert stream" }
  ],
  "actuators": [
    { "name": "notify", "description": "Demo notifier" }
  ]
}"#,
        )
        .expect("manifest should be written");

        let mut runtime = LooperRuntime::with_internal_defaults_for_workspace(&workspace)
            .expect("defaults should build");
        runtime.disable_store();
        let imported = runtime
            .import_plugin_package(&plugin_dir)
            .expect("plugin should import");
        assert_eq!(imported, "demo");

        let sensor = runtime
            .sensors()
            .into_iter()
            .find(|item| item.name == "demo:alerts")
            .expect("plugin sensor should exist");
        assert!(matches!(sensor.ingress, SensorIngressConfig::Plugin(_)));

        let actuator = runtime
            .actuators()
            .into_iter()
            .find(|item| item.name == "demo:notify")
            .expect("plugin actuator should exist");
        assert!(matches!(actuator.kind, ActuatorType::Plugin(_)));

        let reloaded = LooperRuntime::with_internal_defaults_for_workspace(&workspace)
            .expect("defaults should reload");
        let reloaded_sensor = reloaded
            .sensors()
            .into_iter()
            .find(|item| item.name == "demo:alerts")
            .expect("reloaded plugin sensor should exist");
        assert!(matches!(
            reloaded_sensor.ingress,
            SensorIngressConfig::Plugin(_)
        ));

        let reloaded_actuator = reloaded
            .actuators()
            .into_iter()
            .find(|item| item.name == "demo:notify")
            .expect("reloaded plugin actuator should exist");
        assert!(matches!(reloaded_actuator.kind, ActuatorType::Plugin(_)));
    }

    #[test]
    fn plugin_with_missing_requirements_imports_as_disabled() {
        let workspace = unique_test_workspace("plugin-requirements");
        let plugin_dir = workspace.join("plugins").join("restricted-plugin");
        fs::create_dir_all(&plugin_dir).expect("plugin dir should be created");
        fs::write(plugin_dir.join("mod.ts"), "console.log('{}');")
            .expect("entry file should be written");
        fs::write(
            plugin_dir.join("looper-plugin.json"),
            r#"{
  "name": "restricted",
  "version": "0.1.0",
  "entry": "mod.ts",
  "requirements": {
    "env_all": ["LOOPER_TEST_PLUGIN_REQUIREMENT_DO_NOT_SET"],
    "message": "Set required env var"
  },
  "sensors": [
    { "name": "alerts", "description": "Restricted alert stream" }
  ],
  "actuators": [
    { "name": "notify", "description": "Restricted notifier" }
  ]
}"#,
        )
        .expect("manifest should be written");

        let mut runtime = LooperRuntime::with_internal_defaults_for_workspace(&workspace)
            .expect("defaults should build");
        runtime.disable_store();
        runtime
            .import_plugin_package(&plugin_dir)
            .expect("plugin should import");

        let sensor = runtime
            .sensors()
            .into_iter()
            .find(|item| item.name == "restricted:alerts")
            .expect("plugin sensor should exist");
        assert!(sensor.enabled);

        let actuator = runtime
            .actuators()
            .into_iter()
            .find(|item| item.name == "restricted:notify")
            .expect("plugin actuator should exist");
        assert!(matches!(actuator.kind, ActuatorType::Plugin(_)));

        let result = runtime
            .execute_recommendation(
                &RecommendedAction {
                    actuator_name: "restricted:notify".to_string(),
                    action: Action::ChatResponse {
                        message: "notify".to_string(),
                    },
                },
                false,
            )
            .expect("execution should complete");
        assert!(matches!(
            result,
            ExecutionResult::Denied(message) if message.contains("Set required env var")
        ));
    }

    #[test]
    fn plugin_can_be_disabled_and_enabled_via_runtime_setting() {
        let workspace = unique_test_workspace("plugin-toggle");
        let plugin_dir = workspace.join("plugins").join("toggle-plugin");
        fs::create_dir_all(&plugin_dir).expect("plugin dir should be created");
        fs::write(plugin_dir.join("mod.ts"), "console.log('{}');")
            .expect("entry file should be written");
        fs::write(
            plugin_dir.join("looper-plugin.json"),
            r#"{
  "name": "toggle",
  "version": "0.1.0",
  "entry": "mod.ts",
  "sensors": [
    { "name": "alerts", "description": "Toggle alerts" }
  ],
  "actuators": [
    { "name": "notify", "description": "Toggle notifier" }
  ]
}"#,
        )
        .expect("manifest should be written");

        let mut runtime = LooperRuntime::with_internal_defaults_for_workspace(&workspace)
            .expect("defaults should build");
        runtime.disable_store();
        runtime
            .import_plugin_package(&plugin_dir)
            .expect("plugin should import");

        runtime
            .set_plugin_enabled("toggle", false)
            .expect("plugin should disable");
        assert_eq!(runtime.plugin_user_enabled("toggle"), Some(false));

        runtime
            .set_plugin_enabled("toggle", true)
            .expect("plugin should enable");
        assert_eq!(runtime.plugin_user_enabled("toggle"), Some(true));
    }
}
