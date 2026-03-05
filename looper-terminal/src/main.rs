use std::env;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::{SinkExt, StreamExt};
use looper_common::{
    AGENT_HOST, AgentEntry, AgentInfo, AgentMode, AgentSocketMessage, DEFAULT_DISCOVERY_URL,
    DiscoveryRequest, DiscoveryResponse, Effect, Percept, PlannedAction, PlannedActionStatus,
    PluginCommandRequest, ProviderApiKey, SessionOrigin,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Padding, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use ratatui_widgets::list::{List, ListItem, ListState};
use ratatui_widgets::scrollbar::{Scrollbar, ScrollbarOrientation, ScrollbarState};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const TICK_RATE: Duration = Duration::from_millis(450);
const PROVIDERS: [&str; 3] = ["openai", "anthropic", "opencode-zen"];

fn default_model_for_provider(provider: &str) -> &'static str {
    match provider {
        "openai" => "gpt-4o-mini",
        "anthropic" => "claude-3-5-sonnet-latest",
        "opencode-zen" => "openai/gpt-5.1-mini",
        _ => "gpt-4o-mini",
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let agents = discover_agents().await?;
    let selected = run_agent_selector(agents)?;

    let runtime_agent = match selected {
        SelectorOutcome::Quit => return Ok(()),
        SelectorOutcome::CreateNew => create_new_agent().await?,
        SelectorOutcome::Selected(mut agent) => {
            if !agent.is_running {
                start_agent(&agent.workspace_dir).await?;
                agent.is_running = true;
            }

            wait_for_agent_online(&agent.workspace_dir, agent.assigned_port).await?
        }
    };

    let mode = fetch_agent_mode(&runtime_agent).await?;
    if mode == AgentMode::Setup {
        let Some(form) = run_setup_flow(&runtime_agent)? else {
            return Ok(());
        };
        submit_setup(&runtime_agent, form).await?;
    }

    run_chat_ui(&runtime_agent)
}

async fn discover_agents() -> anyhow::Result<Vec<AgentEntry>> {
    let discovery_url =
        env::var("LOOPER_DISCOVERY_URL").unwrap_or_else(|_| DEFAULT_DISCOVERY_URL.to_string());

    let (ws_stream, _) = connect_async(&discovery_url)
        .await
        .with_context(|| format!("failed to connect to discovery server at {discovery_url}"))?;
    let (mut writer, mut reader) = ws_stream.split();

    let list_request = serde_json::to_string(&DiscoveryRequest::ListAgents)?;
    writer
        .send(Message::Text(list_request.into()))
        .await
        .context("failed to send list-agents request")?;

    while let Some(message) = reader.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let response: DiscoveryResponse = serde_json::from_str(&text)
                    .with_context(|| format!("invalid discovery response: {text}"))?;
                match response {
                    DiscoveryResponse::Agents { agents } => return Ok(agents),
                    DiscoveryResponse::Error { message } => {
                        bail!("discovery server returned error: {message}")
                    }
                    DiscoveryResponse::Registered { .. }
                    | DiscoveryResponse::AgentLaunchUpserted
                    | DiscoveryResponse::AgentStarted { .. }
                    | DiscoveryResponse::AgentCreated { .. } => {}
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
    }

    bail!("discovery server closed before listing agents")
}

enum SelectorOutcome {
    Quit,
    Selected(AgentEntry),
    CreateNew,
}

fn run_agent_selector(agents: Vec<AgentEntry>) -> anyhow::Result<SelectorOutcome> {
    let mut app = AgentSelectorApp {
        agents,
        selected_index: 0,
        should_quit: false,
        confirmed: false,
        create_new: false,
    };

    run_tui_loop(&mut app, draw_agent_selector, handle_agent_selector_key)?;
    if app.create_new {
        Ok(SelectorOutcome::CreateNew)
    } else if app.confirmed {
        Ok(app
            .agents
            .get(app.selected_index)
            .cloned()
            .map(SelectorOutcome::Selected)
            .unwrap_or(SelectorOutcome::Quit))
    } else {
        Ok(SelectorOutcome::Quit)
    }
}

async fn create_new_agent() -> anyhow::Result<AgentInfo> {
    let discovery_url =
        env::var("LOOPER_DISCOVERY_URL").unwrap_or_else(|_| DEFAULT_DISCOVERY_URL.to_string());

    let (ws_stream, _) = connect_async(&discovery_url)
        .await
        .with_context(|| format!("failed to connect to discovery server at {discovery_url}"))?;
    let (mut writer, mut reader) = ws_stream.split();

    writer
        .send(Message::Text(
            serde_json::to_string(&DiscoveryRequest::CreateAgent)?.into(),
        ))
        .await
        .context("failed to send create-agent request")?;

    let mut assigned_port = None;
    while let Some(message) = reader.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let response: DiscoveryResponse = serde_json::from_str(&text)
                    .with_context(|| format!("invalid discovery response: {text}"))?;
                match response {
                    DiscoveryResponse::AgentCreated {
                        assigned_port: port,
                    } => {
                        assigned_port = Some(port);
                        break;
                    }
                    DiscoveryResponse::Error { message } => {
                        bail!("discovery failed to create agent: {message}")
                    }
                    DiscoveryResponse::Registered { .. }
                    | DiscoveryResponse::Agents { .. }
                    | DiscoveryResponse::AgentLaunchUpserted
                    | DiscoveryResponse::AgentStarted { .. } => {}
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
    }

    let assigned_port = assigned_port.context("discovery closed before create-agent response")?;
    wait_for_new_agent_ready(assigned_port).await
}

async fn wait_for_new_agent_ready(assigned_port: u16) -> anyhow::Result<AgentInfo> {
    let deadline = Instant::now() + Duration::from_secs(5);

    loop {
        let url = format!("ws://{AGENT_HOST}:{assigned_port}");
        let ws_stream = match connect_async(&url).await {
            Ok((ws_stream, _)) => ws_stream,
            Err(_) => {
                if Instant::now() >= deadline {
                    bail!("timed out waiting for new agent on port {assigned_port}");
                }
                tokio::time::sleep(Duration::from_millis(150)).await;
                continue;
            }
        };
        let (mut writer, mut reader) = ws_stream.split();

        while let Some(message) = reader.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    let payload: AgentSocketMessage = serde_json::from_str(&text)
                        .with_context(|| format!("invalid agent hello payload: {text}"))?;
                    if let AgentSocketMessage::AgentHello { agent_id, mode } = payload {
                        writer.send(Message::Close(None)).await.ok();
                        return Ok(AgentInfo {
                            agent_id,
                            agent_name: None,
                            assigned_port,
                            mode,
                            workspace_dir: None,
                        });
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }

        if Instant::now() >= deadline {
            bail!("timed out waiting for new agent handshake on port {assigned_port}");
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

async fn start_agent(workspace_dir: &str) -> anyhow::Result<()> {
    let discovery_url =
        env::var("LOOPER_DISCOVERY_URL").unwrap_or_else(|_| DEFAULT_DISCOVERY_URL.to_string());

    let (ws_stream, _) = connect_async(&discovery_url)
        .await
        .with_context(|| format!("failed to connect to discovery server at {discovery_url}"))?;
    let (mut writer, mut reader) = ws_stream.split();

    let request = DiscoveryRequest::StartAgent {
        workspace_dir: workspace_dir.to_string(),
    };
    writer
        .send(Message::Text(serde_json::to_string(&request)?.into()))
        .await
        .context("failed to send start-agent request")?;

    while let Some(message) = reader.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let response: DiscoveryResponse = serde_json::from_str(&text)
                    .with_context(|| format!("invalid discovery response: {text}"))?;
                match response {
                    DiscoveryResponse::AgentStarted { .. } => return Ok(()),
                    DiscoveryResponse::Error { message } => {
                        bail!("discovery failed to start agent: {message}")
                    }
                    DiscoveryResponse::Registered { .. }
                    | DiscoveryResponse::Agents { .. }
                    | DiscoveryResponse::AgentLaunchUpserted
                    | DiscoveryResponse::AgentCreated { .. } => {}
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
    }

    bail!("discovery disconnected before start confirmation")
}

async fn wait_for_agent_online(workspace_dir: &str, port: u16) -> anyhow::Result<AgentInfo> {
    let discovery_url =
        env::var("LOOPER_DISCOVERY_URL").unwrap_or_else(|_| DEFAULT_DISCOVERY_URL.to_string());

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let ws_stream = match connect_async(&discovery_url).await {
            Ok((ws_stream, _)) => ws_stream,
            Err(_) => {
                if Instant::now() >= deadline {
                    bail!("timed out waiting for discovery while starting agent");
                }
                tokio::time::sleep(Duration::from_millis(150)).await;
                continue;
            }
        };
        let (mut writer, mut reader) = ws_stream.split();

        let list_request = serde_json::to_string(&DiscoveryRequest::ListAgents)?;
        if writer
            .send(Message::Text(list_request.into()))
            .await
            .is_err()
        {
            if Instant::now() >= deadline {
                bail!("timed out sending list-agents request while waiting for agent start");
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
            continue;
        }

        while let Some(message) = reader.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    let response: DiscoveryResponse = serde_json::from_str(&text)
                        .with_context(|| format!("invalid discovery response: {text}"))?;
                    if let DiscoveryResponse::Agents { agents } = response {
                        if let Some(entry) = agents
                            .iter()
                            .find(|entry| entry.workspace_dir == workspace_dir && entry.is_running)
                        {
                            return Ok(AgentInfo {
                                agent_id: entry
                                    .agent_id
                                    .clone()
                                    .unwrap_or_else(|| "pending".to_string()),
                                agent_name: entry.agent_name.clone(),
                                assigned_port: entry.assigned_port,
                                mode: entry.mode.unwrap_or(AgentMode::Setup),
                                workspace_dir: Some(entry.workspace_dir.clone()),
                            });
                        }

                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }

        if Instant::now() >= deadline {
            bail!(
                "timed out waiting for agent to come online on port {} for workspace {}",
                port,
                workspace_dir
            );
        }

        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

async fn fetch_agent_mode(agent: &AgentInfo) -> anyhow::Result<AgentMode> {
    let url = format!("ws://{AGENT_HOST}:{}", agent.assigned_port);
    let (ws_stream, _) = connect_async(&url)
        .await
        .with_context(|| format!("failed to connect to agent websocket at {url}"))?;
    let (mut writer, mut reader) = ws_stream.split();

    while let Some(message) = reader.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let payload: AgentSocketMessage = serde_json::from_str(&text)
                    .with_context(|| format!("invalid agent hello payload: {text}"))?;
                if let AgentSocketMessage::AgentHello { mode, .. } = payload {
                    writer.send(Message::Close(None)).await.ok();
                    return Ok(mode);
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
    }

    bail!("agent disconnected before sending mode")
}

struct SetupForm {
    workspace_dir: String,
    provider: String,
    model: String,
    api_key: String,
}

fn run_setup_flow(agent: &AgentInfo) -> anyhow::Result<Option<SetupForm>> {
    let mut app = SetupApp {
        stage: SetupStage::Workspace,
        should_quit: false,
        workspace_input: String::new(),
        provider_index: 0,
        model_input: default_model_for_provider(PROVIDERS[0]).to_string(),
        api_key_input: String::new(),
        confirm_index: 0,
        error_message: None,
        cursor_visible: true,
        agent_port: agent.assigned_port,
    };

    run_tui_loop(&mut app, draw_setup, handle_setup_key)?;

    if app.stage != SetupStage::Done {
        return Ok(None);
    }

    Ok(Some(SetupForm {
        workspace_dir: app.workspace_input.trim().to_string(),
        provider: PROVIDERS[app.provider_index].to_string(),
        model: app.model_input.trim().to_string(),
        api_key: app.api_key_input.trim().to_string(),
    }))
}

async fn submit_setup(agent: &AgentInfo, form: SetupForm) -> anyhow::Result<()> {
    let url = format!("ws://{AGENT_HOST}:{}", agent.assigned_port);
    let (ws_stream, _) = connect_async(&url)
        .await
        .with_context(|| format!("failed to connect to agent websocket at {url}"))?;
    let (mut writer, mut reader) = ws_stream.split();

    while let Some(message) = reader.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let payload: AgentSocketMessage = serde_json::from_str(&text)
                    .with_context(|| format!("invalid agent payload: {text}"))?;
                if let AgentSocketMessage::AgentHello { mode, .. } = payload {
                    if mode != AgentMode::Setup {
                        writer.send(Message::Close(None)).await.ok();
                        return Ok(());
                    }
                    break;
                }
            }
            Ok(Message::Close(_)) => bail!("agent disconnected before setup handshake"),
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
    }

    let submit = AgentSocketMessage::SetupSubmit {
        workspace_dir: form.workspace_dir,
        port: agent.assigned_port,
        provider: form.provider.clone(),
        model: form.model,
        api_keys: vec![ProviderApiKey {
            provider: form.provider,
            api_key: form.api_key,
        }],
    };

    writer
        .send(Message::Text(serde_json::to_string(&submit)?.into()))
        .await
        .context("failed to send setup submission")?;

    while let Some(message) = reader.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let payload: AgentSocketMessage = serde_json::from_str(&text)
                    .with_context(|| format!("invalid setup response payload: {text}"))?;
                match payload {
                    AgentSocketMessage::SetupAccepted { mode } if mode == AgentMode::Running => {
                        println!("Agent setup completed and switched to running mode.");
                        writer.send(Message::Close(None)).await.ok();
                        return Ok(());
                    }
                    AgentSocketMessage::Error { message } => {
                        bail!("setup failed: {message}");
                    }
                    _ => {}
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
    }

    bail!("agent disconnected before setup confirmation")
}

fn run_chat_ui(agent: &AgentInfo) -> anyhow::Result<()> {
    let connection_state = Arc::new(AtomicBool::new(false));
    let monitor_state = Arc::clone(&connection_state);
    let agent_port = agent.assigned_port;
    let monitor_handle = tokio::spawn(async move {
        monitor_agent_connection(agent_port, monitor_state).await;
    });

    let (chat_cmd_tx, chat_cmd_rx) = unbounded_channel();
    let (chat_event_tx, chat_event_rx) = unbounded_channel();
    let socket_port = agent.assigned_port;
    let socket_handle = tokio::spawn(async move {
        chat_socket_loop(socket_port, chat_cmd_rx, chat_event_tx).await;
    });

    let project_workspace = env::current_dir()
        .ok()
        .map(|path| path.to_string_lossy().into_owned());

    let mut app = ChatApp {
        should_quit: false,
        input: String::new(),
        input_view_backscroll: 0,
        cursor_visible: true,
        messages: Vec::new(),
        scroll_offset: 0,
        history_max_scroll: 0,
        follow_tail: true,
        status: ChatStatus::Idle,
        status_ticks: 0,
        ws_status: WebSocketStatus::Disconnected,
        connection_state,
        agent_name: agent.agent_name.clone(),
        agent_workspace: agent.workspace_dir.clone(),
        project_workspace,
        chat_cmd_tx,
        chat_event_rx,
        session_id: None,
        next_turn_id: 1,
        active_provider: "(pending)".to_string(),
        active_model: "(pending)".to_string(),
        agent_port: agent.assigned_port,
        planned_actions: Vec::new(),
    };

    let result = run_tui_loop(&mut app, draw_chat, handle_chat_key);
    let _ = app.chat_cmd_tx.send(ChatCommand::EndSession);
    monitor_handle.abort();
    socket_handle.abort();
    result
}

#[derive(Debug)]
enum ChatCommand {
    SendPercept { turn_id: String, text: String },
    PluginCommand { command: PluginCommandRequest },
    EndSession,
}

#[derive(Debug)]
enum ChatEvent {
    SessionStarted {
        session_id: String,
        provider: String,
        model: String,
    },
    EffectApplied {
        effect: Effect,
    },
    Error {
        message: String,
    },
    PluginCommandResult {
        success: bool,
        message: String,
    },
    Disconnected,
}

async fn chat_socket_loop(
    assigned_port: u16,
    mut cmd_rx: UnboundedReceiver<ChatCommand>,
    event_tx: UnboundedSender<ChatEvent>,
) {
    let url = format!("ws://{AGENT_HOST}:{assigned_port}");
    let (ws_stream, _) = match connect_async(&url).await {
        Ok(value) => value,
        Err(error) => {
            let _ = event_tx.send(ChatEvent::Error {
                message: format!("chat connection failed: {error}"),
            });
            let _ = event_tx.send(ChatEvent::Disconnected);
            return;
        }
    };
    let (mut writer, mut reader) = ws_stream.split();

    let mut session_id: Option<String> = None;
    let mut started = false;
    while let Some(message) = reader.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let payload: AgentSocketMessage = match serde_json::from_str(&text) {
                    Ok(parsed) => parsed,
                    Err(error) => {
                        let _ = event_tx.send(ChatEvent::Error {
                            message: format_handshake_parse_error(&text, &error),
                        });
                        continue;
                    }
                };
                match payload {
                    AgentSocketMessage::AgentHello { mode, .. } => {
                        if mode != AgentMode::Running {
                            let _ = event_tx.send(ChatEvent::Error {
                                message: "agent is not in running mode".to_string(),
                            });
                            let _ = event_tx.send(ChatEvent::Disconnected);
                            return;
                        }
                        let request = AgentSocketMessage::SessionStart {
                            origin: SessionOrigin::TerminalChat,
                        };
                        if writer
                            .send(Message::Text(
                                serde_json::to_string(&request)
                                    .unwrap_or_else(|_| "{}".to_string())
                                    .into(),
                            ))
                            .await
                            .is_err()
                        {
                            let _ = event_tx.send(ChatEvent::Error {
                                message: "failed to request chat session".to_string(),
                            });
                            let _ = event_tx.send(ChatEvent::Disconnected);
                            return;
                        }
                    }
                    AgentSocketMessage::SessionStarted {
                        session_id: id,
                        provider,
                        model,
                        ..
                    } => {
                        session_id = Some(id.clone());
                        started = true;
                        let _ = event_tx.send(ChatEvent::SessionStarted {
                            session_id: id,
                            provider,
                            model,
                        });
                        break;
                    }
                    AgentSocketMessage::Error { message } => {
                        let _ = event_tx.send(ChatEvent::Error { message });
                    }
                    _ => {}
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => {
                let _ = event_tx.send(ChatEvent::Error {
                    message: format!("socket handshake read failed: {error}"),
                });
                break;
            }
        }
    }

    if !started {
        let _ = event_tx.send(ChatEvent::Disconnected);
        return;
    }

    loop {
        tokio::select! {
            maybe_cmd = cmd_rx.recv() => {
                let Some(cmd) = maybe_cmd else {
                    break;
                };

                match cmd {
                    ChatCommand::SendPercept { turn_id, text } => {
                        let Some(active_session_id) = session_id.clone() else {
                            let _ = event_tx.send(ChatEvent::Error { message: "session is not started".to_string() });
                            continue;
                        };
                        let percept = AgentSocketMessage::PerceptObserved {
                            session_id: active_session_id,
                            domain: "chat".to_string(),
                            percept: Percept::UserText { turn_id, text },
                        };
                        if let Err(error) = writer
                            .send(Message::Text(
                                serde_json::to_string(&percept)
                                    .unwrap_or_else(|_| "{}".to_string())
                                    .into(),
                            ))
                            .await
                        {
                            let _ = event_tx.send(ChatEvent::Error {
                                message: format!("failed to send percept: {error}"),
                            });
                            break;
                        }
                    }
                    ChatCommand::PluginCommand { command } => {
                        let request = AgentSocketMessage::PluginCommand { command };
                        if let Err(error) = writer
                            .send(Message::Text(
                                serde_json::to_string(&request)
                                    .unwrap_or_else(|_| "{}".to_string())
                                    .into(),
                            ))
                            .await
                        {
                            let _ = event_tx.send(ChatEvent::Error {
                                message: format!("failed to send plugin command: {error}"),
                            });
                            break;
                        }
                    }
                    ChatCommand::EndSession => {
                        if let Some(active_session_id) = session_id.clone() {
                            let end = AgentSocketMessage::SessionEnd {
                                session_id: active_session_id,
                            };
                            let _ = writer
                                .send(Message::Text(
                                    serde_json::to_string(&end)
                                        .unwrap_or_else(|_| "{}".to_string())
                                        .into(),
                                ))
                                .await;
                        }
                        let _ = writer.send(Message::Close(None)).await;
                        break;
                    }
                }
            }
            maybe_msg = reader.next() => {
                let Some(message) = maybe_msg else {
                    break;
                };
                match message {
                    Ok(Message::Text(text)) => {
                        let payload: AgentSocketMessage = match serde_json::from_str(&text) {
                            Ok(parsed) => parsed,
                            Err(error) => {
                                let _ = event_tx.send(ChatEvent::Error {
                                    message: format!("invalid socket payload: {error}"),
                                });
                                continue;
                            }
                        };
                        match payload {
                            AgentSocketMessage::EffectApplied { effect, .. } => {
                                let _ = event_tx.send(ChatEvent::EffectApplied { effect });
                            }
                            AgentSocketMessage::PluginCommandResult {
                                success, message, ..
                            } => {
                                let _ = event_tx.send(ChatEvent::PluginCommandResult {
                                    success,
                                    message,
                                });
                            }
                            AgentSocketMessage::Error { message } => {
                                let _ = event_tx.send(ChatEvent::Error { message });
                            }
                            _ => {}
                        }
                    }
                    Ok(Message::Close(_)) => break,
                    Ok(_) => {}
                    Err(error) => {
                        let _ = event_tx.send(ChatEvent::Error {
                            message: format!("socket read failed: {error}"),
                        });
                        break;
                    }
                }
            }
        }
    }

    let _ = event_tx.send(ChatEvent::Disconnected);
}

fn format_handshake_parse_error(text: &str, error: &serde_json::Error) -> String {
    let error_text = error.to_string();
    if error_text.contains("unknown variant `user_text`") {
        return "received legacy 'user_text' protocol from agent; restart looper-discovery and looper-agent so both run the new PEAS protocol binaries".to_string();
    }

    format!("invalid handshake payload: {error}; payload={text}")
}

async fn monitor_agent_connection(agent_port: u16, state: Arc<AtomicBool>) {
    loop {
        let connected =
            tokio::time::timeout(Duration::from_millis(700), is_agent_running(agent_port))
                .await
                .unwrap_or(false);
        state.store(connected, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn is_agent_running(agent_port: u16) -> bool {
    let discovery_url =
        env::var("LOOPER_DISCOVERY_URL").unwrap_or_else(|_| DEFAULT_DISCOVERY_URL.to_string());

    let (ws_stream, _) = match connect_async(&discovery_url).await {
        Ok(value) => value,
        Err(_) => return false,
    };
    let (mut writer, mut reader) = ws_stream.split();

    let list_request = match serde_json::to_string(&DiscoveryRequest::ListAgents) {
        Ok(req) => req,
        Err(_) => return false,
    };

    if writer
        .send(Message::Text(list_request.into()))
        .await
        .is_err()
    {
        return false;
    }

    while let Some(message) = reader.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let response: DiscoveryResponse = match serde_json::from_str(&text) {
                    Ok(resp) => resp,
                    Err(_) => return false,
                };

                if let DiscoveryResponse::Agents { agents } = response {
                    return agents
                        .iter()
                        .any(|agent| agent.assigned_port == agent_port && agent.is_running);
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(_) => return false,
        }
    }

    false
}

trait TuiApp {
    fn should_quit(&self) -> bool;
    fn on_tick(&mut self);
}

fn run_tui_loop<T>(
    app: &mut T,
    mut draw_fn: impl FnMut(&mut Frame, &mut T),
    mut key_fn: impl FnMut(&mut T, KeyEvent),
) -> anyhow::Result<()>
where
    T: TuiApp,
{
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("failed to create terminal backend")?;

    let result = (|| -> anyhow::Result<()> {
        let mut last_tick = Instant::now();
        loop {
            terminal.draw(|frame| draw_fn(frame, app))?;

            let timeout = TICK_RATE.saturating_sub(last_tick.elapsed());
            if event::poll(timeout)? {
                let input_event = event::read()?;
                if let Event::Key(key) = input_event {
                    if key.kind == KeyEventKind::Press {
                        key_fn(app, key);
                    }
                }
            }

            if last_tick.elapsed() >= TICK_RATE {
                app.on_tick();
                last_tick = Instant::now();
            }

            if app.should_quit() {
                break;
            }
        }
        Ok(())
    })();

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    result
}

struct AgentSelectorApp {
    agents: Vec<AgentEntry>,
    selected_index: usize,
    should_quit: bool,
    confirmed: bool,
    create_new: bool,
}

impl TuiApp for AgentSelectorApp {
    fn should_quit(&self) -> bool {
        self.should_quit
    }

    fn on_tick(&mut self) {}
}

fn handle_agent_selector_key(app: &mut AgentSelectorApp, key: KeyEvent) {
    if matches!(key.code, KeyCode::Esc)
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
    {
        app.should_quit = true;
        return;
    }

    match key.code {
        KeyCode::Up => {
            if app.selected_index > 0 {
                app.selected_index -= 1;
            }
        }
        KeyCode::Down => {
            if app.selected_index + 1 < app.agents.len() {
                app.selected_index += 1;
            }
        }
        KeyCode::Enter => {
            if !app.agents.is_empty() {
                app.confirmed = true;
                app.should_quit = true;
            }
        }
        KeyCode::Char('n') | KeyCode::Char('N') => {
            app.create_new = true;
            app.should_quit = true;
        }
        _ => {}
    }
}

fn draw_agent_selector(frame: &mut Frame, app: &mut AgentSelectorApp) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(17, 21, 28))),
        area,
    );

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(area);

    let title_text = if app.agents.is_empty() {
        "No configured agents. Press N to create a new agent"
    } else {
        "Select an available agent"
    };
    let title = Paragraph::new(title_text).style(
        Style::default()
            .bg(Color::Rgb(34, 41, 52))
            .fg(Color::Rgb(220, 229, 239)),
    );
    frame.render_widget(title, chunks[0]);

    let items: Vec<ListItem> = app
        .agents
        .iter()
        .map(|agent| {
            ListItem::new(Line::from(format!(
                "{} - {} - ws://127.0.0.1:{}",
                agent.workspace_dir,
                if agent.is_running {
                    "running"
                } else {
                    "stopped"
                },
                agent.assigned_port
            )))
        })
        .collect();

    let list = List::new(items)
        .style(
            Style::default()
                .bg(Color::Rgb(25, 30, 39))
                .fg(Color::Rgb(180, 189, 200)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(55, 68, 86))
                .fg(Color::Rgb(245, 249, 255)),
        )
        .highlight_symbol("  > ");

    let mut list_state = ListState::default().with_selected(Some(app.selected_index));
    frame.render_stateful_widget(list, chunks[1], &mut list_state);

    let help = Paragraph::new("Up/Down move, Enter select, N new agent, Esc quit").style(
        Style::default()
            .bg(Color::Rgb(34, 41, 52))
            .fg(Color::Rgb(140, 151, 166)),
    );
    frame.render_widget(help, chunks[2]);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SetupStage {
    Workspace,
    Provider,
    Model,
    ApiKey,
    Confirm,
    Done,
}

struct SetupApp {
    stage: SetupStage,
    should_quit: bool,
    workspace_input: String,
    provider_index: usize,
    model_input: String,
    api_key_input: String,
    confirm_index: usize,
    error_message: Option<String>,
    cursor_visible: bool,
    agent_port: u16,
}

impl TuiApp for SetupApp {
    fn should_quit(&self) -> bool {
        self.should_quit
    }

    fn on_tick(&mut self) {
        self.cursor_visible = !self.cursor_visible;
    }
}

fn handle_setup_key(app: &mut SetupApp, key: KeyEvent) {
    if matches!(key.code, KeyCode::Esc)
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
    {
        app.should_quit = true;
        return;
    }

    app.error_message = None;

    match app.stage {
        SetupStage::Workspace => match key.code {
            KeyCode::Backspace => {
                app.workspace_input.pop();
            }
            KeyCode::Enter => {
                if app.workspace_input.trim().is_empty() {
                    app.error_message = Some("Workspace directory cannot be empty".to_string());
                    return;
                }

                if let Err(error) = std::fs::create_dir_all(app.workspace_input.trim()) {
                    app.error_message =
                        Some(format!("Could not create workspace directory: {error}"));
                    return;
                }

                app.stage = SetupStage::Provider;
            }
            KeyCode::Char(c) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    app.workspace_input.push(c);
                }
            }
            _ => {}
        },
        SetupStage::Provider => match key.code {
            KeyCode::Up => {
                if app.provider_index > 0 {
                    app.provider_index -= 1;
                }
            }
            KeyCode::Down => {
                if app.provider_index + 1 < PROVIDERS.len() {
                    app.provider_index += 1;
                }
            }
            KeyCode::Enter => {
                app.model_input =
                    default_model_for_provider(PROVIDERS[app.provider_index]).to_string();
                app.stage = SetupStage::Model;
            }
            _ => {}
        },
        SetupStage::Model => match key.code {
            KeyCode::Backspace => {
                app.model_input.pop();
            }
            KeyCode::Enter => {
                if app.model_input.trim().is_empty() {
                    app.error_message = Some("Model cannot be empty".to_string());
                    return;
                }
                app.stage = SetupStage::ApiKey;
            }
            KeyCode::Char(c) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    app.model_input.push(c);
                }
            }
            _ => {}
        },
        SetupStage::ApiKey => match key.code {
            KeyCode::Backspace => {
                app.api_key_input.pop();
            }
            KeyCode::Enter => {
                if app.api_key_input.trim().is_empty() {
                    app.error_message = Some("API key cannot be empty".to_string());
                    return;
                }
                app.stage = SetupStage::Confirm;
            }
            KeyCode::Char(c) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    app.api_key_input.push(c);
                }
            }
            _ => {}
        },
        SetupStage::Confirm => match key.code {
            KeyCode::Up => {
                if app.confirm_index > 0 {
                    app.confirm_index -= 1;
                }
            }
            KeyCode::Down => {
                if app.confirm_index < 1 {
                    app.confirm_index += 1;
                }
            }
            KeyCode::Enter => {
                if app.confirm_index == 0 {
                    app.stage = SetupStage::Done;
                    app.should_quit = true;
                } else {
                    app.stage = SetupStage::Model;
                }
            }
            _ => {}
        },
        SetupStage::Done => {}
    }
}

