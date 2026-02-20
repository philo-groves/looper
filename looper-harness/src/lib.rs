use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Result, anyhow};
use glob::glob;
use regex::Regex;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

/// A single unit of perception received from a sensor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Percept {
    /// Name of the sensor that emitted this percept.
    pub sensor_name: String,
    /// Human-readable percept payload.
    pub content: String,
}

impl Percept {
    /// Creates a new percept.
    pub fn new(sensor_name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            sensor_name: sensor_name.into(),
            content: content.into(),
        }
    }
}

/// A receiver of percepts.
#[derive(Clone, Debug)]
pub struct Sensor {
    /// Sensor name.
    pub name: String,
    /// Description of the percepts emitted by this sensor.
    pub description: String,
    /// Whether the sensor is currently active.
    pub enabled: bool,
    queue: VecDeque<Percept>,
    unread_start: usize,
}

impl Sensor {
    /// Creates a new enabled sensor.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            enabled: true,
            queue: VecDeque::new(),
            unread_start: 0,
        }
    }

    /// Enqueues a percept into the sensor.
    pub fn enqueue(&mut self, content: impl Into<String>) {
        self.queue.push_back(Percept::new(&self.name, content));
    }

    /// Moves the read window to latest and returns percepts from this iteration.
    pub fn sense_unread(&mut self) -> Vec<Percept> {
        let unread_count = self.queue.len().saturating_sub(self.unread_start);
        if unread_count == 0 {
            return Vec::new();
        }

        let start = self.unread_start;
        let percepts = self
            .queue
            .iter()
            .skip(start)
            .cloned()
            .collect::<Vec<Percept>>();
        self.unread_start = self.queue.len();
        percepts
    }
}

/// Internal action types supported in this first pass.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    /// Respond in chat.
    ChatResponse { message: String },
    /// Search files by content.
    Grep { pattern: String, path: String },
    /// Search for files by pattern.
    Glob { pattern: String, path: String },
    /// Run a shell command.
    Shell { command: String },
    /// Query web search.
    WebSearch { query: String },
}

impl Action {
    fn keyword(&self) -> &'static str {
        match self {
            Action::ChatResponse { .. } => "chat",
            Action::Grep { .. } => "grep",
            Action::Glob { .. } => "glob",
            Action::Shell { .. } => "shell",
            Action::WebSearch { .. } => "web_search",
        }
    }

    fn internal_kind(&self) -> InternalActuatorKind {
        match self {
            Action::ChatResponse { .. } => InternalActuatorKind::Chat,
            Action::Grep { .. } => InternalActuatorKind::Grep,
            Action::Glob { .. } => InternalActuatorKind::Glob,
            Action::Shell { .. } => InternalActuatorKind::Shell,
            Action::WebSearch { .. } => InternalActuatorKind::WebSearch,
        }
    }
}

/// Policy period for a rate-limit window.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitPeriod {
    /// Per minute rate limit.
    Minute,
    /// Per hour rate limit.
    Hour,
    /// Per day rate limit.
    Day,
    /// Per week rate limit.
    Week,
    /// Per month rate limit.
    Month,
}

/// Per-actuator rate-limit policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RateLimit {
    /// Maximum executions allowed in the configured period.
    pub max: u32,
    /// Period bucket used for this limit.
    pub per: RateLimitPeriod,
}

/// Safety policy for an actuator.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SafetyPolicy {
    /// Optional allowlist of action keywords.
    pub allowlist: Option<Vec<String>>,
    /// Optional denylist of action keywords.
    pub denylist: Option<Vec<String>>,
    /// Optional rate limit.
    pub rate_limit: Option<RateLimit>,
    /// If true, requires a human before execution.
    pub require_hitl: bool,
    /// If true, run in a locked-down environment.
    pub sandboxed: bool,
}

impl SafetyPolicy {
    /// Validates policy invariants.
    pub fn validate(&self) -> Result<()> {
        if self.allowlist.is_some() && self.denylist.is_some() {
            return Err(anyhow!("allowlist and denylist cannot both be set"));
        }

        if let Some(limit) = &self.rate_limit
            && limit.max == 0
        {
            return Err(anyhow!("rate_limit.max must be greater than 0"));
        }

        Ok(())
    }
}

/// Internal actuator kind.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InternalActuatorKind {
    /// Chat action responder.
    Chat,
    /// File content searcher.
    Grep,
    /// File path searcher.
    Glob,
    /// Shell command executor.
    Shell,
    /// Internet search executor.
    WebSearch,
}

