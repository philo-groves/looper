use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, anyhow};
use futures_util::{SinkExt, StreamExt};
use looper_common::{
    AGENT_PORT_END, AGENT_PORT_START, AgentInfo, DISCOVERY_HOST, DISCOVERY_PORT, DiscoveryRequest,
    DiscoveryResponse,
};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::{accept_async, tungstenite::Message};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentLaunchConfig {
    workspace_dir: String,
    port: u16,
    #[serde(default)]
    agent_name: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AgentsFile {
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    #[serde(default)]
    agents: Vec<AgentLaunchConfig>,
}

fn default_schema_version() -> u32 {
    1
}

struct DiscoveryState {
    agents: HashMap<String, AgentInfo>,
    used_ports: HashSet<u16>,
    configured_ports: HashSet<u16>,
    launch_configs: Vec<AgentLaunchConfig>,
}

impl DiscoveryState {
    fn from_launch_configs(launch_configs: Vec<AgentLaunchConfig>) -> Self {
        let configured_ports = launch_configs.iter().map(|cfg| cfg.port).collect();
        Self {
            agents: HashMap::new(),
            used_ports: HashSet::new(),
            configured_ports,
            launch_configs,
        }
    }

    fn assign_port(&mut self) -> Option<u16> {
        for port in AGENT_PORT_START..=AGENT_PORT_END {
            if !self.used_ports.contains(&port) && !self.configured_ports.contains(&port) {
                self.used_ports.insert(port);
                return Some(port);
            }
        }

        None
    }

    fn claim_port(&mut self, port: u16) -> Option<u16> {
        if self.used_ports.contains(&port) {
            return None;
        }
        self.used_ports.insert(port);
        Some(port)
    }

    fn release_port(&mut self, port: u16) {
        self.used_ports.remove(&port);
    }

    fn active_agents(&self) -> Vec<AgentInfo> {
        self.agents.values().cloned().collect()
    }

    fn upsert_launch_config(&mut self, cfg: AgentLaunchConfig) -> Result<(), String> {
        if !(AGENT_PORT_START..=AGENT_PORT_END).contains(&cfg.port) {
            return Err(format!(
                "port {} is out of range ({}-{})",
                cfg.port, AGENT_PORT_START, AGENT_PORT_END
            ));
        }

        for existing in &self.launch_configs {
            if existing.workspace_dir != cfg.workspace_dir && existing.port == cfg.port {
                return Err(format!(
                    "port {} is already configured for another workspace",
                    cfg.port
                ));
            }
        }

        if let Some(existing) = self
            .launch_configs
            .iter_mut()
            .find(|entry| entry.workspace_dir == cfg.workspace_dir)
        {
            existing.port = cfg.port;
            existing.agent_name = cfg.agent_name;
        } else {
            self.launch_configs.push(cfg);
        }

        self.configured_ports = self.launch_configs.iter().map(|entry| entry.port).collect();
        Ok(())
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config_path = agents_file_path()?;
    let launch_configs = load_launch_configs(&config_path)?;

    let bind_addr = format!("{DISCOVERY_HOST}:{DISCOVERY_PORT}");
    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind discovery server to {bind_addr}"))?;

    println!("discovery listening on ws://{bind_addr}");

    let state = Arc::new(Mutex::new(DiscoveryState::from_launch_configs(launch_configs)));
    {
        let state_guard = state.lock().await;
        spawn_configured_agents(&state_guard.launch_configs)?;
    }

    loop {
        let (stream, addr) = listener
            .accept()
            .await
            .context("failed to accept tcp connection")?;

        let state = Arc::clone(&state);
        let config_path = config_path.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, state, config_path).await {
                eprintln!("connection {addr} failed: {error:#}");
            }
        });
    }
}