fn draw_setup(frame: &mut Frame, app: &mut SetupApp) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(18, 20, 25))),
        area,
    );

    match app.stage {
        SetupStage::Workspace => draw_workspace_step(frame, app),
        SetupStage::Provider => draw_provider_step(frame, app),
        SetupStage::Model => draw_model_step(frame, app),
        SetupStage::ApiKey => draw_api_step(frame, app),
        SetupStage::Confirm => draw_confirm_step(frame, app),
        SetupStage::Done => {}
    }
}

fn draw_model_step(frame: &mut Frame, app: &SetupApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(2),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(format!(
            "Agent Setup - Model ({})",
            PROVIDERS[app.provider_index]
        ))
        .style(Style::default().fg(Color::Rgb(227, 237, 255))),
        chunks[0],
    );

    let cursor = if app.cursor_visible { "_" } else { " " };
    frame.render_widget(
        Paragraph::new(format!("{}{cursor}", app.model_input))
            .style(Style::default().fg(Color::Rgb(210, 218, 231))),
        chunks[1],
    );

    frame.render_widget(
        Paragraph::new("Enter model ID for selected provider, then press Enter")
            .style(Style::default().fg(Color::Rgb(150, 160, 174))),
        chunks[2],
    );

    if let Some(error) = &app.error_message {
        frame.render_widget(
            Paragraph::new(error.clone()).style(Style::default().fg(Color::Rgb(255, 120, 120))),
            chunks[3],
        );
    }
}