/// Actuator type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActuatorType {
    /// Built-in actuator.
    Internal(InternalActuatorKind),
    /// External MCP actuator.
    Mcp(McpDetails),
    /// Agentic workflow actuator.
    Workflow(WorkflowDetails),
}

/// Executor for performing actions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Actuator {
    /// Actuator name.
    pub name: String,
    /// Description of actions supported by this actuator.
    pub description: String,
    /// Actuator type.
    pub kind: ActuatorType,
    /// Safety policy.
    pub policy: SafetyPolicy,
}

impl Actuator {
    /// Creates an internal actuator.
    pub fn internal(
        name: impl Into<String>,
        description: impl Into<String>,
        kind: InternalActuatorKind,
        policy: SafetyPolicy,
    ) -> Result<Self> {
        policy.validate()?;
        Ok(Self {
            name: name.into(),
            description: description.into(),
            kind: ActuatorType::Internal(kind),
            policy,
        })
    }

    /// Creates an MCP actuator.
    pub fn mcp(
        name: impl Into<String>,
        description: impl Into<String>,
        details: McpDetails,
        policy: SafetyPolicy,
    ) -> Result<Self> {
        policy.validate()?;
        Ok(Self {
            name: name.into(),
            description: description.into(),
            kind: ActuatorType::Mcp(details),
            policy,
        })
    }

    /// Creates an agentic workflow actuator.
    pub fn workflow(
        name: impl Into<String>,
        description: impl Into<String>,
        details: WorkflowDetails,
        policy: SafetyPolicy,
    ) -> Result<Self> {
        policy.validate()?;
        Ok(Self {
            name: name.into(),
            description: description.into(),
            kind: ActuatorType::Workflow(details),
            policy,
        })
    }
}

/// Planned action recommendation from reasoning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecommendedAction {
    /// Target actuator name.
    pub actuator_name: String,
    /// Action to execute.
    pub action: Action,
}

/// Execution result of a recommended action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionResult {
    /// Action was executed and produced output.
    Executed { output: String },
    /// Action was denied by policy.
    Denied(String),
    /// Action requires human-in-the-loop approval.
    RequiresHitl,
}

/// A pluggable actuator executor.
pub trait ActuatorExecutor: Send + Sync {
    /// Executes a planned action and returns its output.
    fn execute(&self, action: &Action) -> Result<String>;
}

/// Chat actuator executor.
#[derive(Default)]
pub struct ChatActuatorExecutor;

impl ActuatorExecutor for ChatActuatorExecutor {
    fn execute(&self, action: &Action) -> Result<String> {
        if let Action::ChatResponse { message } = action {
            return Ok(message.clone());
        }

        Err(anyhow!("chat executor received incompatible action"))
    }
}

/// Glob actuator executor.
pub struct GlobActuatorExecutor {
    workspace_root: PathBuf,
}

impl GlobActuatorExecutor {
    /// Creates a glob executor rooted at a workspace path.
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

impl ActuatorExecutor for GlobActuatorExecutor {
    fn execute(&self, action: &Action) -> Result<String> {
        if let Action::Glob { pattern, path } = action {
            let base = normalize_rooted_path(&self.workspace_root, path);
            let full_pattern = base.join(pattern).to_string_lossy().to_string();
            let mut matches = Vec::new();

            for path in glob(&full_pattern)?.flatten() {
                matches.push(path.to_string_lossy().to_string());
            }

            matches.sort();
            if matches.is_empty() {
                return Ok("no files matched".to_string());
            }
            return Ok(matches.join("\n"));
        }

        Err(anyhow!("glob executor received incompatible action"))
    }
}

/// Grep actuator executor.
pub struct GrepActuatorExecutor {
    workspace_root: PathBuf,
}

impl GrepActuatorExecutor {
    /// Creates a grep executor rooted at a workspace path.
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

impl ActuatorExecutor for GrepActuatorExecutor {
    fn execute(&self, action: &Action) -> Result<String> {
        if let Action::Grep { pattern, path } = action {
            let root = normalize_rooted_path(&self.workspace_root, path);
            let regex = Regex::new(pattern)?;
            let mut hits = Vec::new();

            for entry in WalkDir::new(&root).into_iter().flatten() {
                if !entry.file_type().is_file() {
                    continue;
                }

                let file_path = entry.path();
                let Ok(content) = fs::read_to_string(file_path) else {
                    continue;
                };

                for (idx, line) in content.lines().enumerate() {
                    if regex.is_match(line) {
                        hits.push(format!(
                            "{}:{}:{}",
                            file_path.to_string_lossy(),
                            idx + 1,
                            line
                        ));
                    }
                }
            }

            if hits.is_empty() {
                return Ok("no matches found".to_string());
            }
            return Ok(hits.join("\n"));
        }

        Err(anyhow!("grep executor received incompatible action"))
    }
}

/// Shell actuator executor.
pub struct ShellActuatorExecutor {
    workspace_root: PathBuf,
}

impl ShellActuatorExecutor {
    /// Creates a shell executor rooted at a workspace path.
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }
}

impl ActuatorExecutor for ShellActuatorExecutor {
    fn execute(&self, action: &Action) -> Result<String> {
        if let Action::Shell { command } = action {
            let output = if cfg!(target_os = "windows") {
                Command::new("cmd")
                    .arg("/C")
                    .arg(command)
                    .current_dir(&self.workspace_root)
                    .output()?
            } else {
                Command::new("sh")
                    .arg("-c")
                    .arg(command)
                    .current_dir(&self.workspace_root)
                    .output()?
            };

            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let mut parts = vec![format!("status: {}", output.status)];
            if !stdout.is_empty() {
                parts.push(format!("stdout:\n{}", stdout));
            }
            if !stderr.is_empty() {
                parts.push(format!("stderr:\n{}", stderr));
            }

            return Ok(parts.join("\n"));
        }

        Err(anyhow!("shell executor received incompatible action"))
    }
}

/// Web search actuator executor.
#[derive(Default)]
pub struct WebSearchActuatorExecutor;

impl ActuatorExecutor for WebSearchActuatorExecutor {
    fn execute(&self, action: &Action) -> Result<String> {
        if let Action::WebSearch { query } = action {
            return Ok(format!(
                "web search request accepted for query: '{query}' (provider integration pending)"
            ));
        }

        Err(anyhow!("web_search executor received incompatible action"))
    }
}

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
}

