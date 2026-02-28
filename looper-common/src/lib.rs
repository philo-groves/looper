use serde::{Deserialize, Serialize};

pub const DISCOVERY_HOST: &str = "127.0.0.1";
pub const DISCOVERY_PORT: u16 = 10001;
pub const DEFAULT_DISCOVERY_URL: &str = "ws://127.0.0.1:10001";

pub const AGENT_HOST: &str = "127.0.0.1";
pub const AGENT_PORT_START: u16 = 11000;
pub const AGENT_PORT_END: u16 = 12000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    Setup,
    Running,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub agent_id: String,
    pub agent_name: Option<String>,
    pub assigned_port: u16,
    pub mode: AgentMode,
    pub workspace_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentEntry {
    pub workspace_dir: String,
    pub assigned_port: u16,
    pub agent_name: Option<String>,
    pub is_running: bool,
    pub mode: Option<AgentMode>,
    pub agent_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiscoveryRequest {
    Register {
        agent_name: Option<String>,
        requested_port: Option<u16>,
        workspace_dir: Option<String>,
        mode: AgentMode,
    },
    ListAgents,
    UpsertAgentLaunch {
        workspace_dir: String,
        port: u16,
        agent_name: Option<String>,
    },
    StartAgent {
        workspace_dir: String,
    },
    CreateAgent,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiscoveryResponse {
    Registered {
        agent_id: String,
        assigned_port: u16,
        active_agents: Vec<AgentInfo>,
    },
    Agents {
        agents: Vec<AgentEntry>,
    },
    AgentLaunchUpserted,
    AgentStarted {
        workspace_dir: String,
        assigned_port: u16,
    },
    AgentCreated {
        assigned_port: u16,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentSocketMessage {
    AgentHello {
        agent_id: String,
        mode: AgentMode,
    },
    SetupSubmit {
        workspace_dir: String,
        port: u16,
        provider: String,
        api_keys: Vec<ProviderApiKey>,
    },
    SetupAccepted {
        mode: AgentMode,
    },
    Error {
        message: String,
    },
    UserText {
        text: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderApiKey {
    pub provider: String,
    pub api_key: String,
}
