use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Result, anyhow};
use serde::Serialize;

use crate::executors::{
    ActuatorExecutor, ChatActuatorExecutor, GlobActuatorExecutor, GrepActuatorExecutor,
    ShellActuatorExecutor, WebSearchActuatorExecutor,
};
use crate::model::{
    Actuator, ActuatorType, ExecutionResult, InternalActuatorKind, PendingApproval, Percept,
    RecommendedAction, SafetyPolicy, Sensor,
};
use crate::models::{
    FiddlesticksFrontierModel, FiddlesticksLocalModel, FrontierModel, FrontierModelRequest,
    LocalModel, LocalModelRequest, RuleBasedFrontierModel, RuleBasedLocalModel,
};
use crate::storage::{PersistedIteration, SqliteStore};

/// Phases of a loop iteration.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum LoopPhase {
    /// Read new percepts from sensors.
    SurpriseDetection,
    /// Frontier reasoning over surprising percepts.
    Reasoning,
    /// Action execution phase.
    PerformActions,
}

impl LoopPhase {
    fn as_key(self) -> &'static str {
        match self {
            LoopPhase::SurpriseDetection => "surprise_detection",
            LoopPhase::Reasoning => "reasoning",
            LoopPhase::PerformActions => "perform_actions",
        }
    }
}

/// Observability counters for loop health.
#[derive(Clone, Debug)]
pub struct Observability {
    /// Execution counts per phase.
    pub phase_execution_counts: HashMap<LoopPhase, u64>,
    /// Approximate local model token usage.
    pub local_model_tokens: u64,
    /// Approximate frontier model token usage.
    pub frontier_model_tokens: u64,
    /// Number of false positive surprises.
    pub false_positive_surprises: u64,
    /// Number of failed tool executions.
    pub failed_tool_executions: u64,
    /// Total iteration count.
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
    /// Increments the execution counter for a loop phase.
    pub fn bump_phase(&mut self, phase: LoopPhase) {
        *self.phase_execution_counts.entry(phase).or_insert(0) += 1;
    }

    /// Calculates loops per minute since startup.
    pub fn loops_per_minute(&self) -> f64 {
        let elapsed_secs = self.start.elapsed().as_secs_f64();
        if elapsed_secs <= f64::EPSILON {
            return 0.0;
        }
        (self.total_iterations as f64 / elapsed_secs) * 60.0
    }

    /// Returns failed tool execution percent.
    pub fn failed_tool_execution_percent(&self) -> f64 {
        if self.total_iterations == 0 {
            return 0.0;
        }
        (self.failed_tool_executions as f64 / self.total_iterations as f64) * 100.0
    }

    /// Returns false positive surprise percent.
    pub fn false_positive_surprise_percent(&self) -> f64 {
        if self.total_iterations == 0 {
            return 0.0;
        }
        (self.false_positive_surprises as f64 / self.total_iterations as f64) * 100.0
    }