fn draw_workspace_step(frame: &mut Frame, app: &SetupApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(2),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new("Agent Setup - Workspace Directory")
            .style(Style::default().fg(Color::Rgb(227, 237, 255))),
        chunks[0],
    );

    let cursor = if app.cursor_visible { "_" } else { " " };
    frame.render_widget(
        Paragraph::new(format!("{}{cursor}", app.workspace_input))
            .style(Style::default().fg(Color::Rgb(210, 218, 231))),
        chunks[1],
    );

    frame.render_widget(
        Paragraph::new("Enter a workspace path. It will be created if missing.")
            .style(Style::default().fg(Color::Rgb(150, 160, 174))),
        chunks[2],
    );

    if let Some(error) = &app.error_message {
        frame.render_widget(
            Paragraph::new(error.clone()).style(Style::default().fg(Color::Rgb(255, 120, 120))),
            chunks[3],
        );
    }
}

fn draw_provider_step(frame: &mut Frame, app: &SetupApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new("Agent Setup - Select Model Provider")
            .style(Style::default().fg(Color::Rgb(227, 237, 255))),
        chunks[0],
    );

    let items: Vec<ListItem> = PROVIDERS
        .iter()
        .map(|provider| ListItem::new(Line::from((*provider).to_string())))
        .collect();

    let list = List::new(items)
        .style(Style::default().fg(Color::Rgb(188, 202, 220)))
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(56, 74, 96))
                .fg(Color::Rgb(243, 248, 255)),
        )
        .highlight_symbol("  > ");

    let mut state = ListState::default().with_selected(Some(app.provider_index));
    frame.render_stateful_widget(list, chunks[1], &mut state);

    frame.render_widget(
        Paragraph::new("Up/Down to choose provider, Enter to continue")
            .style(Style::default().fg(Color::Rgb(150, 160, 174))),
        chunks[2],
    );
}