/// Output of a completed loop iteration.
#[derive(Clone, Debug)]
pub struct IterationReport {
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
    observability: Observability,
    previous_percept_windows: Vec<Vec<String>>,
    executions_per_actuator: HashMap<String, u32>,
}

impl LooperRuntime {
    /// Creates an empty runtime.
    pub fn new() -> Self {
        Self {
            sensors: HashMap::new(),
            actuators: HashMap::new(),
            internal_executors: HashMap::new(),
            observability: Observability::default(),
            previous_percept_windows: Vec::new(),
            executions_per_actuator: HashMap::new(),
        }
    }

    /// Creates a runtime with default internal sensors, actuators, and executors.
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

        Ok(runtime)
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

    /// Returns a snapshot of current observability metrics.
    pub fn observability(&self) -> &Observability {
        &self.observability
    }

    /// Executes one full sensory loop iteration.
    pub fn run_iteration(&mut self) -> Result<IterationReport> {
        self.observability.total_iterations += 1;

        self.observability.bump_phase(LoopPhase::SurpriseDetection);
        let sensed = self.collect_new_percepts();
        self.observability.local_model_tokens += estimate_tokens(&sensed);

        let surprising = self.detect_surprises(&sensed);
        if surprising.is_empty() {
            self.persist_window(&sensed);
            return Ok(IterationReport {
                sensed_percepts: sensed,
                surprising_percepts: Vec::new(),
                planned_actions: Vec::new(),
                action_results: Vec::new(),
                ended_after_surprise_detection: true,
                ended_after_reasoning: false,
            });
        }

        self.observability.bump_phase(LoopPhase::Reasoning);
        self.observability.frontier_model_tokens += estimate_tokens(&surprising);
        let planned_actions = self.plan_actions(&surprising);
        if planned_actions.is_empty() {
            self.observability.false_positive_surprises += 1;
            self.persist_window(&sensed);
            return Ok(IterationReport {
                sensed_percepts: sensed,
                surprising_percepts: surprising,
                planned_actions,
                action_results: Vec::new(),
                ended_after_surprise_detection: false,
                ended_after_reasoning: true,
            });
        }

        self.observability.bump_phase(LoopPhase::PerformActions);
        let mut action_results = Vec::with_capacity(planned_actions.len());
        for recommendation in &planned_actions {
            let result = self.execute_recommendation(recommendation)?;
            if matches!(result, ExecutionResult::Denied(_)) {
                self.observability.failed_tool_executions += 1;
            }
            action_results.push(result);
        }

        self.persist_window(&sensed);
        Ok(IterationReport {
            sensed_percepts: sensed,
            surprising_percepts: surprising,
            planned_actions,
            action_results,
            ended_after_surprise_detection: false,
            ended_after_reasoning: false,
        })
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

    fn detect_surprises(&self, percepts: &[Percept]) -> Vec<Percept> {
        if percepts.is_empty() {
            return Vec::new();
        }

        let mut surprising = Vec::new();
        for percept in percepts {
            let lower = percept.content.to_lowercase();
            let has_surprise_signal = lower.contains("!")
                || lower.contains("error")
                || lower.contains("fail")
                || lower.contains("urgent")
                || lower.contains("new")
                || lower.contains("search")
                || lower.contains("run")
                || lower.contains("glob")
                || lower.contains("grep");

            if !has_surprise_signal {
                continue;
            }

            let seen_recently = self
                .previous_percept_windows
                .iter()
                .rev()
                .take(10)
                .flatten()
                .any(|seen| seen == &percept.content);

            if !seen_recently {
                surprising.push(percept.clone());
            }
        }

        surprising
    }

    fn plan_actions(&self, surprising: &[Percept]) -> Vec<RecommendedAction> {
        let mut actions = Vec::new();
        for percept in surprising {
            let lower = percept.content.to_lowercase();

            if lower.contains("search") {
                actions.push(RecommendedAction {
                    actuator_name: "web_search".to_string(),
                    action: Action::WebSearch {
                        query: percept.content.clone(),
                    },
                });
                continue;
            }

            if lower.contains("glob") || lower.contains("find file") {
                actions.push(RecommendedAction {
                    actuator_name: "glob".to_string(),
                    action: Action::Glob {
                        pattern: "**/*".to_string(),
                        path: ".".to_string(),
                    },
                });
                continue;
            }

            if lower.contains("grep") || lower.contains("find text") {
                let pattern = if lower.contains("grep ") {
                    percept
                        .content
                        .split_once("grep ")
                        .map(|(_, suffix)| suffix)
                        .unwrap_or(".")
                } else {
                    "."
                };

                actions.push(RecommendedAction {
                    actuator_name: "grep".to_string(),
                    action: Action::Grep {
                        pattern: pattern.to_string(),
                        path: ".".to_string(),
                    },
                });
                continue;
            }

            if lower.contains("run") || lower.contains("shell") {
                actions.push(RecommendedAction {
                    actuator_name: "shell".to_string(),
                    action: Action::Shell {
                        command: extract_shell_command(&percept.content),
                    },
                });
                continue;
            }

            actions.push(RecommendedAction {
                actuator_name: "chat".to_string(),
                action: Action::ChatResponse {
                    message: "I noticed a surprising percept and queued it for review.".to_string(),
                },
            });
        }
        actions
    }

    fn execute_recommendation(
        &mut self,
        recommendation: &RecommendedAction,
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

        if actuator.policy.require_hitl {
            return Ok(ExecutionResult::RequiresHitl);
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

    fn persist_window(&mut self, percepts: &[Percept]) {
        let window = percepts
            .iter()
            .map(|p| p.content.clone())
            .collect::<Vec<_>>();
        self.previous_percept_windows.push(window);
        if self.previous_percept_windows.len() > 10 {
            self.previous_percept_windows.remove(0);
        }
    }
}

impl Default for LooperRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// API request body for creating a sensor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SensorCreateRequest {
    /// Sensor name.
    pub name: String,
    /// Description of the percept stream.
    pub description: String,
}

impl SensorCreateRequest {
    /// Converts the request into a runtime sensor.
    pub fn into_sensor(self) -> Sensor {
        Sensor::new(self.name, self.description)
    }
}

/// Top-level API request body for creating an actuator.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActuatorCreateRequest {
    /// Actuator name.
    pub name: String,
    /// Description of actuator behavior.
    pub description: String,
    /// Actuator type.
    #[serde(rename = "type")]
    pub actuator_type: ActuatorRegistrationType,
    /// Type-specific details.
    pub details: serde_json::Value,
    /// Optional safety policy.
    #[serde(default)]
    pub policy: SafetyPolicy,
}

impl ActuatorCreateRequest {
    /// Converts the request into a runtime actuator.
    pub fn try_into_actuator(self) -> Result<Actuator> {
        self.policy.validate()?;
        match self.actuator_type {
            ActuatorRegistrationType::Mcp => {
                let details: McpDetails = serde_json::from_value(self.details)?;
                details.validate()?;
                Actuator::mcp(self.name, self.description, details, self.policy)
            }
            ActuatorRegistrationType::Workflow => {
                let details: WorkflowDetails = serde_json::from_value(self.details)?;
                details.validate()?;
                Actuator::workflow(self.name, self.description, details, self.policy)
            }
        }
    }
}

/// Supported external actuator types accepted by REST API.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActuatorRegistrationType {
    /// MCP actuator registration.
    Mcp,
    /// Agentic workflow registration.
    Workflow,
}

