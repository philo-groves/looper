use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use looper_common::{
    AGENT_PORT_END, AGENT_PORT_START, AgentInfo, DISCOVERY_HOST, DISCOVERY_PORT, DiscoveryRequest,
    DiscoveryResponse,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::{accept_async, tungstenite::Message};
use uuid::Uuid;

#[derive(Default)]
struct DiscoveryState {
    agents: HashMap<String, AgentInfo>,
    used_ports: HashSet<u16>,
}

impl DiscoveryState {
    fn assign_port(&mut self) -> Option<u16> {
        for port in AGENT_PORT_START..=AGENT_PORT_END {
            if !self.used_ports.contains(&port) {
                self.used_ports.insert(port);
                return Some(port);
            }
        }

        None
    }

    fn release_port(&mut self, port: u16) {
        self.used_ports.remove(&port);
    }

    fn active_agents(&self) -> Vec<AgentInfo> {
        self.agents.values().cloned().collect()
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let bind_addr = format!("{DISCOVERY_HOST}:{DISCOVERY_PORT}");
    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind discovery server to {bind_addr}"))?;

    println!("discovery listening on ws://{bind_addr}");

    let state = Arc::new(Mutex::new(DiscoveryState::default()));

    loop {
        let (stream, addr) = listener
            .accept()
            .await
            .context("failed to accept tcp connection")?;

        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, state).await {
                eprintln!("connection {addr} failed: {error:#}");
            }
        });
    }
}

async fn handle_connection(
    stream: TcpStream,
    state: Arc<Mutex<DiscoveryState>>,
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
        DiscoveryRequest::Register { agent_name } => {
            let mut state_guard = state.lock().await;
            let active_agents = state_guard.active_agents();

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

            let agent_info = AgentInfo {
                agent_id: Uuid::new_v4().to_string(),
                agent_name,
                assigned_port,
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
