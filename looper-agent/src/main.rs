use std::env;

use anyhow::{Context, bail};
use futures_util::{SinkExt, StreamExt};
use looper_common::{
    AGENT_HOST, AgentInfo, DEFAULT_DISCOVERY_URL, DiscoveryRequest, DiscoveryResponse,
};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, connect_async, tungstenite::Message};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let discovery_url =
        env::var("LOOPER_DISCOVERY_URL").unwrap_or_else(|_| DEFAULT_DISCOVERY_URL.to_string());
    let agent_name = env::var("LOOPER_AGENT_NAME").ok();

    let (ws_stream, _) = connect_async(&discovery_url)
        .await
        .with_context(|| format!("failed to connect to discovery server at {discovery_url}"))?;

    let (mut writer, mut reader) = ws_stream.split();

    let register_request = serde_json::to_string(&DiscoveryRequest::Register { agent_name })?;
    writer
        .send(Message::Text(register_request.into()))
        .await
        .context("failed to send register request")?;

    let registration = wait_for_registration(&mut reader).await?;

    println!(
        "registered agent {} and assigned websocket port {}",
        registration.agent_id, registration.assigned_port
    );
    if !registration.active_agents.is_empty() {
        println!(
            "{} other active agent(s) discovered at startup",
            registration.active_agents.len()
        );
        for agent in &registration.active_agents {
            println!(
                "- active agent {} ({}) on ws://{}:{}",
                agent.agent_id,
                agent.agent_name.as_deref().unwrap_or("unnamed"),
                AGENT_HOST,
                agent.assigned_port
            );
        }
    }

    let server_handle = tokio::spawn(run_agent_server(
        registration.agent_id.clone(),
        registration.assigned_port,
    ));

    while let Some(message) = reader.next().await {
        match message {
            Ok(Message::Close(_)) => {
                println!("discovery connection closed");
                break;
            }
            Ok(_) => {}
            Err(error) => {
                eprintln!("discovery connection error: {error}");
                break;
            }
        }
    }

    server_handle.abort();
    Ok(())
}

async fn wait_for_registration(
    reader: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    >,
) -> anyhow::Result<RegistrationInfo> {
    while let Some(message) = reader.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let response: DiscoveryResponse = serde_json::from_str(&text)
                    .with_context(|| format!("invalid discovery response: {text}"))?;

                match response {
                    DiscoveryResponse::Registered {
                        agent_id,
                        assigned_port,
                        active_agents,
                    } => {
                        return Ok(RegistrationInfo {
                            agent_id,
                            assigned_port,
                            active_agents,
                        });
                    }
                    DiscoveryResponse::Error { message } => {
                        bail!("discovery registration failed: {message}");
                    }
                    DiscoveryResponse::Agents { .. } => {}
                }
            }
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
    }

    bail!("discovery server closed before registration completed")
}

struct RegistrationInfo {
    agent_id: String,
    assigned_port: u16,
    active_agents: Vec<AgentInfo>,
}

async fn run_agent_server(agent_id: String, assigned_port: u16) -> anyhow::Result<()> {
    let bind_addr = format!("{AGENT_HOST}:{assigned_port}");
    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind agent websocket server to {bind_addr}"))?;

    println!("agent {agent_id} listening for user websocket on ws://{bind_addr}");

    loop {
        let (stream, _) = listener
            .accept()
            .await
            .context("failed to accept agent websocket connection")?;
        let agent_id = agent_id.clone();

        tokio::spawn(async move {
            if let Err(error) = handle_user_socket(stream, agent_id).await {
                eprintln!("user websocket handler failed: {error:#}");
            }
        });
    }
}

async fn handle_user_socket(stream: TcpStream, agent_id: String) -> anyhow::Result<()> {
    let ws_stream = accept_async(stream)
        .await
        .context("agent websocket handshake failed")?;
    let (mut writer, mut reader) = ws_stream.split();

    writer
        .send(Message::Text(
            serde_json::json!({
                "type": "agent_ready",
                "agent_id": agent_id,
            })
            .to_string()
            .into(),
        ))
        .await
        .context("failed to send agent_ready message")?;

    while let Some(message) = reader.next().await {
        match message {
            Ok(Message::Text(text)) => {
                writer
                    .send(Message::Text(text))
                    .await
                    .context("failed to echo user message")?;
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
    }

    Ok(())
}
