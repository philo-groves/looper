use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::executors::{
    ActuatorExecutor, ChatActuatorExecutor, GlobActuatorExecutor, GrepActuatorExecutor,
    ShellActuatorExecutor, WebSearchActuatorExecutor,
};
use crate::model::{
    Actuator, ActuatorType, AgentState, ExecutionResult, InternalActuatorKind, ModelProviderKind,
    ModelSelection, PendingApproval, Percept, RecommendedAction, SafetyPolicy, Sensor,
};
use crate::models::{
    FiddlesticksFrontierModel, FiddlesticksLocalModel, FrontierModel, FrontierModelRequest,
    LocalModel, LocalModelRequest, RuleBasedFrontierModel, RuleBasedLocalModel,
};
use crate::storage::{PersistedIteration, SqliteStore};

const FORCE_SURPRISE_SENSITIVITY_THRESHOLD: u8 = 90;

/// Phases of a loop iteration.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum LoopPhase {
    SurpriseDetection,
    Reasoning,
    PerformActions,
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
    store: Option<SqliteStore>,
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
            store: None,
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
        runtime.add_sensor(Sensor::with_sensitivity_score(
            "chat",
            "Receiver of chat messages in percept form",
            100,
        ));

        runtime.add_actuator(Actuator::internal(
            "chat",
            "Responder of chat messages in action form",
            InternalActuatorKind::Chat,
            SafetyPolicy::default(),
        )?);
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
            self.stop_reason = None;
        }
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
        Ok(())
    }

    pub fn stop(&mut self, reason: impl Into<String>) {
        self.agent_state = AgentState::Stopped;
        self.stop_reason = Some(reason.into());
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

    pub fn add_sensor(&mut self, sensor: Sensor) {
        self.sensors.insert(sensor.name.clone(), sensor);
    }

    pub fn add_actuator(&mut self, actuator: Actuator) {
        self.actuators.insert(actuator.name.clone(), actuator);
    }

    pub fn register_internal_executor(
        &mut self,
        kind: InternalActuatorKind,
        executor: Box<dyn ActuatorExecutor>,
    ) {
        self.internal_executors.insert(kind, executor);
    }

    pub fn enqueue_chat_message(&mut self, message: impl Into<String>) -> Result<()> {
        let sensor = self
            .sensors
            .get_mut("chat")
            .ok_or_else(|| anyhow!("chat sensor is not configured"))?;
        sensor.enqueue(message);
        Ok(())
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

    pub async fn run_iteration(&mut self) -> Result<IterationReport> {
        if self.agent_state != AgentState::Running {
            return Err(anyhow!("runtime is not running"));
        }

        self.observability.total_iterations += 1;
        self.observability.bump_phase(LoopPhase::SurpriseDetection);

        let sensed = self.collect_new_percepts();
        let prior_windows = self.latest_history_windows()?;
        let local_model = self
            .local_model
            .as_ref()
            .ok_or_else(|| anyhow!("local model is not configured"))?;
        let surprise_response = local_model
            .detect_surprises(LocalModelRequest {
                latest_percepts: sensed.clone(),
                previous_windows: prior_windows,
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

        if surprising.is_empty() {
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

        self.observability.bump_phase(LoopPhase::Reasoning);
        let frontier_model = self
            .frontier_model
            .as_ref()
            .ok_or_else(|| anyhow!("frontier model is not configured"))?;
        let plan_response = match frontier_model
            .plan_actions(FrontierModelRequest {
                surprising_percepts: surprising.clone(),
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
        let planned_actions = plan_response.actions;

        if planned_actions.is_empty() {
            self.observability.false_positive_surprises += 1;
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
        let mut action_results = Vec::with_capacity(planned_actions.len());
        for recommendation in &planned_actions {
            let result = self.execute_recommendation(recommendation, false)?;
            if matches!(result, ExecutionResult::Denied(_)) {
                self.observability.failed_tool_executions += 1;
            }
            action_results.push(result);
        }

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
        Ok(report)
    }

    fn collect_new_percepts(&mut self) -> Vec<Percept> {
        let mut all = Vec::new();
        for sensor in self.sensors.values_mut() {
            if sensor.enabled {
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
        let actuator = self
            .actuators
            .get(&recommendation.actuator_name)
            .ok_or_else(|| {
                anyhow!(
                    "actuator '{}' is not configured",
                    recommendation.actuator_name
                )
            })?;

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
        report.iteration_id = Some(store.insert_iteration(&persisted)?);
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
        let path = api_key_store_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let encoded = serde_json::to_string_pretty(&self.provider_api_keys)?;
        fs::write(path, encoded)?;
        Ok(())
    }

    fn load_persisted_api_keys(&mut self) -> Result<()> {
        let path = api_key_store_path();
        if !path.exists() {
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
        Ok(())
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

fn api_key_store_path() -> PathBuf {
    looper_user_dir().join("keys.json")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn chat_sensor_is_always_surprising_when_sensitivity_is_high() {
        let mut runtime = LooperRuntime::with_internal_defaults().expect("defaults should build");
        runtime.use_rule_models_for_testing();
        runtime.disable_store();
        runtime.start().expect("start should succeed");
        runtime
            .enqueue_chat_message("routine status update")
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
            .enqueue_chat_message("please search docs for model guidance")
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
            .enqueue_chat_message("run cargo test")
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
}
