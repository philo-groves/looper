use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, bail};
use futures_util::{SinkExt, StreamExt};
use looper_common::{
    AGENT_HOST, AgentInfo, AgentMode, AgentSocketMessage, DEFAULT_DISCOVERY_URL, DiscoveryRequest,
    DiscoveryResponse,
};
use looper_agent::settings::{
    AgentKeys, AgentSettings, PersistedAgentConfig, is_config_complete, load_persisted_config,
    normalize_workspace_dir, persist_config,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::{accept_async, connect_async, tungstenite::Message};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli_args = parse_args()?;
    let discovery_url =
        env::var("LOOPER_DISCOVERY_URL").unwrap_or_else(|_| DEFAULT_DISCOVERY_URL.to_string());
    let agent_name = env::var("LOOPER_AGENT_NAME").ok();

    let workspace_hint = match &cli_args.workspace_dir {
        Some(path) => Some(normalize_workspace_dir(path)?),
        None => None,
    };

    let persisted_config = match &workspace_hint {
        Some(path) => load_persisted_config(path)?,
        None => None,
    };
    let startup_mode = if persisted_config
        .as_ref()
        .is_some_and(is_config_complete)
    {
        AgentMode::Running
    } else {
        AgentMode::Setup
    };

    let (ws_stream, _) = connect_async(&discovery_url)
        .await
        .with_context(|| format!("failed to connect to discovery server at {discovery_url}"))?;

    let (mut writer, mut reader) = ws_stream.split();

    let register_request = serde_json::to_string(&DiscoveryRequest::Register {
        agent_name: agent_name.clone(),
        requested_port: cli_args.port,
        workspace_dir: cli_args.workspace_dir.clone(),
        mode: startup_mode,
    })?;
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

    let runtime = Arc::new(Mutex::new(AgentRuntime {
        agent_id: registration.agent_id.clone(),
        assigned_port: registration.assigned_port,
        mode: startup_mode,
        persisted: persisted_config,
        workspace_hint,
        agent_name,
    }));

    let server_handle = tokio::spawn(run_agent_server(runtime, discovery_url.clone()));

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
                    DiscoveryResponse::Agents { .. } | DiscoveryResponse::AgentLaunchUpserted => {}
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

struct CliArgs {
    port: Option<u16>,
    workspace_dir: Option<String>,
}

struct AgentRuntime {
    agent_id: String,
    assigned_port: u16,
    mode: AgentMode,
    persisted: Option<PersistedAgentConfig>,
    workspace_hint: Option<PathBuf>,
    agent_name: Option<String>,
}

fn parse_args() -> anyhow::Result<CliArgs> {
    let mut args = env::args().skip(1);
    let mut port = None;
    let mut workspace_dir = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => {
                let raw = args
                    .next()
                    .context("--port requires a numeric value")?;
                let parsed = raw
                    .parse::<u16>()
                    .with_context(|| format!("invalid --port value: {raw}"))?;
                port = Some(parsed);
            }
            "--workspace-dir" => {
                let value = args
                    .next()
                    .context("--workspace-dir requires a directory path")?;
                workspace_dir = Some(value);
            }
            _ => bail!("unsupported argument: {arg}"),
        }
    }

    Ok(CliArgs {
        port,
        workspace_dir,
    })
}

async fn run_agent_server(
    runtime: Arc<Mutex<AgentRuntime>>,
    discovery_url: String,
) -> anyhow::Result<()> {
    let runtime_guard = runtime.lock().await;
    let bind_addr = format!("{AGENT_HOST}:{}", runtime_guard.assigned_port);
    let agent_id = runtime_guard.agent_id.clone();
    let mode = runtime_guard.mode;
    drop(runtime_guard);

    let listener = TcpListener::bind(&bind_addr)
        .await
        .with_context(|| format!("failed to bind agent websocket server to {bind_addr}"))?;

    println!("agent {agent_id} ({mode:?}) listening for user websocket on ws://{bind_addr}");

    loop {
        let (stream, _) = listener
            .accept()
            .await
            .context("failed to accept agent websocket connection")?;
        let runtime = Arc::clone(&runtime);
        let discovery_url = discovery_url.clone();

        tokio::spawn(async move {
            if let Err(error) = handle_user_socket(stream, runtime, discovery_url).await {
                eprintln!("user websocket handler failed: {error:#}");
            }
        });
    }
}