fn draw_api_step(frame: &mut Frame, app: &SetupApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(2),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(format!(
            "Agent Setup - API Key ({})",
            PROVIDERS[app.provider_index]
        ))
        .style(Style::default().fg(Color::Rgb(227, 237, 255))),
        chunks[0],
    );

    let masked = "*".repeat(app.api_key_input.chars().count());
    let cursor = if app.cursor_visible { "_" } else { " " };
    frame.render_widget(
        Paragraph::new(format!("{masked}{cursor}"))
            .style(Style::default().fg(Color::Rgb(210, 218, 231))),
        chunks[1],
    );

    frame.render_widget(
        Paragraph::new("Enter the API key for the selected provider, then press Enter")
            .style(Style::default().fg(Color::Rgb(150, 160, 174))),
        chunks[2],
    );

    if let Some(error) = &app.error_message {
        frame.render_widget(
            Paragraph::new(error.clone()).style(Style::default().fg(Color::Rgb(255, 120, 120))),
            chunks[3],
        );
    }
}

fn draw_confirm_step(frame: &mut Frame, app: &SetupApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(frame.area());

    let summary = format!(
        "Confirm setup values:\n- workspace: {}\n- port: {}\n- provider: {}\n- model: {}",
        app.workspace_input.trim(),
        app.agent_port,
        PROVIDERS[app.provider_index],
        app.model_input.trim()
    );
    frame.render_widget(
        Paragraph::new(summary)
            .style(Style::default().fg(Color::Rgb(225, 235, 250)))
            .wrap(Wrap { trim: false }),
        chunks[0],
    );

    let options = vec![
        ListItem::new(Line::from("Confirm and save")),
        ListItem::new(Line::from("Back to model")),
    ];
    let list = List::new(options)
        .style(Style::default().fg(Color::Rgb(188, 202, 220)))
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(56, 74, 96))
                .fg(Color::Rgb(243, 248, 255)),
        )
        .highlight_symbol("  > ");
    let mut state = ListState::default().with_selected(Some(app.confirm_index));
    frame.render_stateful_widget(list, chunks[1], &mut state);

    frame.render_widget(
        Paragraph::new("Up/Down to choose, Enter to continue")
            .style(Style::default().fg(Color::Rgb(150, 160, 174))),
        chunks[2],
    );
}

