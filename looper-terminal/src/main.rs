use std::env;
use std::io;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::{SinkExt, StreamExt};
use looper_common::{
    AGENT_HOST, AgentInfo, AgentMode, AgentSocketMessage, DEFAULT_DISCOVERY_URL, DiscoveryRequest,
    DiscoveryResponse, ProviderApiKey,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use ratatui_widgets::list::{List, ListItem, ListState};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const TICK_RATE: Duration = Duration::from_millis(450);
const PROVIDERS: [&str; 4] = ["openai", "anthropic", "google", "xai"];

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let agents = discover_agents().await?;
    let Some(agent) = run_agent_selector(agents)? else {
        println!("No agents discovered. Start looper-discovery and at least one looper-agent.");
        return Ok(());
    };

    let mode = fetch_agent_mode(&agent).await?;
    if mode == AgentMode::Setup {
        let Some(form) = run_setup_flow(&agent)? else {
            return Ok(());
        };
        submit_setup(&agent, form).await?;
    }

    run_chat_ui(&agent)
}

async fn discover_agents() -> anyhow::Result<Vec<AgentInfo>> {
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
                    DiscoveryResponse::Agents { active_agents } => return Ok(active_agents),
                    DiscoveryResponse::Error { message } => {
                        bail!("discovery server returned error: {message}")
                    }
                    DiscoveryResponse::Registered { .. } | DiscoveryResponse::AgentLaunchUpserted => {
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

fn run_agent_selector(agents: Vec<AgentInfo>) -> anyhow::Result<Option<AgentInfo>> {
    if agents.is_empty() {
        return Ok(None);
    }

    if agents.len() == 1 {
        return Ok(Some(agents[0].clone()));
    }

    let mut app = AgentSelectorApp {
        agents,
        selected_index: 0,
        should_quit: false,
        confirmed: false,
    };

    run_tui_loop(&mut app, draw_agent_selector, handle_agent_selector_key)?;
    if app.confirmed {
        Ok(app.agents.get(app.selected_index).cloned())
    } else {
        Ok(None)
    }
}

async fn fetch_agent_mode(agent: &AgentInfo) -> anyhow::Result<AgentMode> {
    let url = format!("ws://{AGENT_HOST}:{}", agent.assigned_port);
    let (ws_stream, _) = connect_async(&url)
        .await
        .with_context(|| format!("failed to connect to agent websocket at {url}"))?;
    let (_, mut reader) = ws_stream.split();

    while let Some(message) = reader.next().await {
        match message {
            Ok(Message::Text(text)) => {
                let payload: AgentSocketMessage = serde_json::from_str(&text)
                    .with_context(|| format!("invalid agent hello payload: {text}"))?;
                if let AgentSocketMessage::AgentHello { mode, .. } = payload {
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
    let mut app = ChatApp {
        should_quit: false,
        input: String::new(),
        cursor_visible: true,
        messages: vec![format!(
            "Connected to {} ({}) on ws://{}:{}",
            agent.agent_id,
            agent_name(agent),
            AGENT_HOST,
            agent.assigned_port
        )],
    };

    run_tui_loop(&mut app, draw_chat, handle_chat_key)
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
    agents: Vec<AgentInfo>,
    selected_index: usize,
    should_quit: bool,
    confirmed: bool,
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
            app.confirmed = true;
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

    let title = Paragraph::new("Select an available agent").style(
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
                "{} ({}) - {} - ws://127.0.0.1:{}",
                agent.agent_id,
                agent_name(agent),
                match agent.mode {
                    AgentMode::Setup => "setup",
                    AgentMode::Running => "running",
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

    let help = Paragraph::new("Up/Down to move, Enter to confirm, Esc to quit").style(
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
}

impl TuiApp for ChatApp {
    fn should_quit(&self) -> bool {
        self.should_quit
    }

    fn on_tick(&mut self) {
        self.cursor_visible = !self.cursor_visible;
    }
}

fn handle_chat_key(app: &mut ChatApp, key: KeyEvent) {
    if matches!(key.code, KeyCode::Esc)
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
    {
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
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(75), Constraint::Percentage(25)])
        .split(area);

    draw_chat_panel(frame, split[0], app);
    draw_sidenav(frame, split[1]);
}

fn draw_chat_panel(frame: &mut Frame, area: Rect, app: &ChatApp) {
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(23, 29, 37))),
        area,
    );

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);

    let history_text = if app.messages.is_empty() {
        "".to_string()
    } else {
        app.messages.join("\n")
    };

    let history = Paragraph::new(history_text)
        .style(
            Style::default()
                .bg(Color::Rgb(28, 35, 45))
                .fg(Color::Rgb(219, 227, 238)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(history, rows[0]);

    let cursor = if app.cursor_visible { "█" } else { " " };
    let input_line = format!("> {}{cursor}", app.input);
    let input = Paragraph::new(input_line).style(
        Style::default()
            .bg(Color::Rgb(43, 54, 69))
            .fg(Color::Rgb(242, 248, 255)),
    );
    frame.render_widget(input, rows[1]);
}

fn draw_sidenav(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(16, 19, 25))),
        area,
    );
}

fn agent_name(agent: &AgentInfo) -> &str {
    agent.agent_name.as_deref().unwrap_or("unnamed")
}