/// MCP server connection type.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpConnectionType {
    /// MCP over HTTP endpoint.
    Http,
    /// MCP over stdio executable.
    Stdio,
}

/// MCP details payload for actuator creation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct McpDetails {
    /// Human-readable MCP server name.
    pub name: String,
    /// MCP connection type.
    #[serde(rename = "type")]
    pub connection: McpConnectionType,
    /// URL or executable path.
    pub url: String,
}

impl McpDetails {
    /// Validates MCP details.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(anyhow!("mcp details.name cannot be empty"));
        }

        if self.url.trim().is_empty() {
            return Err(anyhow!("mcp details.url cannot be empty"));
        }

        Ok(())
    }
}

/// Agentic workflow details payload for actuator creation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkflowDetails {
    /// Workflow name.
    pub name: String,
    /// Ordered list of cells.
    pub cells: Vec<String>,
}

impl WorkflowDetails {
    /// Validates workflow details.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(anyhow!("workflow details.name cannot be empty"));
        }

        if self.cells.is_empty() {
            return Err(anyhow!("workflow details.cells cannot be empty"));
        }

        Ok(())
    }
}

fn normalize_rooted_path(root: &Path, requested: &str) -> PathBuf {
    let requested_path = Path::new(requested);
    if requested_path.is_absolute() {
        requested_path.to_path_buf()
    } else {
        root.join(requested_path)
    }
}

