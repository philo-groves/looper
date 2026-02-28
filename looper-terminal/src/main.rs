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
    DiscoveryRequest, DiscoveryResponse, ProviderApiKey,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use ratatui_widgets::list::{List, ListItem, ListState};
use ratatui_widgets::scrollbar::{Scrollbar, ScrollbarOrientation, ScrollbarState};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const TICK_RATE: Duration = Duration::from_millis(450);
const PROVIDERS: [&str; 4] = ["openai", "anthropic", "google", "xai"];

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
                    | DiscoveryResponse::AgentCreated { .. } => {
                    }
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
                    DiscoveryResponse::AgentCreated { assigned_port: port } => {
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
                        if let Some(entry) = agents.iter().find(|entry| {
                            entry.workspace_dir == workspace_dir && entry.is_running
                        }) {
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
    api_key: String,
}

fn run_setup_flow(agent: &AgentInfo) -> anyhow::Result<Option<SetupForm>> {
    let mut app = SetupApp {
        stage: SetupStage::Workspace,
        should_quit: false,
        workspace_input: String::new(),
        provider_index: 0,
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

    let mut app = ChatApp {
        should_quit: false,
        input: String::new(),
        cursor_visible: true,
        messages: vec![format!(
            "Connected to {} ({}) on ws://{}:{}",
            agent_name(agent),
            agent.agent_id,
            AGENT_HOST,
            agent.assigned_port
        )],
        scroll_offset: 0,
        follow_tail: true,
        status: ChatStatus::Idle,
        status_ticks: 0,
        ws_status: WebSocketStatus::Disconnected,
        connection_state,
        agent_name: agent.agent_name.clone(),
        agent_workspace: agent.workspace_dir.clone(),
    };

    let result = run_tui_loop(&mut app, draw_chat, handle_chat_key);
    monitor_handle.abort();
    result
}

async fn monitor_agent_connection(agent_port: u16, state: Arc<AtomicBool>) {
    loop {
        let connected = tokio::time::timeout(
            Duration::from_millis(700),
            is_agent_running(agent_port),
        )
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

    if writer.send(Message::Text(list_request.into())).await.is_err() {
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
                if agent.is_running { "running" } else { "stopped" },
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
    ApiKey,
    Confirm,
    Done,
}

struct SetupApp {
    stage: SetupStage,
    should_quit: bool,
    workspace_input: String,
    provider_index: usize,
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
                    app.error_message = Some(format!("Could not create workspace directory: {error}"));
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
                app.stage = SetupStage::ApiKey;
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
                    app.stage = SetupStage::ApiKey;
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
        SetupStage::ApiKey => draw_api_step(frame, app),
        SetupStage::Confirm => draw_confirm_step(frame, app),
        SetupStage::Done => {}
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
        Paragraph::new(format!("> {}{cursor}", app.workspace_input))
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
        .constraints([Constraint::Length(3), Constraint::Min(3), Constraint::Length(2)])
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
        Paragraph::new(format!("> {masked}{cursor}"))
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
        "Confirm setup values:\n- workspace: {}\n- port: {}\n- provider: {}",
        app.workspace_input.trim(),
        app.agent_port,
        PROVIDERS[app.provider_index]
    );
    frame.render_widget(
        Paragraph::new(summary)
            .style(Style::default().fg(Color::Rgb(225, 235, 250)))
            .wrap(Wrap { trim: false }),
        chunks[0],
    );

    let options = vec![
        ListItem::new(Line::from("Confirm and save")),
        ListItem::new(Line::from("Back to API key")),
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
    cursor_visible: bool,
    messages: Vec<String>,
    scroll_offset: usize,
    follow_tail: bool,
    status: ChatStatus,
    status_ticks: u8,
    ws_status: WebSocketStatus,
    connection_state: Arc<AtomicBool>,
    agent_name: Option<String>,
    agent_workspace: Option<String>,
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
    PerformingTask,
    Responding,
}

impl ChatStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Thinking => "Thinking...",
            Self::PerformingTask => "Performing a Task...",
            Self::Responding => "Responding...",
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

        match self.status {
            ChatStatus::Idle => {}
            ChatStatus::Thinking => {
                self.status_ticks = self.status_ticks.saturating_add(1);
                if self.status_ticks >= 2 {
                    self.status = ChatStatus::PerformingTask;
                    self.status_ticks = 0;
                }
            }
            ChatStatus::PerformingTask => {
                self.status_ticks = self.status_ticks.saturating_add(1);
                if self.status_ticks >= 2 {
                    self.status = ChatStatus::Responding;
                    self.status_ticks = 0;
                }
            }
            ChatStatus::Responding => {
                self.status_ticks = self.status_ticks.saturating_add(1);
                if self.status_ticks >= 2 {
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
        }
        KeyCode::Enter => {
            if app.input.trim().is_empty() {
                return;
            }
            app.messages.push(format!("You: {}", app.input.trim()));
            app.input.clear();
            app.follow_tail = true;
            app.status = ChatStatus::Thinking;
            app.status_ticks = 0;
        }
        KeyCode::Up => {
            app.follow_tail = false;
            app.scroll_offset = app.scroll_offset.saturating_sub(1);
        }
        KeyCode::Down => {
            app.scroll_offset = app.scroll_offset.saturating_add(1);
        }
        KeyCode::Char(c) => {
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT)
            {
                app.input.push(c);
            }
        }
        _ => {}
    }
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
            app.agent_workspace.as_deref(),
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
        .constraints([Constraint::Min(3), Constraint::Length(4), Constraint::Length(1)])
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
    let input_line = format!("> {}{cursor}", app.input);

    let input_outer = Block::default().style(
        Style::default()
            .bg(Color::Rgb(28, 35, 45))
            .fg(Color::Rgb(219, 227, 238)),
    );
    frame.render_widget(input_outer, rows[1]);

    let input_container = Rect {
        x: rows[1].x.saturating_add(1),
        y: rows[1].y,
        width: rows[1].width.saturating_sub(2),
        height: rows[1].height,
    };
    let input_bg = Block::default().style(
        Style::default()
            .bg(Color::Rgb(43, 54, 69))
            .fg(Color::Rgb(242, 248, 255)),
    );
    frame.render_widget(input_bg, input_container);

    let input_margin = Rect {
        x: input_container.x.saturating_add(1),
        y: input_container.y,
        width: input_container.width.saturating_sub(2),
        height: input_container.height,
    };
    let input = Paragraph::new(input_line).style(
        Style::default()
            .bg(Color::Rgb(43, 54, 69))
            .fg(Color::Rgb(242, 248, 255)),
    );
    frame.render_widget(input, input_margin);

    let status = Paragraph::new(format!(" Status: {}", app.status.label())).style(
        Style::default()
            .bg(Color::Rgb(23, 29, 37))
            .fg(Color::Rgb(144, 163, 183)),
    );
    frame.render_widget(status, rows[2]);
}

fn build_history_lines(messages: &[String], width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![Line::from("")];
    }

    if width <= 2 {
        let mut compact = vec![Line::from("")];
        for message in messages {
            compact.push(Line::from(message.clone()));
            compact.push(Line::from(""));
        }
        return compact;
    }

    let mut lines = Vec::new();
    lines.push(Line::from(""));

        let bubble_outer_width = width.saturating_sub(2);
        let bubble_inner_width = bubble_outer_width.saturating_sub(2).max(1);

    for message in messages {
        let is_user = message.starts_with("You:");
        if is_user {
            lines.push(Line::from(format!(" {}", message)));
            lines.push(Line::from(""));
            continue;
        }

        let bubble_style = Style::default()
            .bg(Color::Rgb(20, 24, 31))
            .fg(Color::Rgb(224, 233, 245));
        let wrapped = wrap_text(message, bubble_inner_width);

        lines.push(Line::from(Span::styled(
            " ".repeat(bubble_outer_width),
            bubble_style,
        )));
        for part in wrapped {
            let mut content = String::with_capacity(bubble_outer_width);
            content.push(' ');
            content.push_str(&pad_right(&part, bubble_inner_width));
            content.push(' ');
            lines.push(Line::from(Span::styled(content, bubble_style)));
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

fn pad_right(input: &str, width: usize) -> String {
    let len = input.chars().count();
    if len >= width {
        input.to_string()
    } else {
        format!("{input}{}", " ".repeat(width - len))
    }
}

fn draw_sidenav(
    frame: &mut Frame,
    area: Rect,
    ws_status: WebSocketStatus,
    agent_name: Option<&str>,
    agent_workspace: Option<&str>,
) {
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(16, 19, 25))),
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

    if agent_name.is_none() {
        let tip_area = Rect {
            x: area.x.saturating_add(1),
            y: area.y.saturating_add(2),
            width: area.width.saturating_sub(2),
            height: 1,
        };
        frame.render_widget(
            Paragraph::new("Tip: /rename <new name>")
                .style(Style::default().fg(Color::Rgb(144, 163, 183))),
            tip_area,
        );
    }

    let label = format!(" {} ", ws_status.label());
    let label_width = label.chars().count() as u16;
    let right_margin = 1;
    let top_margin = 1;

    let badge_x = area
        .x
        .saturating_add(area.width.saturating_sub(label_width.saturating_add(right_margin)));
    let badge_y = area.y.saturating_add(top_margin);
    let badge_area = Rect {
        x: badge_x,
        y: badge_y,
        width: label_width.min(area.width),
        height: 1,
    };

    let badge = Paragraph::new(label).style(
        Style::default()
            .bg(ws_status.bg_color())
            .fg(Color::Rgb(16, 19, 25)),
    );
    frame.render_widget(badge, badge_area);

    if area.height >= 3 {
        let workspace_label_area = Rect {
            x: area.x.saturating_add(1),
            y: area.y.saturating_add(area.height.saturating_sub(3)),
            width: area.width.saturating_sub(2),
            height: 1,
        };
        frame.render_widget(
            Paragraph::new("Agent Workspace")
                .style(Style::default().fg(Color::Rgb(220, 229, 239))),
            workspace_label_area,
        );

        let workspace_value = format_workspace_path(agent_workspace);
        let workspace_path_area = Rect {
            x: area.x.saturating_add(1),
            y: area.y.saturating_add(area.height.saturating_sub(2)),
            width: area.width.saturating_sub(2),
            height: 2,
        };
        frame.render_widget(
            Paragraph::new(workspace_value)
                .style(Style::default().fg(Color::Rgb(144, 163, 183)))
                .wrap(Wrap { trim: false }),
            workspace_path_area,
        );
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

fn agent_name(agent: &AgentInfo) -> &str {
    agent.agent_name.as_deref().unwrap_or("unnamed")
}