    /// Creates a serialization-friendly snapshot of metrics.
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
#[derive(Clone, Debug, Serialize)]
pub struct ObservabilitySnapshot {
    /// Execution counts per phase.
    pub phase_execution_counts: HashMap<String, u64>,
    /// Approximate local model token usage.
    pub local_model_tokens: u64,
    /// Approximate frontier model token usage.
    pub frontier_model_tokens: u64,
    /// Number of false positive surprises.
    pub false_positive_surprises: u64,
    /// Percent of iterations with false positive surprises.
    pub false_positive_surprise_percent: f64,
    /// Number of failed tool executions.
    pub failed_tool_executions: u64,
    /// Percent of iterations with failed executions.
    pub failed_tool_execution_percent: f64,
    /// Total iteration count.
    pub total_iterations: u64,
    /// Loops per minute since startup.
    pub loops_per_minute: f64,
}

/// Output of a completed loop iteration.
#[derive(Clone, Debug)]
pub struct IterationReport {
    /// Persisted database id if storage is enabled.
    pub iteration_id: Option<i64>,
    /// Percepts sensed this iteration.
    pub sensed_percepts: Vec<Percept>,
    /// Percepts marked surprising this iteration.
    pub surprising_percepts: Vec<Percept>,
    /// Planned actions this iteration.
    pub planned_actions: Vec<RecommendedAction>,
    /// Action execution results.
    pub action_results: Vec<ExecutionResult>,
    /// Whether the iteration ended before reasoning.
    pub ended_after_surprise_detection: bool,
    /// Whether the iteration ended before action execution.
    pub ended_after_reasoning: bool,
}

/// First-pass implementation of the Looper sensory loop runtime.
pub struct LooperRuntime {
    sensors: HashMap<String, Sensor>,
    actuators: HashMap<String, Actuator>,
    internal_executors: HashMap<InternalActuatorKind, Box<dyn ActuatorExecutor>>,
    local_model: Box<dyn LocalModel>,
    frontier_model: Box<dyn FrontierModel>,
    observability: Observability,
    executions_per_actuator: HashMap<String, u32>,
    pending_approvals: HashMap<u64, PendingApproval>,
    next_approval_id: u64,
    store: Option<SqliteStore>,
}

impl LooperRuntime {
    /// Creates an empty runtime.
    pub fn new() -> Self {
        Self {
            sensors: HashMap::new(),
            actuators: HashMap::new(),
            internal_executors: HashMap::new(),
            local_model: Box::new(RuleBasedLocalModel),
            frontier_model: Box::new(RuleBasedFrontierModel),
            observability: Observability::default(),
            executions_per_actuator: HashMap::new(),
            pending_approvals: HashMap::new(),
            next_approval_id: 1,
            store: None,
        }
    }