async fn handle_connection(
    stream: TcpStream,
    state: Arc<Mutex<DiscoveryState>>,
    config_path: PathBuf,
) -> anyhow::Result<()> {
    let ws_stream = accept_async(stream)
        .await
        .context("websocket handshake failed")?;
    let (mut writer, mut reader) = ws_stream.split();

    let initial_message = match reader.next().await {
        Some(Ok(Message::Text(message))) => message,
        Some(Ok(_)) => {
            writer
                .send(Message::Text(
                    serde_json::to_string(&DiscoveryResponse::Error {
                        message: "expected text register message".to_string(),
                    })?
                    .into(),
                ))
                .await
                .ok();
            return Ok(());
        }
        Some(Err(error)) => return Err(error.into()),
        None => return Ok(()),
    };

    let request: DiscoveryRequest = match serde_json::from_str(&initial_message) {
        Ok(request) => request,
        Err(error) => {
            writer
                .send(Message::Text(
                    serde_json::to_string(&DiscoveryResponse::Error {
                        message: format!("invalid request json: {error}"),
                    })?
                    .into(),
                ))
                .await
                .ok();
            return Ok(());
        }
    };

    let registered_agent = match request {
        DiscoveryRequest::Register {
            agent_name,
            requested_port,
            workspace_dir: _,
            mode,
        } => {
            let mut state_guard = state.lock().await;
            let active_agents = state_guard.active_agents();

            let assigned_port = if let Some(requested_port) = requested_port {
                if !(AGENT_PORT_START..=AGENT_PORT_END).contains(&requested_port) {
                    writer
                        .send(Message::Text(
                            serde_json::to_string(&DiscoveryResponse::Error {
                                message: format!(
                                    "requested port {requested_port} is out of range ({}-{})",
                                    AGENT_PORT_START, AGENT_PORT_END
                                ),
                            })?
                            .into(),
                        ))
                        .await
                        .ok();
                    return Ok(());
                }

                match state_guard.claim_port(requested_port) {
                    Some(port) => port,
                    None => {
                        writer
                            .send(Message::Text(
                                serde_json::to_string(&DiscoveryResponse::Error {
                                    message: format!(
                                        "requested port {requested_port} is already in use"
                                    ),
                                })?
                                .into(),
                            ))
                            .await
                            .ok();
                        return Ok(());
                    }
                }
            } else {
                let Some(assigned_port) = state_guard.assign_port() else {
                    writer
                        .send(Message::Text(
                            serde_json::to_string(&DiscoveryResponse::Error {
                                message: "no available agent ports".to_string(),
                            })?
                            .into(),
                        ))
                        .await
                        .ok();
                    return Ok(());
                };
                assigned_port
            };

            let agent_info = AgentInfo {
                agent_id: Uuid::new_v4().to_string(),
                agent_name,
                assigned_port,
                mode,
            };

            state_guard
                .agents
                .insert(agent_info.agent_id.clone(), agent_info.clone());

            writer
                .send(Message::Text(
                    serde_json::to_string(&DiscoveryResponse::Registered {
                        agent_id: agent_info.agent_id.clone(),
                        assigned_port,
                        active_agents,
                    })?
                    .into(),
                ))
                .await
                .context("failed to send register response")?;

            println!(
                "registered agent {} on port {}",
                agent_info.agent_id, agent_info.assigned_port
            );
            Some(agent_info)
        }
        DiscoveryRequest::ListAgents => {
            let state_guard = state.lock().await;
            let active_agents = state_guard.active_agents();

            writer
                .send(Message::Text(
                    serde_json::to_string(&DiscoveryResponse::Agents { active_agents })?.into(),
                ))
                .await
                .context("failed to send agents list response")?;

            return Ok(());
        }
        DiscoveryRequest::UpsertAgentLaunch {
            workspace_dir,
            port,
            agent_name,
        } => {
            let mut state_guard = state.lock().await;
            let upsert = state_guard.upsert_launch_config(AgentLaunchConfig {
                workspace_dir,
                port,
                agent_name,
            });

            match upsert {
                Ok(()) => {
                    persist_launch_configs(&config_path, &state_guard.launch_configs)?;
                    writer
                        .send(Message::Text(
                            serde_json::to_string(&DiscoveryResponse::AgentLaunchUpserted)?.into(),
                        ))
                        .await
                        .context("failed to send launch config upsert response")?;
                }
                Err(message) => {
                    writer
                        .send(Message::Text(
                            serde_json::to_string(&DiscoveryResponse::Error { message })?.into(),
                        ))
                        .await
                        .context("failed to send launch config error response")?;
                }
            }

            return Ok(());
        }
    };

    while let Some(message) = reader.next().await {
        match message {
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => {
                eprintln!("agent websocket read failed: {error}");
                break;
            }
        }
    }

    if let Some(agent) = registered_agent {
        let mut state_guard = state.lock().await;
        state_guard.agents.remove(&agent.agent_id);
        state_guard.release_port(agent.assigned_port);
        println!("agent {} disconnected", agent.agent_id);
    }

    Ok(())
}