struct ChatApp {
    should_quit: bool,
    input: String,
    input_view_backscroll: usize,
    cursor_visible: bool,
    messages: Vec<ChatMessage>,
    scroll_offset: usize,
    history_max_scroll: usize,
    follow_tail: bool,
    status: ChatStatus,
    status_ticks: u8,
    ws_status: WebSocketStatus,
    connection_state: Arc<AtomicBool>,
    agent_name: Option<String>,
    agent_workspace: Option<String>,
    project_workspace: Option<String>,
    chat_cmd_tx: UnboundedSender<ChatCommand>,
    chat_event_rx: UnboundedReceiver<ChatEvent>,
    session_id: Option<String>,
    next_turn_id: u64,
    active_provider: String,
    active_model: String,
    agent_port: u16,
    planned_actions: Vec<PlannedAction>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MessageRole {
    User,
    Assistant,
    System,
}

struct ChatMessage {
    role: MessageRole,
    text: String,
}

#[derive(Clone, Copy)]
enum WebSocketStatus {
    Connected,
    Disconnected,
}

impl WebSocketStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Connected => "Connected",
            Self::Disconnected => "Disconnected",
        }
    }

    fn bg_color(self) -> Color {
        match self {
            Self::Connected => Color::Rgb(48, 135, 83),
            Self::Disconnected => Color::Rgb(154, 51, 51),
        }
    }
}

#[derive(Clone, Copy)]
enum ChatStatus {
    Idle,
    Thinking,
}

impl ChatStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle...",
            Self::Thinking => "Thinking...",
        }
    }
}

impl TuiApp for ChatApp {
    fn should_quit(&self) -> bool {
        self.should_quit
    }

    fn on_tick(&mut self) {
        self.cursor_visible = !self.cursor_visible;
        self.ws_status = if self.connection_state.load(Ordering::Relaxed) {
            WebSocketStatus::Connected
        } else {
            WebSocketStatus::Disconnected
        };

        while let Ok(event) = self.chat_event_rx.try_recv() {
            match event {
                ChatEvent::SessionStarted {
                    session_id,
                    provider,
                    model,
                } => {
                    self.session_id = Some(session_id.clone());
                    self.active_provider = provider;
                    self.active_model = model;
                    self.planned_actions.clear();
                    self.messages.push(ChatMessage {
                        role: MessageRole::System,
                        text: format!(
                            "Connected to {session_id} on ws://{AGENT_HOST}:{}",
                            self.agent_port
                        ),
                    });
                    self.status = ChatStatus::Idle;
                    self.status_ticks = 0;
                }
                ChatEvent::EffectApplied { effect } => {
                    match effect {
                        Effect::ChatResponseDelta { text_delta, .. } => {
                            match self.messages.last_mut() {
                                Some(last) if last.role == MessageRole::Assistant => {
                                    last.text.push_str(&text_delta);
                                }
                                _ => {
                                    self.messages.push(ChatMessage {
                                        role: MessageRole::Assistant,
                                        text: text_delta,
                                    });
                                }
                            }
                        }
                        Effect::ChatResponse { text, .. } => match self.messages.last_mut() {
                            Some(last)
                                if last.role == MessageRole::Assistant && !last.text.is_empty() =>
                            {
                                last.text = text;
                            }
                            _ => self.messages.push(ChatMessage {
                                role: MessageRole::Assistant,
                                text,
                            }),
                        },
                        Effect::TaskCompletion {
                            status, details, ..
                        } => {
                            self.messages.push(ChatMessage {
                                role: MessageRole::System,
                                text: format!("Task Completion: {status} - {details}"),
                            });
                        }
                        Effect::PlanUpdated { actions, .. } => {
                            self.planned_actions = actions;
                        }
                        Effect::ActionStatusChanged { action, .. } => {
                            if let Some(existing) = self
                                .planned_actions
                                .iter_mut()
                                .find(|existing| existing.action_id == action.action_id)
                            {
                                *existing = action;
                            } else {
                                self.planned_actions.push(action);
                            }
                        }
                    }
                    self.status = ChatStatus::Idle;
                    self.status_ticks = 0;
                }
                ChatEvent::Error { message } => {
                    self.messages.push(ChatMessage {
                        role: MessageRole::System,
                        text: format!("Error: {message}"),
                    });
                    self.status = ChatStatus::Idle;
                    self.status_ticks = 0;
                }
                ChatEvent::PluginCommandResult { success, message } => {
                    self.messages.push(ChatMessage {
                        role: MessageRole::System,
                        text: if success {
                            format!("Plugin: {message}")
                        } else {
                            format!("Plugin Error: {message}")
                        },
                    });
                    self.status = ChatStatus::Idle;
                    self.status_ticks = 0;
                }
                ChatEvent::Disconnected => {
                    self.messages.push(ChatMessage {
                        role: MessageRole::System,
                        text: "Connection closed.".to_string(),
                    });
                    self.status = ChatStatus::Idle;
                    self.status_ticks = 0;
                }
            }
        }
    }
}

fn handle_chat_key(app: &mut ChatApp, key: KeyEvent) {
    if matches!(key.code, KeyCode::Esc)
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
    {
        app.ws_status = WebSocketStatus::Disconnected;
        app.should_quit = true;
        return;
    }

    match key.code {
        KeyCode::Backspace => {
            app.input.pop();
            app.input_view_backscroll = 0;
        }
        KeyCode::Enter => {
            if app.input.trim().is_empty() {
                return;
            }
            let text = app.input.trim().to_string();

            if let Some(command) = parse_plugin_command(&text) {
                app.messages.push(ChatMessage {
                    role: MessageRole::User,
                    text: text.clone(),
                });
                match command {
                    Ok(command) => {
                        if app
                            .chat_cmd_tx
                            .send(ChatCommand::PluginCommand { command })
                            .is_err()
                        {
                            app.messages.push(ChatMessage {
                                role: MessageRole::System,
                                text: "Error: Unable to send plugin command to agent.".to_string(),
                            });
                        } else {
                            app.status = ChatStatus::Thinking;
                            app.status_ticks = 0;
                        }
                    }
                    Err(error_message) => {
                        app.messages.push(ChatMessage {
                            role: MessageRole::System,
                            text: error_message,
                        });
                        app.status = ChatStatus::Idle;
                        app.status_ticks = 0;
                    }
                }
                app.input.clear();
                app.input_view_backscroll = 0;
                app.follow_tail = true;
                return;
            }

            let turn_id = format!("turn-{}", app.next_turn_id);
            app.next_turn_id = app.next_turn_id.saturating_add(1);
            app.messages.push(ChatMessage {
                role: MessageRole::User,
                text: text.clone(),
            });
            if app
                .chat_cmd_tx
                .send(ChatCommand::SendPercept { turn_id, text })
                .is_err()
            {
                app.messages.push(ChatMessage {
                    role: MessageRole::System,
                    text: "Error: Unable to send percept to agent.".to_string(),
                });
            }
            app.input.clear();
            app.input_view_backscroll = 0;
            app.follow_tail = true;
            app.status = ChatStatus::Thinking;
            app.status_ticks = 0;
        }
        KeyCode::Up => {
            if has_history_scroll_modifier(key.modifiers) {
                app.follow_tail = false;
                app.scroll_offset = app.scroll_offset.saturating_sub(1);
            } else {
                app.input_view_backscroll = app.input_view_backscroll.saturating_add(1);
            }
        }
        KeyCode::Down => {
            if has_history_scroll_modifier(key.modifiers) {
                app.follow_tail = false;
                app.scroll_offset = app
                    .scroll_offset
                    .saturating_add(1)
                    .min(app.history_max_scroll);
                if app.scroll_offset >= app.history_max_scroll {
                    app.follow_tail = true;
                }
            } else {
                app.input_view_backscroll = app.input_view_backscroll.saturating_sub(1);
            }
        }
        KeyCode::End => {
            app.follow_tail = true;
            app.scroll_offset = app.history_max_scroll;
        }
        KeyCode::Char(c) => {
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
            {
                app.input.push(c);
                app.input_view_backscroll = 0;
            }
        }
        _ => {}
    }
}

fn has_history_scroll_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) || modifiers.contains(KeyModifiers::SUPER)
}

fn parse_plugin_command(input: &str) -> Option<Result<PluginCommandRequest, String>> {
    let trimmed = input.trim();
    if !trimmed.starts_with("/plugin") {
        return None;
    }

    let tokens = trimmed.split_whitespace().collect::<Vec<_>>();
    if tokens.len() < 2 {
        return Some(Err(
            "Usage: /plugin <add|remove|enable|disable|list|catalog> [arg]".to_string(),
        ));
    }

    let result = match tokens[1] {
        "add" => {
            if tokens.len() < 3 {
                Err("Usage: /plugin add <directory_path>".to_string())
            } else {
                let source = trimmed
                    .splitn(3, ' ')
                    .nth(2)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if source.is_empty() {
                    Err("Usage: /plugin add <directory_path>".to_string())
                } else {
                    Ok(PluginCommandRequest::Add { source })
                }
            }
        }
        "remove" => {
            if tokens.len() < 3 {
                Err("Usage: /plugin remove <plugin_name>".to_string())
            } else {
                Ok(PluginCommandRequest::Remove {
                    plugin_name: tokens[2].to_string(),
                })
            }
        }
        "enable" => {
            if tokens.len() < 3 {
                Err("Usage: /plugin enable <plugin_name>".to_string())
            } else {
                Ok(PluginCommandRequest::Enable {
                    plugin_name: tokens[2].to_string(),
                })
            }
        }
        "disable" => {
            if tokens.len() < 3 {
                Err("Usage: /plugin disable <plugin_name>".to_string())
            } else {
                Ok(PluginCommandRequest::Disable {
                    plugin_name: tokens[2].to_string(),
                })
            }
        }
        "list" => Ok(PluginCommandRequest::List),
        "catalog" => Ok(PluginCommandRequest::Catalog),
        _ => Err("Usage: /plugin <add|remove|enable|disable|list|catalog> [arg]".to_string()),
    };

    Some(result)
}

