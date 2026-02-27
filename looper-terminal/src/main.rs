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
use looper_common::{AgentInfo, DEFAULT_DISCOVERY_URL, DiscoveryRequest, DiscoveryResponse};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use ratatui_widgets::list::{List, ListItem, ListState};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const TICK_RATE: Duration = Duration::from_millis(450);

#[derive(Clone, Copy)]
enum Screen {
    AgentSelect,
    Chat,
}

struct App {
    screen: Screen,
    agents: Vec<AgentInfo>,
    selected_agent: Option<AgentInfo>,
    selected_index: usize,
    messages: Vec<String>,
    input: String,
    cursor_visible: bool,
    should_quit: bool,
}

impl App {
    fn new(agents: Vec<AgentInfo>) -> Self {
        let mut messages = Vec::new();
        let mut selected_agent = None;
        let screen = if agents.len() > 1 {
            Screen::AgentSelect
        } else {
            Screen::Chat
        };

        if agents.is_empty() {
            messages.push(
                "No agents discovered. Start a looper-agent and restart terminal.".to_string(),
            );
        } else if agents.len() == 1 {
            let agent = agents[0].clone();
            messages.push(format!(
                "Auto-selected only available agent: {} ({}) on port {}",
                agent.agent_id,
                agent_name(&agent),
                agent.assigned_port
            ));
            selected_agent = Some(agent);
        }

        Self {
            screen,
            agents,
            selected_agent,
            selected_index: 0,
            messages,
            input: String::new(),
            cursor_visible: true,
            should_quit: false,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if matches!(key.code, KeyCode::Esc)
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
        {
            self.should_quit = true;
            return;
        }

        match self.screen {
            Screen::AgentSelect => self.handle_select_key(key),
            Screen::Chat => self.handle_chat_key(key),
        }
    }

    fn handle_select_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => {
                if self.selected_index > 0 {
                    self.selected_index -= 1;
                }
            }
            KeyCode::Down => {
                if self.selected_index + 1 < self.agents.len() {
                    self.selected_index += 1;
                }
            }
            KeyCode::Enter => {
                if let Some(agent) = self.agents.get(self.selected_index).cloned() {
                    self.messages.push(format!(
                        "Selected agent: {} ({}) on port {}",
                        agent.agent_id,
                        agent_name(&agent),
                        agent.assigned_port
                    ));
                    self.selected_agent = Some(agent);
                    self.screen = Screen::Chat;
                }
            }
            _ => {}
        }
    }

    fn handle_chat_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Enter => {
                if self.input.trim().is_empty() {
                    return;
                }
                self.messages.push(format!("You: {}", self.input.trim()));
                self.input.clear();
            }
            KeyCode::Char(c) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT)
                {
                    self.input.push(c);
                }
            }
            _ => {}
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let agents = discover_agents().await?;
    let mut app = App::new(agents);
    run_tui(&mut app)
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
                    DiscoveryResponse::Registered { .. } => {}
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
    }

    bail!("discovery server closed before listing agents")
}

fn run_tui(app: &mut App) -> anyhow::Result<()> {
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("failed to create terminal backend")?;

    let result = tui_loop(&mut terminal, app);

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    result
}

fn tui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> anyhow::Result<()> {
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|frame| draw(frame, app))?;

        let timeout = TICK_RATE.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            let event = event::read()?;
            if let Event::Key(key) = event {
                if key.kind == KeyEventKind::Press {
                    app.handle_key(key);
                }
            }
        }

        if last_tick.elapsed() >= TICK_RATE {
            app.cursor_visible = !app.cursor_visible;
            last_tick = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn draw(frame: &mut Frame, app: &mut App) {
    match app.screen {
        Screen::AgentSelect => draw_agent_select(frame, app),
        Screen::Chat => draw_chat(frame, app),
    }
}

fn draw_agent_select(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let container = Block::default().style(Style::default().bg(Color::Rgb(17, 21, 28)));
    frame.render_widget(container, area);

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
                "{} ({}) - ws://127.0.0.1:{}",
                agent.agent_id,
                agent_name(agent),
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

fn draw_chat(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(75), Constraint::Percentage(25)])
        .split(area);

    draw_chat_panel(frame, split[0], app);
    draw_sidenav(frame, split[1]);
}

fn draw_chat_panel(frame: &mut Frame, area: Rect, app: &App) {
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
    let input_line = format!("> {}{}", app.input, cursor);
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