fn agents_file_path() -> anyhow::Result<PathBuf> {
    let home = env::var("USERPROFILE")
        .or_else(|_| env::var("HOME"))
        .map(PathBuf::from)
        .context("failed to resolve USERPROFILE/HOME for .looper config")?;
    Ok(home.join(".looper").join("agents.json"))
}

fn load_launch_configs(path: &PathBuf) -> anyhow::Result<Vec<AgentLaunchConfig>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read launch config file {}", path.display()))?;
    let parsed: AgentsFile = serde_json::from_str(&text)
        .with_context(|| format!("invalid launch config json in {}", path.display()))?;

    let mut configs = Vec::new();
    let mut used = HashSet::new();

    for entry in parsed.agents {
        if entry.workspace_dir.trim().is_empty() {
            eprintln!("ignoring launch config with empty workspace path");
            continue;
        }
        if !(AGENT_PORT_START..=AGENT_PORT_END).contains(&entry.port) {
            eprintln!(
                "ignoring launch config for {} with out-of-range port {}",
                entry.workspace_dir, entry.port
            );
            continue;
        }
        if !used.insert(entry.port) {
            eprintln!(
                "ignoring duplicate launch config port {} for {}",
                entry.port, entry.workspace_dir
            );
            continue;
        }
        configs.push(entry);
    }

    Ok(configs)
}

fn persist_launch_configs(path: &PathBuf, launch_configs: &[AgentLaunchConfig]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("invalid launch config path {}", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create config directory {}", parent.display()))?;

    let file = AgentsFile {
        schema_version: default_schema_version(),
        agents: launch_configs.to_vec(),
    };
    let text = serde_json::to_string_pretty(&file).context("failed to serialize launch config")?;
    fs::write(path, text)
        .with_context(|| format!("failed to write launch config file {}", path.display()))?;
    Ok(())
}

fn spawn_configured_agents(launch_configs: &[AgentLaunchConfig]) -> anyhow::Result<()> {
    if launch_configs.is_empty() {
        return Ok(());
    }

    let agent_binary = resolve_agent_binary()?;
    for config in launch_configs {
        let mut cmd = Command::new(&agent_binary);
        cmd.arg("--workspace-dir")
            .arg(&config.workspace_dir)
            .arg("--port")
            .arg(config.port.to_string());

        if let Some(agent_name) = &config.agent_name {
            cmd.env("LOOPER_AGENT_NAME", agent_name);
        }

        match cmd.spawn() {
            Ok(_) => {
                println!(
                    "launched configured agent for {} on port {}",
                    config.workspace_dir, config.port
                );
            }
            Err(error) => {
                eprintln!(
                    "failed to launch configured agent for {} on port {}: {}",
                    config.workspace_dir, config.port, error
                );
            }
        }
    }

    Ok(())
}

fn resolve_agent_binary() -> anyhow::Result<PathBuf> {
    let current_exe = env::current_exe().context("failed to resolve current executable path")?;
    let executable_name = if cfg!(windows) {
        "looper-agent.exe"
    } else {
        "looper-agent"
    };

    let sibling = current_exe.with_file_name(executable_name);
    if sibling.exists() {
        return Ok(sibling);
    }

    Ok(PathBuf::from(executable_name))
}