fn extract_shell_command(message: &str) -> String {
    let lower = message.to_lowercase();
    if let Some((_, right)) = lower.split_once("run ") {
        let offset = message.len().saturating_sub(right.len());
        return message[offset..].trim().to_string();
    }

    if let Some((_, right)) = lower.split_once("shell ") {
        let offset = message.len().saturating_sub(right.len());
        return message[offset..].trim().to_string();
    }

    message.trim().to_string()
}

fn estimate_tokens(percepts: &[Percept]) -> u64 {
    percepts
        .iter()
        .map(|p| (p.content.split_whitespace().count() as u64) + 4)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_policy_rejects_allow_and_deny_together() {
        let policy = SafetyPolicy {
            allowlist: Some(vec!["shell".to_string()]),
            denylist: Some(vec!["grep".to_string()]),
            ..SafetyPolicy::default()
        };
        assert!(policy.validate().is_err());
    }

    #[test]
    fn non_surprising_percept_ends_early() {
        let mut runtime = LooperRuntime::with_internal_defaults().expect("defaults should build");
        runtime
            .enqueue_chat_message("routine status update")
            .expect("chat sensor should exist");

        let report = runtime.run_iteration().expect("iteration should complete");
        assert!(report.ended_after_surprise_detection);
        assert!(report.planned_actions.is_empty());
    }

    #[test]
    fn surprising_percept_executes_real_web_search_executor() {
        let mut runtime = LooperRuntime::with_internal_defaults().expect("defaults should build");
        runtime
            .enqueue_chat_message("please search docs for model guidance")
            .expect("chat sensor should exist");

        let report = runtime.run_iteration().expect("iteration should complete");
        assert!(!report.ended_after_surprise_detection);
        assert_eq!(report.planned_actions.len(), 1);
        assert!(matches!(
            report.action_results[0],
            ExecutionResult::Executed { .. }
        ));
    }

    #[test]
    fn denied_action_counts_as_failed_execution() {
        let mut runtime = LooperRuntime::with_internal_defaults().expect("defaults should build");
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

        let report = runtime.run_iteration().expect("iteration should complete");
        assert!(matches!(
            report.action_results.first(),
            Some(ExecutionResult::Denied(_))
        ));
        assert_eq!(runtime.observability().failed_tool_executions, 1);
    }

    #[test]
    fn actuator_rest_payload_parses() {
        let json = r#"
        {
            "name": "bundle deploy",
            "description": "Deploys our databricks bundle",
            "type": "workflow",
            "details": {
                "name": "Bundle Validation & Deployment",
                "cells": ["step one", "%shell cargo test"]
            },
            "policy": {
                "allowlist": ["shell"],
                "require_hitl": true,
                "sandboxed": true
            }
        }
        "#;

        let request: ActuatorCreateRequest = serde_json::from_str(json).expect("json should parse");
        let actuator = request
            .try_into_actuator()
            .expect("request should convert to actuator");

        assert_eq!(actuator.name, "bundle deploy");
        assert!(matches!(actuator.kind, ActuatorType::Workflow(_)));
    }
}