fn draw_chat(frame: &mut Frame, app: &mut ChatApp) {
    let area = frame.area();
    if should_render_sidenav(area) {
        let split = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(75), Constraint::Percentage(25)])
            .split(area);

        draw_chat_panel(frame, split[0], app);
        draw_sidenav(
            frame,
            split[1],
            app.ws_status,
            app.agent_name.as_deref(),
            app.agent_port,
            app.agent_workspace.as_deref(),
            app.project_workspace.as_deref(),
            &app.planned_actions,
        );
    } else {
        draw_chat_panel(frame, area, app);
    }
}

fn should_render_sidenav(area: Rect) -> bool {
    match crossterm::terminal::window_size() {
        Ok(size) => size.width >= 800,
        Err(_) => area.width >= 120,
    }
}

fn draw_chat_panel(frame: &mut Frame, area: Rect, app: &mut ChatApp) {
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(23, 29, 37))),
        area,
    );

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(5),
            Constraint::Length(1),
        ])
        .split(area);

    let history_bg = Block::default().style(
        Style::default()
            .bg(Color::Rgb(28, 35, 45))
            .fg(Color::Rgb(219, 227, 238)),
    );
    frame.render_widget(history_bg, rows[0]);

    let history_area = Rect {
        x: rows[0].x.saturating_add(1),
        y: rows[0].y,
        width: rows[0].width.saturating_sub(2),
        height: rows[0].height,
    };

    let history_lines = build_history_lines(&app.messages, history_area.width as usize);

    let content_length = history_lines.len().max(1);
    let viewport_height = history_area.height as usize;
    let max_scroll = content_length.saturating_sub(viewport_height);
    app.history_max_scroll = max_scroll;

    if app.follow_tail {
        app.scroll_offset = max_scroll;
    } else {
        app.scroll_offset = app.scroll_offset.min(max_scroll);
    }

    let history = Paragraph::new(Text::from(history_lines))
        .style(
            Style::default()
                .bg(Color::Rgb(28, 35, 45))
                .fg(Color::Rgb(219, 227, 238)),
        )
        .scroll((app.scroll_offset as u16, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(history, history_area);

    let mut scrollbar_state = ScrollbarState::new(content_length)
        .position(app.scroll_offset)
        .viewport_content_length(viewport_height);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .thumb_style(Style::default().fg(Color::Rgb(140, 170, 200)))
        .track_style(Style::default().fg(Color::Rgb(70, 80, 92)));
    frame.render_stateful_widget(scrollbar, history_area, &mut scrollbar_state);

    let cursor = if app.cursor_visible { "█" } else { " " };

    let input_label_outer = Block::default().style(
        Style::default()
            .bg(Color::Rgb(28, 35, 45))
            .fg(Color::Rgb(219, 227, 238)),
    );
    frame.render_widget(input_label_outer, rows[1]);

    let input_label_container = Rect {
        x: rows[1].x.saturating_add(1),
        y: rows[1].y,
        width: rows[1].width.saturating_sub(2),
        height: rows[1].height,
    };
    let input_label_border_area = Rect {
        x: input_label_container.x,
        y: input_label_container.y,
        width: 1,
        height: input_label_container.height,
    };
    let input_label_border = std::iter::repeat("▌")
        .take(input_label_container.height as usize)
        .collect::<Vec<_>>()
        .join("\n");
    let input_label_border_widget = Paragraph::new(input_label_border).style(
        Style::default()
            .bg(Color::Rgb(43, 54, 69))
            .fg(Color::Rgb(255, 240, 140)),
    );
    frame.render_widget(input_label_border_widget, input_label_border_area);

    let input_label_margin = Rect {
        x: input_label_container.x.saturating_add(1),
        y: input_label_container.y,
        width: input_label_container.width.saturating_sub(1),
        height: input_label_container.height,
    };
    let input_label = Paragraph::new("").style(
        Style::default()
            .bg(Color::Rgb(43, 54, 69))
            .fg(Color::Rgb(144, 163, 183)),
    );
    frame.render_widget(input_label, input_label_margin);

    let input_outer = Block::default().style(
        Style::default()
            .bg(Color::Rgb(28, 35, 45))
            .fg(Color::Rgb(219, 227, 238)),
    );
    frame.render_widget(input_outer, rows[2]);

    let input_container = Rect {
        x: rows[2].x.saturating_add(1),
        y: rows[2].y,
        width: rows[2].width.saturating_sub(2),
        height: rows[2].height,
    };
    let input_bg = Block::default().style(
        Style::default()
            .bg(Color::Rgb(43, 54, 69))
            .fg(Color::Rgb(242, 248, 255)),
    );
    frame.render_widget(input_bg, input_container);

    let input_border_area = Rect {
        x: input_container.x,
        y: input_container.y,
        width: 1,
        height: input_container.height,
    };
    let input_border = std::iter::repeat("▌")
        .take(input_container.height as usize)
        .collect::<Vec<_>>()
        .join("\n");
    let input_border_widget = Paragraph::new(input_border).style(
        Style::default()
            .bg(Color::Rgb(43, 54, 69))
            .fg(Color::Rgb(255, 240, 140)),
    );
    frame.render_widget(input_border_widget, input_border_area);

    let input_text_area = Rect {
        x: input_container.x.saturating_add(2),
        y: input_container.y,
        width: input_container.width.saturating_sub(3),
        height: 2,
    };

    let input_with_cursor = format!("{}{cursor}", app.input);
    let wrapped_input = wrap_text(&input_with_cursor, input_text_area.width.max(1) as usize);
    let max_input_scroll = wrapped_input
        .len()
        .saturating_sub(input_text_area.height as usize);
    let input_scroll = max_input_scroll.saturating_sub(app.input_view_backscroll);
    let input_lines: Vec<Line> = wrapped_input
        .into_iter()
        .skip(input_scroll)
        .take(input_text_area.height as usize)
        .map(Line::from)
        .collect();

    let input = Paragraph::new(Text::from(input_lines)).style(
        Style::default()
            .bg(Color::Rgb(43, 54, 69))
            .fg(Color::Rgb(242, 248, 255)),
    );
    frame.render_widget(input, input_text_area);

    let provider_area = Rect {
        x: input_container.x.saturating_add(2),
        y: input_container
            .y
            .saturating_add(input_container.height.saturating_sub(2)),
        width: input_container.width.saturating_sub(3),
        height: 1,
    };
    let provider_line = Line::from(vec![
        Span::styled(
            app.active_provider.clone(),
            Style::default().fg(Color::Rgb(255, 240, 140)),
        ),
        Span::raw(" "),
        Span::styled(
            app.active_model.clone(),
            Style::default().fg(Color::Rgb(144, 163, 183)),
        ),
    ]);
    let provider = Paragraph::new(provider_line).style(Style::default().bg(Color::Rgb(43, 54, 69)));
    frame.render_widget(provider, provider_area);

    let send_label = " Send ";
    let send_width = send_label.chars().count() as u16;
    let send_area = Rect {
        x: provider_area
            .x
            .saturating_add(provider_area.width.saturating_sub(send_width)),
        y: provider_area.y,
        width: send_width.min(provider_area.width),
        height: 1,
    };
    let send_bg = if app.input.trim().is_empty() {
        Color::Rgb(96, 106, 120)
    } else {
        Color::Rgb(255, 240, 140)
    };
    let send =
        Paragraph::new(send_label).style(Style::default().bg(send_bg).fg(Color::Rgb(16, 22, 31)));
    frame.render_widget(send, send_area);

    let status = Paragraph::new(format!(" {}", app.status.label())).style(
        Style::default()
            .bg(Color::Rgb(23, 29, 37))
            .fg(Color::Rgb(144, 163, 183)),
    );
    frame.render_widget(status, rows[3]);
}

fn build_history_lines(messages: &[ChatMessage], width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::from("")];
    }

    if width <= 2 {
        let mut compact = vec![Line::from("")];
        for message in messages {
            compact.push(Line::from(message.text.clone()));
            compact.push(Line::from(""));
        }
        return compact;
    }

    let mut lines = Vec::new();
    lines.push(Line::from(""));

    let bubble_outer_width = width.saturating_sub(2);
    let bubble_inner_width = bubble_outer_width.saturating_sub(2).max(1);

    for message in messages {
        let is_user = message.role == MessageRole::User;
        if is_user {
            lines.push(Line::from(format!(" {}", message.text)));
            lines.push(Line::from(""));
            continue;
        }

        let bubble_style = Style::default()
            .bg(Color::Rgb(20, 24, 31))
            .fg(Color::Rgb(224, 233, 245));
        let wrapped = format_agent_markdown(&message.text, bubble_inner_width);

        lines.push(Line::from(Span::styled(
            " ".repeat(bubble_outer_width),
            bubble_style,
        )));
        for part in wrapped {
            let content_width = part
                .iter()
                .map(|segment| segment.text.chars().count())
                .sum::<usize>();
            let right_padding = bubble_inner_width
                .saturating_sub(content_width)
                .saturating_add(1);

            let mut spans = Vec::with_capacity(part.len().saturating_add(2));
            spans.push(Span::styled(" ", bubble_style));
            for segment in part {
                spans.push(Span::styled(
                    segment.text,
                    bubble_style.patch(segment.style),
                ));
            }
            spans.push(Span::styled(" ".repeat(right_padding), bubble_style));

            lines.push(Line::from(spans));
        }
        lines.push(Line::from(Span::styled(
            " ".repeat(bubble_outer_width),
            bubble_style,
        )));
        lines.push(Line::from(""));
    }

    if lines.is_empty() {
        lines.push(Line::from(""));
    }

    lines
}