    /// Creates a runtime with default sensors, actuators, executors, and fiddlesticks models.
    pub fn with_internal_defaults() -> Result<Self> {
        let mut runtime = Self::new();
        runtime.add_sensor(Sensor::new(
            "chat",
            "Receiver of chat messages in percept form",
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

        let workspace_root = std::env::current_dir()?;
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

        runtime.configure_fiddlesticks_ollama_models()?;

        let db_path = default_store_path(&workspace_root);
        runtime.attach_store(SqliteStore::new(db_path)?);
        Ok(runtime)
    }

    /// Configures local/frontier models using fiddlesticks Ollama adapters.
    pub fn configure_fiddlesticks_ollama_models(&mut self) -> Result<()> {
        let local_model_name = std::env::var("LOOPER_LOCAL_MODEL")
            .unwrap_or_else(|_| "qwen2.5:3b-instruct".to_string());
        let frontier_model_name = std::env::var("LOOPER_FRONTIER_MODEL")
            .unwrap_or_else(|_| "qwen2.5:7b-instruct".to_string());

        self.set_local_model(Box::new(FiddlesticksLocalModel::from_ollama(
            local_model_name,
        )?));
        self.set_frontier_model(Box::new(FiddlesticksFrontierModel::from_ollama(
            frontier_model_name,
        )?));
        Ok(())
    }

    /// Replaces the local model implementation.
    pub fn set_local_model(&mut self, local_model: Box<dyn LocalModel>) {
        self.local_model = local_model;
    }

    /// Replaces the frontier model implementation.
    pub fn set_frontier_model(&mut self, frontier_model: Box<dyn FrontierModel>) {
        self.frontier_model = frontier_model;
    }

    /// Attaches a persistence store.
    pub fn attach_store(&mut self, store: SqliteStore) {
        self.store = Some(store);
    }

    /// Disables persistence storage.
    pub fn disable_store(&mut self) {
        self.store = None;
    }

    /// Gets persisted iteration by id.
    pub fn get_iteration(&self, id: i64) -> Result<Option<PersistedIteration>> {
        match &self.store {
            Some(store) => store.get_iteration(id),
            None => Ok(None),
        }
    }

    /// Adds or replaces a sensor by name.
    pub fn add_sensor(&mut self, sensor: Sensor) {
        self.sensors.insert(sensor.name.clone(), sensor);
    }

    /// Adds or replaces an actuator by name.
    pub fn add_actuator(&mut self, actuator: Actuator) {
        self.actuators.insert(actuator.name.clone(), actuator);
    }

    /// Registers or replaces an internal tool executor.
    pub fn register_internal_executor(
        &mut self,
        kind: InternalActuatorKind,
        executor: Box<dyn ActuatorExecutor>,
    ) {
        self.internal_executors.insert(kind, executor);
    }

    /// Enqueues a percept on the chat sensor.
    pub fn enqueue_chat_message(&mut self, message: impl Into<String>) -> Result<()> {
        let sensor = self
            .sensors
            .get_mut("chat")
            .ok_or_else(|| anyhow!("chat sensor is not configured"))?;
        sensor.enqueue(message);
        Ok(())
    }

    /// Returns pending approvals.
    pub fn pending_approvals(&self) -> Vec<PendingApproval> {
        let mut approvals = self.pending_approvals.values().cloned().collect::<Vec<_>>();
        approvals.sort_by_key(|approval| approval.id);
        approvals
    }

    /// Approves an action and executes it.
    pub fn approve(&mut self, approval_id: u64) -> Result<Option<ExecutionResult>> {
        let approval = match self.pending_approvals.remove(&approval_id) {
            Some(approval) => approval,
            None => return Ok(None),
        };

        let result = self.execute_recommendation(&approval.recommendation, true)?;
        Ok(Some(result))
    }

    /// Denies an action without execution.
    pub fn deny(&mut self, approval_id: u64) -> bool {
        self.pending_approvals.remove(&approval_id).is_some()
    }

    /// Returns a snapshot of current observability metrics.
    pub fn observability(&self) -> &Observability {
        &self.observability
    }

    /// Returns a serialization-friendly metrics snapshot.
    pub fn observability_snapshot(&self) -> ObservabilitySnapshot {
        self.observability.snapshot()
    }

    /// Executes one full sensory loop iteration.
    pub async fn run_iteration(&mut self) -> Result<IterationReport> {
        self.observability.total_iterations += 1;

        self.observability.bump_phase(LoopPhase::SurpriseDetection);
        let sensed = self.collect_new_percepts();
        let prior_windows = self.latest_history_windows()?;
        let surprise_response = self
            .local_model
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
        let plan_response = self
            .frontier_model
            .plan_actions(FrontierModelRequest {
                surprising_percepts: surprising.clone(),
            })
            .await?;
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
            ActuatorType::Workflow(details) => format!(
                "workflow '{}' accepted {} cells",
                details.name,
                details.cells.len()
            ),
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
}

impl Default for LooperRuntime {
    fn default() -> Self {
        Self::new()
    }
}

fn default_store_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".looper").join("looper.db")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn non_surprising_percept_ends_early() {
        let mut runtime = LooperRuntime::with_internal_defaults().expect("defaults should build");
        runtime.set_local_model(Box::new(RuleBasedLocalModel));
        runtime.set_frontier_model(Box::new(RuleBasedFrontierModel));
        runtime.disable_store();
        runtime
            .enqueue_chat_message("routine status update")
            .expect("chat sensor should exist");

        let report = runtime
            .run_iteration()
            .await
            .expect("iteration should complete");
        assert!(report.ended_after_surprise_detection);
        assert!(report.planned_actions.is_empty());
    }

    #[tokio::test]
    async fn surprising_percept_executes_real_web_search_executor() {
        let mut runtime = LooperRuntime::with_internal_defaults().expect("defaults should build");
        runtime.set_local_model(Box::new(RuleBasedLocalModel));
        runtime.set_frontier_model(Box::new(RuleBasedFrontierModel));
        runtime.disable_store();
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
        assert!(report.iteration_id.is_none());
    }

    #[tokio::test]
    async fn denied_action_counts_as_failed_execution() {
        let mut runtime = LooperRuntime::with_internal_defaults().expect("defaults should build");
        runtime.set_local_model(Box::new(RuleBasedLocalModel));
        runtime.set_frontier_model(Box::new(RuleBasedFrontierModel));
        runtime.disable_store();
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
