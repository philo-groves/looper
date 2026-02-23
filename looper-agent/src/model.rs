use std::collections::VecDeque;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

/// Accepted payload format for REST-ingested percepts.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorRestFormat {
    /// Plain text or markdown payload.
    Text,
    /// JSON payload serialized to text.
    Json,
}

/// How a sensor receives percepts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SensorIngressConfig {
    /// Sensor receives percepts from internal runtime hooks.
    Internal,
    /// Sensor receives percepts by watching files in a directory.
    Directory { path: String },
    /// Sensor receives percepts via the HTTP API.
    RestApi { format: SensorRestFormat },
}

/// Supported model providers for Looper configuration.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelProviderKind {
    /// Local Ollama provider.
    Ollama,
    /// OpenAI provider.
    OpenAi,
    /// OpenCode Zen provider.
    OpenCodeZen,
}

/// Current lifecycle state of the Looper agent.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Agent is not yet configured for first run.
    Setup,
    /// Agent is actively looping.
    Running,
    /// Agent is stopped and cannot operate until restarted.
    Stopped,
}

/// Model selection for a loop phase.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelSelection {
    /// Provider backing the selected model.
    pub provider: ModelProviderKind,
    /// Provider-specific model identifier.
    pub model: String,
}

/// A single unit of perception received from a sensor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Percept {
    /// Name of the sensor that emitted this percept.
    pub sensor_name: String,
    /// Human-readable percept payload.
    pub content: String,
    /// Optional chat session id when the percept came from chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
}

impl Percept {
    /// Creates a new percept.
    pub fn new(sensor_name: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            sensor_name: sensor_name.into(),
            content: content.into(),
            chat_id: None,
        }
    }

    /// Creates a chat percept with a chat id.
    pub fn chat(content: impl Into<String>, chat_id: impl Into<String>) -> Self {
        Self {
            sensor_name: "chat".to_string(),
            content: content.into(),
            chat_id: Some(chat_id.into()),
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
    /// Sensitivity score for surprise detection, from 0 to 100.
    pub sensitivity_score: u8,
    /// Singular display name for percept items.
    pub percept_singular_name: String,
    /// Plural display name for percept items.
    pub percept_plural_name: String,
    /// Ingestion configuration for this sensor.
    pub ingress: SensorIngressConfig,
    queue: VecDeque<Percept>,
    unread_start: usize,
}

impl Sensor {
    /// Creates a new enabled sensor.
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self::with_sensitivity_score(name, description, 50)
    }

    /// Creates a new enabled sensor with a specific sensitivity score.
    pub fn with_sensitivity_score(
        name: impl Into<String>,
        description: impl Into<String>,
        sensitivity_score: u8,
    ) -> Self {
        let name = name.into();
        let singular = name.trim().to_lowercase();
        let plural = if singular.ends_with('s') {
            singular.clone()
        } else {
            format!("{singular}s")
        };

        Self {
            name,
            description: description.into(),
            enabled: true,
            sensitivity_score: sensitivity_score.min(100),
            percept_singular_name: singular,
            percept_plural_name: plural,
            ingress: SensorIngressConfig::RestApi {
                format: SensorRestFormat::Text,
            },
            queue: VecDeque::new(),
            unread_start: 0,
        }
    }

    /// Enqueues a percept into the sensor.
    pub fn enqueue(&mut self, content: impl Into<String>) {
        self.queue.push_back(Percept::new(&self.name, content));
    }

    /// Enqueues a percept with an explicit chat id.
    pub fn enqueue_with_chat_id(&mut self, content: impl Into<String>, chat_id: impl Into<String>) {
        let mut percept = Percept::new(&self.name, content);
        percept.chat_id = Some(chat_id.into());
        self.queue.push_back(percept);
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

    /// Returns total queued percepts retained by this sensor.
    pub fn queued_count(&self) -> usize {
        self.queue.len()
    }

    /// Returns queued percepts that have not yet been consumed.
    pub fn unread_count(&self) -> usize {
        self.queue.len().saturating_sub(self.unread_start)
    }
}

/// Internal action types supported in this first pass.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
    /// Returns policy keyword used for allowlist and denylist checks.
    pub fn keyword(&self) -> &'static str {
        match self {
            Action::ChatResponse { .. } => "chat",
            Action::Grep { .. } => "grep",
            Action::Glob { .. } => "glob",
            Action::Shell { .. } => "shell",
            Action::WebSearch { .. } => "web_search",
        }
    }

    /// Returns the internal actuator kind needed to execute this action.
    pub fn internal_kind(&self) -> InternalActuatorKind {
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
    /// Singular display name for action items.
    pub action_singular_name: String,
    /// Plural display name for action items.
    pub action_plural_name: String,
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
        let name = name.into();
        let singular = name.trim().to_lowercase();
        let plural = if singular.ends_with('s') {
            singular.clone()
        } else {
            format!("{singular}s")
        };

        Ok(Self {
            name,
            description: description.into(),
            kind: ActuatorType::Internal(kind),
            policy,
            action_singular_name: singular,
            action_plural_name: plural,
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
        details.validate()?;
        let name = name.into();
        let singular = name.trim().to_lowercase();
        let plural = if singular.ends_with('s') {
            singular.clone()
        } else {
            format!("{singular}s")
        };

        Ok(Self {
            name,
            description: description.into(),
            kind: ActuatorType::Mcp(details),
            policy,
            action_singular_name: singular,
            action_plural_name: plural,
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
        details.validate()?;
        let name = name.into();
        let singular = name.trim().to_lowercase();
        let plural = if singular.ends_with('s') {
            singular.clone()
        } else {
            format!("{singular}s")
        };

        Ok(Self {
            name,
            description: description.into(),
            kind: ActuatorType::Workflow(details),
            policy,
            action_singular_name: singular,
            action_plural_name: plural,
        })
    }
}

/// Planned action recommendation from reasoning.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecommendedAction {
    /// Target actuator name.
    pub actuator_name: String,
    /// Action to execute.
    pub action: Action,
}

/// Execution result of a recommended action.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ExecutionResult {
    /// Action was executed and produced output.
    Executed { output: String },
    /// Action was denied by policy.
    Denied(String),
    /// Action requires human-in-the-loop approval.
    RequiresHitl { approval_id: u64 },
}

/// Pending human-in-the-loop approval.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PendingApproval {
    /// Unique approval id.
    pub id: u64,
    /// Action that requires approval.
    pub recommendation: RecommendedAction,
}