async fn handle_user_socket(
    stream: TcpStream,
    runtime: Arc<Mutex<AgentRuntime>>,
    discovery_url: String,
) -> anyhow::Result<()> {
    let ws_stream = accept_async(stream)
        .await
        .context("agent websocket handshake failed")?;
    let (mut writer, mut reader) = ws_stream.split();

    let runtime_guard = runtime.lock().await;
    let hello = AgentSocketMessage::AgentHello {
        agent_id: runtime_guard.agent_id.clone(),
        mode: runtime_guard.mode,
    };
    drop(runtime_guard);

    writer
        .send(Message::Text(
            serde_json::to_string(&hello)?
            .into(),
        ))
        .await
        .context("failed to send agent hello message")?;

    while let Some(message) = reader.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let parsed = serde_json::from_str::<AgentSocketMessage>(&text)
                    .unwrap_or(AgentSocketMessage::UserText { text: text.to_string() });

                match parsed {
                    AgentSocketMessage::SetupSubmit {
                        workspace_dir,
                        port,
                        provider,
                        api_keys,
                    } => match complete_setup(
                        &runtime,
                        &discovery_url,
                        workspace_dir,
                        port,
                        provider,
                        api_keys,
                    )
                    .await
                    {
                        Ok(()) => {
                            let accepted = AgentSocketMessage::SetupAccepted {
                                mode: AgentMode::Running,
                            };
                            writer
                                .send(Message::Text(serde_json::to_string(&accepted)?.into()))
                                .await
                                .context("failed to send setup accepted message")?;
                        }
                        Err(error) => {
                            let response = AgentSocketMessage::Error {
                                message: error.to_string(),
                            };
                            writer
                                .send(Message::Text(serde_json::to_string(&response)?.into()))
                                .await
                                .context("failed to send setup error")?;
                        }
                    },
                    AgentSocketMessage::UserText { text } => {
                        let runtime_guard = runtime.lock().await;
                        if runtime_guard.mode != AgentMode::Running {
                            drop(runtime_guard);
                            let response = AgentSocketMessage::Error {
                                message: "agent is in setup mode".to_string(),
                            };
                            writer
                                .send(Message::Text(serde_json::to_string(&response)?.into()))
                                .await
                                .context("failed to send setup mode warning")?;
                            continue;
                        }
                        drop(runtime_guard);

                        let response = AgentSocketMessage::UserText { text };
                        writer
                            .send(Message::Text(serde_json::to_string(&response)?.into()))
                            .await
                            .context("failed to echo user message")?;
                    }
                    AgentSocketMessage::AgentHello { .. }
                    | AgentSocketMessage::SetupAccepted { .. }
                    | AgentSocketMessage::Error { .. } => {}
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
    }

    Ok(())
}

async fn complete_setup(
    runtime: &Arc<Mutex<AgentRuntime>>,
    discovery_url: &str,
    workspace_dir: String,
    port: u16,
    provider: String,
    api_keys: Vec<looper_common::ProviderApiKey>,
) -> anyhow::Result<()> {
    let workspace_path = normalize_workspace_dir(&workspace_dir)?;
    let runtime_guard = runtime.lock().await;

    if port != runtime_guard.assigned_port {
        bail!(
            "setup port {} does not match assigned port {}",
            port,
            runtime_guard.assigned_port
        );
    }

    let settings = AgentSettings {
        workspace_dir: workspace_path.to_string_lossy().to_string(),
        port,
        provider,
    };
    let keys = AgentKeys { api_keys };

    let persisted = persist_config(&workspace_path, settings, keys)?;
    if !is_config_complete(&persisted) {
        bail!("setup data is incomplete: provider API key is missing");
    }

    let agent_name = runtime_guard.agent_name.clone();
    drop(runtime_guard);

    upsert_launch_config(
        discovery_url,
        persisted.settings.workspace_dir.clone(),
        port,
        agent_name,
    )
    .await?;

    let mut runtime_guard = runtime.lock().await;
    runtime_guard.persisted = Some(persisted);
    runtime_guard.workspace_hint = Some(workspace_path);
    runtime_guard.mode = AgentMode::Running;
    Ok(())
}

async fn upsert_launch_config(
    discovery_url: &str,
    workspace_dir: String,
    port: u16,
    agent_name: Option<String>,
) -> anyhow::Result<()> {
    let (ws_stream, _) = connect_async(discovery_url)
        .await
        .with_context(|| format!("failed to connect to discovery server at {discovery_url}"))?;
    let (mut writer, mut reader) = ws_stream.split();

    let request = DiscoveryRequest::UpsertAgentLaunch {
        workspace_dir,
        port,
        agent_name,
    };
    writer
        .send(Message::Text(serde_json::to_string(&request)?.into()))
        .await
        .context("failed to send upsert launch request")?;

    while let Some(message) = reader.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let response: DiscoveryResponse = serde_json::from_str(&text)
                    .with_context(|| format!("invalid discovery response: {text}"))?;
                match response {
                    DiscoveryResponse::AgentLaunchUpserted => return Ok(()),
                    DiscoveryResponse::Error { message } => {
                        bail!("discovery could not persist launch config: {message}")
                    }
                    DiscoveryResponse::Registered { .. } | DiscoveryResponse::Agents { .. } => {}
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
    }

    bail!("discovery server closed before upsert confirmation")
}