#[derive(Clone)]
struct StyledSegment {
    text: String,
    style: Style,
}

fn format_agent_markdown(input: &str, max_width: usize) -> Vec<Vec<StyledSegment>> {
    if max_width == 0 {
        return vec![Vec::new()];
    }

    let mut lines = Vec::new();
    for raw_line in input.lines() {
        let styled = parse_markdown_line(raw_line);
        let mut wrapped = wrap_styled_segments(&styled, max_width);
        lines.append(&mut wrapped);
    }

    if input.ends_with('\n') {
        lines.push(Vec::new());
    }

    if lines.is_empty() {
        lines.push(Vec::new());
    }

    lines
}

fn parse_markdown_line(raw_line: &str) -> Vec<StyledSegment> {
    let heading = parse_markdown_heading(raw_line);

    let mut output = Vec::new();
    if !heading.prefix.is_empty() {
        output.push(StyledSegment {
            text: heading.prefix,
            style: Style::default(),
        });
    }

    if !heading.ordered_prefix.is_empty() {
        output.push(StyledSegment {
            text: heading.ordered_prefix,
            style: Style::default().add_modifier(Modifier::BOLD),
        });
    }

    let mut inline = parse_inline_markdown(heading.remaining.as_str());
    if heading.bold_line {
        for segment in &mut inline {
            segment.style = segment.style.add_modifier(Modifier::BOLD);
        }
    }
    output.append(&mut inline);

    output
}

struct ParsedLine {
    prefix: String,
    ordered_prefix: String,
    remaining: String,
    bold_line: bool,
}

fn parse_markdown_heading(raw_line: &str) -> ParsedLine {
    let mut chars = raw_line.chars();
    let mut prefix = String::new();
    while let Some(ch) = chars.next() {
        if ch == ' ' || ch == '\t' {
            prefix.push(ch);
        } else {
            let mut remaining_chars = String::new();
            remaining_chars.push(ch);
            remaining_chars.extend(chars);

            let (content, bold_line) = strip_heading_marker(&remaining_chars);
            let (ordered_prefix, remaining) = split_ordered_prefix(&content);
            return ParsedLine {
                prefix,
                ordered_prefix,
                remaining,
                bold_line,
            };
        }
    }

    ParsedLine {
        prefix,
        ordered_prefix: String::new(),
        remaining: String::new(),
        bold_line: false,
    }
}

fn strip_heading_marker(line: &str) -> (String, bool) {
    let mut marker_len = 0;
    for ch in line.chars() {
        if ch == '#' {
            marker_len += 1;
        } else {
            break;
        }
    }

    if marker_len == 0 || marker_len > 6 {
        return (line.to_string(), false);
    }

    let after_markers = line.chars().skip(marker_len).collect::<String>();
    if !after_markers.starts_with(' ') {
        return (line.to_string(), false);
    }

    (after_markers.trim_start().to_string(), true)
}

fn split_ordered_prefix(line: &str) -> (String, String) {
    let mut digits = String::new();
    let mut consumed = 0usize;

    for ch in line.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            consumed += ch.len_utf8();
            continue;
        }
        break;
    }

    if digits.is_empty() {
        return (String::new(), line.to_string());
    }

    let rest = &line[consumed..];
    if let Some(stripped) = rest.strip_prefix(". ") {
        return (format!("{digits}."), format!(" {stripped}"));
    }

    (String::new(), line.to_string())
}

fn parse_inline_markdown(text: &str) -> Vec<StyledSegment> {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    let mut bold = false;
    let mut italic = false;
    let mut buffer = String::new();
    let mut segments = Vec::new();

    while i < chars.len() {
        if let Some((display, url, consumed)) = parse_markdown_link(&chars[i..]) {
            push_buffered_text(&mut segments, &mut buffer, bold, italic);
            let link_style = style_for_state(bold, italic)
                .fg(Color::Blue)
                .add_modifier(Modifier::UNDERLINED);
            let append_url = !url.is_empty() && url != display;
            segments.push(StyledSegment {
                text: display,
                style: link_style,
            });
            if append_url {
                segments.push(StyledSegment {
                    text: format!(" ({url})"),
                    style: link_style,
                });
            }
            i += consumed;
            continue;
        }

        if let Some((url, consumed)) = parse_plain_url(&chars[i..]) {
            push_buffered_text(&mut segments, &mut buffer, bold, italic);
            segments.push(StyledSegment {
                text: url,
                style: style_for_state(bold, italic)
                    .fg(Color::Blue)
                    .add_modifier(Modifier::UNDERLINED),
            });
            i += consumed;
            continue;
        }

        if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            push_buffered_text(&mut segments, &mut buffer, bold, italic);
            bold = !bold;
            i += 2;
            continue;
        }
        if chars[i] == '_' && i + 1 < chars.len() && chars[i + 1] == '_' {
            push_buffered_text(&mut segments, &mut buffer, bold, italic);
            bold = !bold;
            i += 2;
            continue;
        }
        if chars[i] == '*' || chars[i] == '_' {
            push_buffered_text(&mut segments, &mut buffer, bold, italic);
            italic = !italic;
            i += 1;
            continue;
        }

        buffer.push(chars[i]);
        i += 1;
    }

    push_buffered_text(&mut segments, &mut buffer, bold, italic);
    segments
}

fn parse_markdown_link(chars: &[char]) -> Option<(String, String, usize)> {
    if chars.first() != Some(&'[') {
        return None;
    }

    let close_label = chars.iter().position(|ch| *ch == ']')?;
    if chars.get(close_label + 1) != Some(&'(') {
        return None;
    }

    let url_start = close_label + 2;
    let close_url = chars[url_start..].iter().position(|ch| *ch == ')')? + url_start;
    let label = chars[1..close_label].iter().collect::<String>();
    let url = chars[url_start..close_url].iter().collect::<String>();

    Some((label, url, close_url + 1))
}

fn parse_plain_url(chars: &[char]) -> Option<(String, usize)> {
    let starts_http = chars.starts_with(&['h', 't', 't', 'p', ':', '/', '/']);
    let starts_https = chars.starts_with(&['h', 't', 't', 'p', 's', ':', '/', '/']);
    if !starts_http && !starts_https {
        return None;
    }

    let mut consumed = 0usize;
    let mut url = String::new();
    for ch in chars {
        if ch.is_whitespace() {
            break;
        }
        if *ch == ')' || *ch == ']' {
            break;
        }
        consumed += 1;
        url.push(*ch);
    }

    if url.is_empty() {
        None
    } else {
        Some((url, consumed))
    }
}

fn push_buffered_text(
    segments: &mut Vec<StyledSegment>,
    buffer: &mut String,
    bold: bool,
    italic: bool,
) {
    if buffer.is_empty() {
        return;
    }

    segments.push(StyledSegment {
        text: std::mem::take(buffer),
        style: style_for_state(bold, italic),
    });
}

fn style_for_state(bold: bool, italic: bool) -> Style {
    let mut style = Style::default();
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    style
}

fn wrap_styled_segments(segments: &[StyledSegment], max_width: usize) -> Vec<Vec<StyledSegment>> {
    if max_width == 0 {
        return vec![Vec::new()];
    }

    if segments.is_empty() {
        return vec![Vec::new()];
    }

    let mut wrapped: Vec<Vec<StyledSegment>> = vec![Vec::new()];
    let mut current_width = 0usize;

    for segment in segments {
        for ch in segment.text.chars() {
            if current_width >= max_width {
                wrapped.push(Vec::new());
                current_width = 0;
            }

            if let Some(last) = wrapped.last_mut().and_then(|line| line.last_mut()) {
                if last.style == segment.style {
                    last.text.push(ch);
                } else if let Some(line) = wrapped.last_mut() {
                    line.push(StyledSegment {
                        text: ch.to_string(),
                        style: segment.style,
                    });
                }
            } else if let Some(line) = wrapped.last_mut() {
                line.push(StyledSegment {
                    text: ch.to_string(),
                    style: segment.style,
                });
            }

            current_width += 1;
        }
    }

    wrapped
}

