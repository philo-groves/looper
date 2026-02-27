use serde::{Deserialize, Serialize};

pub const DISCOVERY_HOST: &str = "127.0.0.1";
pub const DISCOVERY_PORT: u16 = 10001;
pub const DEFAULT_DISCOVERY_URL: &str = "ws://127.0.0.1:10001";

pub const AGENT_HOST: &str = "127.0.0.1";
pub const AGENT_PORT_START: u16 = 11000;
pub const AGENT_PORT_END: u16 = 12000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub agent_id: String,
    pub agent_name: Option<String>,
    pub assigned_port: u16,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiscoveryRequest {
    Register { agent_name: Option<String> },
    ListAgents,
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
        active_agents: Vec<AgentInfo>,
    },
    Error {
        message: String,
    },
}