fn wrap_text(input: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![String::new()];
    }

    let mut result = Vec::new();
    for raw_line in input.lines() {
        let mut current = String::new();
        for word in raw_line.split_whitespace() {
            if current.is_empty() {
                if word.chars().count() <= max_width {
                    current.push_str(word);
                } else {
                    let mut chunk = String::new();
                    for ch in word.chars() {
                        chunk.push(ch);
                        if chunk.chars().count() == max_width {
                            result.push(chunk.clone());
                            chunk.clear();
                        }
                    }
                    if !chunk.is_empty() {
                        current.push_str(&chunk);
                    }
                }
            } else if current.chars().count() + 1 + word.chars().count() <= max_width {
                current.push(' ');
                current.push_str(word);
            } else {
                result.push(current.clone());
                current.clear();
                if word.chars().count() <= max_width {
                    current.push_str(word);
                } else {
                    let mut chunk = String::new();
                    for ch in word.chars() {
                        chunk.push(ch);
                        if chunk.chars().count() == max_width {
                            result.push(chunk.clone());
                            chunk.clear();
                        }
                    }
                    if !chunk.is_empty() {
                        current.push_str(&chunk);
                    }
                }
            }
        }

        if !current.is_empty() {
            result.push(current);
        }
        if raw_line.is_empty() {
            result.push(String::new());
        }
    }

    if result.is_empty() {
        result.push(String::new());
    }

    result
}

fn draw_sidenav(
    frame: &mut Frame,
    area: Rect,
    ws_status: WebSocketStatus,
    agent_name: Option<&str>,
    agent_port: u16,
    agent_workspace: Option<&str>,
    project_workspace: Option<&str>,
    planned_actions: &[PlannedAction],
) {
    let sidenav_bg = Color::Rgb(16, 19, 25);
    frame.render_widget(
        Block::default().style(Style::default().bg(sidenav_bg)),
        area,
    );

    let display_name = agent_name.unwrap_or("(unnamed)");
    let name_area = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(1),
        width: area.width.saturating_sub(2),
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(display_name).style(Style::default().fg(Color::Rgb(220, 229, 239))),
        name_area,
    );

    let tip_area = Rect {
        x: area.x.saturating_add(1),
        y: area.y.saturating_add(2),
        width: area.width.saturating_sub(2),
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(format!("ws://{AGENT_HOST}:{agent_port}"))
            .style(Style::default().fg(Color::Rgb(144, 163, 183))),
        tip_area,
    );

    let todos_top = area.y.saturating_add(4);
    let todos_bottom_exclusive = area.y.saturating_add(area.height.saturating_sub(7));
    let todos_height = todos_bottom_exclusive.saturating_sub(todos_top);
    if todos_height > 0 {
        let todos_area = Rect {
            x: area.x.saturating_add(1),
            y: todos_top,
            width: area.width.saturating_sub(2),
            height: todos_height,
        };

        let todos_content = build_planning_text(planned_actions);

        let todos_container = Paragraph::new(todos_content)
            .block(
                Block::default()
                    .style(Style::default().bg(Color::Rgb(24, 29, 37)))
                    .padding(Padding::new(1, 1, 1, 1)),
            )
            .style(
                Style::default()
                    .bg(Color::Rgb(24, 29, 37))
                    .fg(Color::Rgb(220, 229, 239)),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(todos_container, todos_area);
    }

    let label = format!(" {} ", ws_status.label());
    let label_width = label.chars().count() as u16;
    let right_margin = 1;
    let top_margin = 1;

    let badge_x = area.x.saturating_add(
        area.width
            .saturating_sub(label_width.saturating_add(right_margin)),
    );
    let badge_y = area.y.saturating_add(top_margin);
    let badge_area = Rect {
        x: badge_x,
        y: badge_y,
        width: label_width.min(area.width),
        height: 1,
    };

    let badge =
        Paragraph::new(label).style(Style::default().bg(ws_status.bg_color()).fg(sidenav_bg));
    frame.render_widget(badge, badge_area);

    if area.height >= 7 {
        let workspace_top = area.y.saturating_add(area.height.saturating_sub(7));
        let workspace_label_area = Rect {
            x: area.x.saturating_add(1),
            y: workspace_top.saturating_add(1),
            width: area.width.saturating_sub(2),
            height: 1,
        };
        frame.render_widget(
            Paragraph::new("Agent Workspace").style(Style::default().fg(Color::Rgb(220, 229, 239))),
            workspace_label_area,
        );

        let workspace_value = format_workspace_path(agent_workspace);
        let workspace_path_area = Rect {
            x: area.x.saturating_add(1),
            y: workspace_top.saturating_add(2),
            width: area.width.saturating_sub(2),
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(workspace_value)
                .style(Style::default().fg(Color::Rgb(144, 163, 183)))
                .wrap(Wrap { trim: false }),
            workspace_path_area,
        );

        let project_label_area = Rect {
            x: area.x.saturating_add(1),
            y: workspace_top.saturating_add(4),
            width: area.width.saturating_sub(2),
            height: 1,
        };
        frame.render_widget(
            Paragraph::new("Project Workspace")
                .style(Style::default().fg(Color::Rgb(220, 229, 239))),
            project_label_area,
        );

        let project_workspace_value = format_workspace_path(project_workspace);
        let project_path_area = Rect {
            x: area.x.saturating_add(1),
            y: workspace_top.saturating_add(5),
            width: area.width.saturating_sub(2),
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(project_workspace_value)
                .style(Style::default().fg(Color::Rgb(144, 163, 183)))
                .wrap(Wrap { trim: false }),
            project_path_area,
        );
    }
}

fn build_planning_text(planned_actions: &[PlannedAction]) -> Text<'static> {
    let mut lines = vec![
        Line::from(Span::styled(
            "Task Planning",
            Style::default().fg(Color::Rgb(220, 229, 239)),
        )),
        Line::from(""),
    ];

    if planned_actions.is_empty() {
        lines.push(Line::from(Span::styled(
            "No planning to show yet...",
            Style::default().fg(Color::Rgb(180, 196, 214)),
        )));
        return Text::from(lines);
    }

    for action in planned_actions.iter().rev().take(6) {
        lines.push(Line::from(Span::styled(
            format!(
                "{} {}",
                status_icon(action.status.clone()),
                action.actuator.replace("filesystem_", "")
            ),
            Style::default().fg(status_color(action.status.clone())),
        )));

        if let Some(summary) = summarize_action_args(action) {
            lines.push(Line::from(Span::styled(
                format!("  {summary}"),
                Style::default().fg(Color::Rgb(180, 196, 214)),
            )));
        }

        if let Some(details) = &action.details {
            lines.push(Line::from(Span::styled(
                format!("  {details}"),
                Style::default().fg(Color::Rgb(155, 171, 191)),
            )));
        }

        lines.push(Line::from(""));
    }

    Text::from(lines)
}

fn summarize_action_args(action: &PlannedAction) -> Option<String> {
    if action.actuator == "filesystem_read" {
        return action
            .args
            .get("file_path")
            .and_then(|value| value.as_str())
            .map(ToString::to_string);
    }

    let pattern = action
        .args
        .get("pattern")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    if pattern.is_empty() {
        return None;
    }

    let path = action
        .args
        .get("path")
        .and_then(|value| value.as_str())
        .unwrap_or(".");
    Some(format!("{pattern} in {path}"))
}

fn status_icon(status: PlannedActionStatus) -> &'static str {
    match status {
        PlannedActionStatus::Planned => "[ ]",
        PlannedActionStatus::InProgress => "[~]",
        PlannedActionStatus::AwaitingApproval => "[?]",
        PlannedActionStatus::Completed => "[x]",
        PlannedActionStatus::Failed => "[!]",
        PlannedActionStatus::Blocked => "[!]",
        PlannedActionStatus::Skipped => "[-]",
    }
}

fn status_color(status: PlannedActionStatus) -> Color {
    match status {
        PlannedActionStatus::Planned => Color::Rgb(180, 196, 214),
        PlannedActionStatus::InProgress => Color::Rgb(255, 240, 140),
        PlannedActionStatus::AwaitingApproval => Color::Rgb(255, 214, 120),
        PlannedActionStatus::Completed => Color::Rgb(127, 214, 154),
        PlannedActionStatus::Failed => Color::Rgb(255, 120, 120),
        PlannedActionStatus::Blocked => Color::Rgb(255, 164, 89),
        PlannedActionStatus::Skipped => Color::Rgb(152, 165, 181),
    }
}

fn format_workspace_path(path: Option<&str>) -> String {
    let Some(path) = path else {
        return "(not set)".to_string();
    };

    let home = env::var("USERPROFILE")
        .ok()
        .or_else(|| env::var("HOME").ok())
        .unwrap_or_default();
    if home.is_empty() {
        return path.to_string();
    }

    let normalized_home = home.replace('\\', "/").to_lowercase();
    let normalized_path = path.replace('\\', "/");
    let normalized_path_lower = normalized_path.to_lowercase();
    if normalized_path_lower.starts_with(&normalized_home) {
        let suffix = &normalized_path[normalized_home.len()..];
        if suffix.is_empty() {
            return "~".to_string();
        }
        if suffix.starts_with('/') {
            return format!("~{suffix}");
        }
        return format!("~/{suffix}");
    }

    normalized_path
}
