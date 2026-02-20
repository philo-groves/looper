use std::env;
use std::io;
use std::io::Write;
use std::process::Command;
use std::time::{Duration, Instant};
use std::{fs, path::PathBuf};
use std::{fs::OpenOptions, path::Path};

use anyhow::{Result, anyhow};
use arboard::Clipboard;
use chrono::Local;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use fiddlesticks::{ProviderId, list_models_with_api_key};
use looper_agent::{
    AgentState, ExecutionResult, ModelProviderKind, ModelSelection, ObservabilitySnapshot,
    PersistedIteration,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState, Sparkline,
};
use serde::{Deserialize, Serialize};

#[tokio::main]
async fn main() -> Result<()> {
    clear_terminal_log();
    let args = std::env::args().skip(1).collect::<Vec<String>>();
    if matches!(args.first().map(String::as_str), Some("serve")) {
        return Err(anyhow!(
            "server mode was removed. Run looper-agent for the background process"
        ));
    }

    if !args.is_empty() {
        return run_one_shot(args.join(" ")).await;
    }

    run_tui().await
}

async fn run_one_shot(message: String) -> Result<()> {
    append_terminal_log(&format!("starting one-shot mode message={}", message));
    let client = AgentClient::new(default_agent_base_url());
    client.health().await?;
    client.enqueue_chat_message(message).await?;
    println!("message accepted by looper-agent");
    Ok(())
}

#[derive(Clone)]
struct AgentClient {
    base_url: String,
    http: reqwest::Client,
}

impl AgentClient {
    fn new(base_url: String) -> Self {
        Self {
            base_url,
            http: reqwest::Client::new(),
        }
    }

    async fn health(&self) -> Result<()> {
        let response = self
            .http
            .get(format!("{}/api/health", self.base_url))
            .send()
            .await?
            .error_for_status()?;
        let body = response.json::<HealthResponse>().await?;
        if body.status != "ok" {
            return Err(anyhow!("agent reported unhealthy status: {}", body.status));
        }
        Ok(())
    }

    async fn state(&self) -> Result<AgentStateResponse> {
        Ok(self
            .http
            .get(format!("{}/api/state", self.base_url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn metrics(&self) -> Result<ObservabilitySnapshot> {
        Ok(self
            .http
            .get(format!("{}/api/metrics", self.base_url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn enqueue_chat_message(&self, message: String) -> Result<()> {
        self.http
            .post(format!("{}/api/percepts/chat", self.base_url))
            .json(&ChatPerceptRequest { message })
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    async fn register_api_key(&self, provider: ModelProviderKind, api_key: String) -> Result<()> {
        let response = self
            .http
            .post(format!("{}/api/config/keys", self.base_url))
            .json(&ApiKeyRequest { provider, api_key })
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("register key failed ({status}): {body}"));
        }
        Ok(())
    }

    async fn configure_models(
        &self,
        local: ModelSelection,
        frontier: ModelSelection,
    ) -> Result<()> {
        let response = self
            .http
            .post(format!("{}/api/config/models", self.base_url))
            .json(&ModelConfigRequest { local, frontier })
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow!("configure models failed ({status}): {body}"));
        }
        Ok(())
    }

    async fn start_loop(&self, interval_ms: u64) -> Result<LoopStatusResponse> {
        Ok(self
            .http
            .post(format!("{}/api/loop/start", self.base_url))
            .json(&LoopStartRequest {
                interval_ms: Some(interval_ms),
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn loop_status(&self) -> Result<LoopStatusResponse> {
        Ok(self
            .http
            .get(format!("{}/api/loop/status", self.base_url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn list_iterations_after(
        &self,
        after_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<PersistedIteration>> {
        let response = self
            .http
            .get(format!("{}/api/iterations", self.base_url))
            .query(&[("after_id", after_id), ("limit", Some(limit as i64))])
            .send()
            .await?
            .error_for_status()?;
        let body = response.json::<IterationsResponse>().await?;
        Ok(body.iterations)
    }
}

#[derive(Clone, Debug, Deserialize)]
struct HealthResponse {
    status: String,
}

#[derive(Clone, Debug, Deserialize)]
struct AgentStateResponse {
    state: AgentState,
    reason: Option<String>,
    configured: bool,
    local_selection: Option<ModelSelection>,
    frontier_selection: Option<ModelSelection>,
    latest_iteration_id: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
struct LoopStatusResponse {
    running: bool,
    interval_ms: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct IterationsResponse {
    iterations: Vec<PersistedIteration>,
}

#[derive(Clone, Debug, Serialize)]
struct ChatPerceptRequest {
    message: String,
}

#[derive(Clone, Debug, Serialize)]
struct ApiKeyRequest {
    provider: ModelProviderKind,
    api_key: String,
}

#[derive(Clone, Debug, Serialize)]
struct ModelConfigRequest {
    local: ModelSelection,
    frontier: ModelSelection,
}

#[derive(Clone, Debug, Serialize)]
struct LoopStartRequest {
    interval_ms: Option<u64>,
}

fn default_agent_base_url() -> String {
    env::var("LOOPER_AGENT_URL").unwrap_or_else(|_| "http://127.0.0.1:10001".to_string())
}

async fn run_tui() -> Result<()> {
    append_terminal_log("starting tui mode");
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new().await?;
    let result = app.run(&mut terminal).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    append_terminal_log("tui exited");
    result
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SetupStep {
    LocalProvider,
    LocalModel,
    LocalModelVersion,
    FrontierProvider,
    FrontierApiKey,
    FrontierModel,
    InstallOllamaPrompt,
    InstallModelPrompt,
}

struct App {
    client: AgentClient,
    agent_state: AgentState,
    configured: bool,
    local_selection: Option<ModelSelection>,
    frontier_selection: Option<ModelSelection>,
    observability: ObservabilitySnapshot,
    latest_iteration_id: Option<i64>,
    iterations_initialized: bool,
    stop_reason: Option<String>,
    loop_running: bool,
    loop_interval_ms: u64,
    step: SetupStep,
    ollama_catalog_models: Vec<String>,
    ollama_catalog_tagged_models: Vec<String>,
    local_model_index: usize,
    local_model_versions: Vec<String>,
    local_model_version_index: usize,
    frontier_provider_index: usize,
    frontier_api_key: String,
    frontier_models: Vec<String>,
    frontier_model_index: usize,
    pending_missing_models: Vec<String>,
    install_prompt_index: usize,
    chat_input: String,
    status: String,
    activity_log: Vec<String>,
    chat_history: Vec<String>,
    latest_loop_state_log: String,
    start_timestamp: String,
    started_at: Instant,
    loops_per_minute_history: Vec<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PersistedSetupConfig {
    #[serde(default = "default_local_provider")]
    local_provider: ModelProviderKind,
    local_model: String,
    frontier_provider: ModelProviderKind,
    frontier_model: String,
}

fn default_local_provider() -> ModelProviderKind {
    ModelProviderKind::Ollama
}

fn empty_observability_snapshot() -> ObservabilitySnapshot {
    ObservabilitySnapshot {
        phase_execution_counts: Default::default(),
        local_model_tokens: 0,
        frontier_model_tokens: 0,
        false_positive_surprises: 0,
        false_positive_surprise_percent: 0.0,
        failed_tool_executions: 0,
        failed_tool_execution_percent: 0.0,
        total_iterations: 0,
        loops_per_minute: 0.0,
    }
}

impl App {
    async fn new() -> Result<Self> {
        let client = AgentClient::new(default_agent_base_url());
        let catalog = scrape_ollama_library_base_models()
            .await
            .unwrap_or_else(|_| {
                vec![
                    "gemma3".to_string(),
                    "qwen3".to_string(),
                    "gpt-oss".to_string(),
                ]
            });
        let tagged_catalog = scrape_ollama_library_models().await.unwrap_or_default();

        let local_index = catalog
            .iter()
            .position(|item| item == "gemma3" || item.contains("qwen") || item.contains("gpt-oss"))
            .unwrap_or(0);

        let mut app = Self {
            client,
            agent_state: AgentState::Setup,
            configured: false,
            local_selection: None,
            frontier_selection: None,
            observability: empty_observability_snapshot(),
            latest_iteration_id: None,
            iterations_initialized: false,
            stop_reason: None,
            loop_running: false,
            loop_interval_ms: 0,
            step: SetupStep::LocalProvider,
            ollama_catalog_models: catalog,
            ollama_catalog_tagged_models: tagged_catalog,
            local_model_index: local_index,
            local_model_versions: Vec::new(),
            local_model_version_index: 0,
            frontier_provider_index: 0,
            frontier_api_key: std::env::var("OPENAI_API_KEY").unwrap_or_default(),
            frontier_models: Vec::new(),
            frontier_model_index: 0,
            pending_missing_models: Vec::new(),
            install_prompt_index: 0,
            chat_input: String::new(),
            status: "Setup: use arrows + Enter. Ctrl+V/right-click to paste.".to_string(),
            activity_log: Vec::new(),
            chat_history: Vec::new(),
            latest_loop_state_log: "(no loop state yet)".to_string(),
            start_timestamp: format_start_timestamp(Local::now()),
            started_at: Instant::now(),
            loops_per_minute_history: vec![0.0],
        };

        if let Err(error) = app.refresh_agent_status().await {
            app.status = format!("Could not reach looper-agent: {error}");
        }

        if let Some(config) = read_persisted_setup_config()? {
            app.apply_persisted_setup_config(config.clone()).await;
            match app.try_start_from_persisted_setup(config).await {
                Ok(()) => {
                    app.status = "Restored setup and started in running mode".to_string();
                }
                Err(error) => {
                    app.status =
                        format!("Restored setup selections, but auto-start failed: {error}");
                }
            }
        }

        Ok(app)
    }

    async fn apply_persisted_setup_config(&mut self, config: PersistedSetupConfig) {
        if config.local_provider != ModelProviderKind::Ollama {
            self.status = format!(
                "Saved local provider {:?} is not supported yet; using Ollama",
                config.local_provider
            );
        }

        let (base_model, version) = split_model_and_version(&config.local_model);
        if let Some(index) = self
            .ollama_catalog_models
            .iter()
            .position(|item| item == base_model)
        {
            self.local_model_index = index;
            self.local_model_versions = scrape_ollama_model_versions(base_model)
                .await
                .unwrap_or_default();
            if self.local_model_versions.is_empty() {
                self.local_model_versions.push(version.to_string());
            }
            if let Some(version_index) = self
                .local_model_versions
                .iter()
                .position(|item| item == version)
            {
                self.local_model_version_index = version_index;
            } else {
                self.local_model_versions.push(version.to_string());
                self.local_model_versions.sort();
                self.local_model_versions.dedup();
                self.local_model_version_index = self
                    .local_model_versions
                    .iter()
                    .position(|item| item == version)
                    .unwrap_or(0);
            }
        }

        if let Some(provider_index) = frontier_provider_options()
            .iter()
            .position(|provider| *provider == config.frontier_provider)
        {
            self.frontier_provider_index = provider_index;
        }

        if config.frontier_provider == ModelProviderKind::Ollama {
            self.frontier_models = self.ollama_catalog_tagged_models.clone();
        } else {
            self.frontier_models = vec![config.frontier_model.clone()];
        }

        if let Some(frontier_index) = self
            .frontier_models
            .iter()
            .position(|item| item == &config.frontier_model)
        {
            self.frontier_model_index = frontier_index;
        } else if !config.frontier_model.is_empty() {
            self.frontier_models.push(config.frontier_model.clone());
            self.frontier_models.sort();
            self.frontier_models.dedup();
            self.frontier_model_index = self
                .frontier_models
                .iter()
                .position(|item| item == &config.frontier_model)
                .unwrap_or(0);
        }

        self.step = SetupStep::FrontierModel;
        self.status = "Restored previous setup selections".to_string();
    }

    async fn refresh_agent_status(&mut self) -> Result<()> {
        self.client.health().await?;

        let state = self.client.state().await?;
        if !self.iterations_initialized {
            self.latest_iteration_id = state.latest_iteration_id;
            self.iterations_initialized = true;
        }
        self.agent_state = state.state;
        self.configured = state.configured;
        self.local_selection = state.local_selection;
        self.frontier_selection = state.frontier_selection;
        self.stop_reason = state.reason;

        self.observability = self.client.metrics().await?;
        self.record_loops_per_minute(self.observability.loops_per_minute);

        let loop_status = self.client.loop_status().await?;
        self.loop_running = loop_status.running;
        self.loop_interval_ms = loop_status.interval_ms;

        let new_iterations = self
            .client
            .list_iterations_after(self.latest_iteration_id, 100)
            .await?;
        self.consume_iterations(new_iterations);

        Ok(())
    }

    async fn try_start_from_persisted_setup(&mut self, config: PersistedSetupConfig) -> Result<()> {
        if config.frontier_provider != ModelProviderKind::Ollama {
            let normalized_key = normalize_api_key_value(&self.frontier_api_key);
            if !normalized_key.is_empty() {
                self.client
                    .register_api_key(config.frontier_provider, normalized_key)
                    .await?;
            }
        }

        self.client
            .configure_models(
                ModelSelection {
                    provider: ModelProviderKind::Ollama,
                    model: config.local_model,
                },
                ModelSelection {
                    provider: config.frontier_provider,
                    model: config.frontier_model,
                },
            )
            .await?;
        self.client.start_loop(500).await?;
        self.refresh_agent_status().await?;
        Ok(())
    }

    fn consume_iterations(&mut self, iterations: Vec<PersistedIteration>) {
        for iteration in iterations {
            self.latest_iteration_id = Some(iteration.id);
            self.latest_loop_state_log = format!(
                "loop: sensed={} surprising={} actions={}",
                iteration.sensed_percepts.len(),
                iteration.surprising_percepts.len(),
                iteration.action_results.len()
            );

            for result in &iteration.action_results {
                match result {
                    ExecutionResult::Executed { output } if !output.trim().is_empty() => {
                        self.push_looper_message(output.trim());
                    }
                    ExecutionResult::Denied(reason) => {
                        self.push_looper_message(&format!("action denied ({reason})"));
                    }
                    ExecutionResult::RequiresHitl { approval_id } => {
                        self.push_looper_message(&format!(
                            "action requires HITL (approval id: {approval_id})"
                        ));
                    }
                    _ => {}
                }
            }
        }
    }

    async fn run(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        let mut last_tick = Instant::now();
        loop {
            terminal.draw(|frame| {
                if self.agent_state == AgentState::Setup {
                    self.draw_setup(frame);
                } else {
                    self.draw_runtime(frame);
                }
            })?;

            if event::poll(Duration::from_millis(60))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        match self.handle_key(key.code, key.modifiers).await {
                            Ok(should_exit) => {
                                if should_exit {
                                    break;
                                }
                            }
                            Err(error) => {
                                self.status = format!("input error: {error}");
                                append_terminal_log(&self.status);
                            }
                        }
                    }
                    Event::Paste(text) => {
                        self.handle_paste(text);
                    }
                    _ => {}
                }
            }

            if last_tick.elapsed() >= Duration::from_millis(500) {
                if let Err(error) = self.refresh_agent_status().await {
                    self.status = format!("agent refresh error: {error}");
                    append_terminal_log(&self.status);
                }
                last_tick = Instant::now();
            }
        }
        Ok(())
    }

    async fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<bool> {
        if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(true);
        }

        if code == KeyCode::Char('v') && modifiers.contains(KeyModifiers::CONTROL) {
            if let Ok(mut clipboard) = Clipboard::new()
                && let Ok(content) = clipboard.get_text()
            {
                self.handle_paste(content);
            }
            return Ok(false);
        }

        if self.agent_state == AgentState::Setup {
            self.handle_setup_key(code, modifiers).await?;
            return Ok(false);
        }

        self.handle_runtime_key(code, modifiers).await?;

        Ok(false)
    }

    async fn handle_setup_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        if modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(());
        }

        if matches!(code, KeyCode::Char('s') | KeyCode::Char('S')) {
            self.step = self.previous_setup_step(self.step);
            self.status = "Moved to previous setup step".to_string();
            return Ok(());
        }

        if matches!(code, KeyCode::Char('r') | KeyCode::Char('R')) {
            self.advance_setup_step().await?;
            return Ok(());
        }

        if code == KeyCode::Left {
            self.step = self.previous_setup_step(self.step);
            self.status = "Moved to previous setup step".to_string();
            return Ok(());
        }

        if code == KeyCode::Right {
            self.advance_setup_step().await?;
            return Ok(());
        }

        match self.step {
            SetupStep::LocalProvider => {
                if code == KeyCode::Enter {
                    self.step = SetupStep::LocalModel;
                }
            }
            SetupStep::LocalModel => match code {
                KeyCode::Up => {
                    self.local_model_index = self.local_model_index.saturating_sub(1);
                }
                KeyCode::Down => {
                    if self.local_model_index + 1 < self.ollama_catalog_models.len() {
                        self.local_model_index += 1;
                    }
                }
                KeyCode::Enter => {
                    let Some(selected_base) =
                        self.ollama_catalog_models.get(self.local_model_index)
                    else {
                        self.status = "No local model selected".to_string();
                        return Ok(());
                    };

                    self.local_model_versions = scrape_ollama_model_versions(selected_base)
                        .await
                        .unwrap_or_default();
                    if self.local_model_versions.is_empty() {
                        self.status = format!("No versions found for '{}'.", selected_base);
                    } else {
                        self.local_model_version_index =
                            preferred_version_index(&self.local_model_versions);
                        self.step = SetupStep::LocalModelVersion;
                    }
                }
                _ => {}
            },
            SetupStep::LocalModelVersion => match code {
                KeyCode::Up => {
                    self.local_model_version_index =
                        self.local_model_version_index.saturating_sub(1);
                }
                KeyCode::Down => {
                    if self.local_model_version_index + 1 < self.local_model_versions.len() {
                        self.local_model_version_index += 1;
                    }
                }
                KeyCode::Enter => {
                    self.step = SetupStep::FrontierProvider;
                }
                _ => {}
            },
            SetupStep::FrontierProvider => match code {
                KeyCode::Up => {
                    self.frontier_provider_index = self.frontier_provider_index.saturating_sub(1);
                }
                KeyCode::Down => {
                    if self.frontier_provider_index + 1 < frontier_provider_options().len() {
                        self.frontier_provider_index += 1;
                    }
                }
                KeyCode::Enter => {
                    let provider = self.selected_frontier_provider();
                    if provider == ModelProviderKind::Ollama {
                        self.frontier_models = self.ollama_catalog_tagged_models.clone();
                        self.step = SetupStep::FrontierModel;
                    } else {
                        if !self.frontier_api_key.trim().is_empty() {
                            self.frontier_models = self
                                .models_for_provider(provider, &self.frontier_api_key)
                                .await?;
                        }
                        self.step = SetupStep::FrontierApiKey;
                    }
                }
                _ => {}
            },
            SetupStep::FrontierApiKey => match code {
                KeyCode::Backspace => {
                    self.frontier_api_key.pop();
                }
                KeyCode::Enter => {
                    if self.frontier_api_key.trim().is_empty() {
                        if self.frontier_models.is_empty() {
                            self.frontier_models = vec![self.preferred_frontier_model()];
                            self.frontier_model_index = 0;
                            self.status = "No key entered. Using default model and saved agent key"
                                .to_string();
                        }
                        self.step = SetupStep::FrontierModel;
                    } else {
                        self.frontier_models = self
                            .models_for_provider(
                                self.selected_frontier_provider(),
                                &self.frontier_api_key,
                            )
                            .await?;
                        if self.frontier_models.is_empty() {
                            self.frontier_models = vec![self.preferred_frontier_model()];
                            self.frontier_model_index = 0;
                            self.status =
                                "No models returned for key. Using default model".to_string();
                            self.step = SetupStep::FrontierModel;
                        } else {
                            self.frontier_model_index = 0;
                            self.step = SetupStep::FrontierModel;
                        }
                    }
                }
                KeyCode::Char(ch) => {
                    self.frontier_api_key.push(ch);
                }
                _ => {}
            },
            SetupStep::FrontierModel => match code {
                KeyCode::Up => {
                    self.frontier_model_index = self.frontier_model_index.saturating_sub(1);
                }
                KeyCode::Down => {
                    if self.frontier_model_index + 1 < self.frontier_models.len() {
                        self.frontier_model_index += 1;
                    }
                }
                KeyCode::Enter => {
                    self.verify_ollama_and_models().await?;
                }
                _ => {}
            },
            SetupStep::InstallOllamaPrompt => {
                self.handle_install_prompt_key(code, true).await?;
            }
            SetupStep::InstallModelPrompt => {
                self.handle_install_prompt_key(code, false).await?;
            }
        }

        Ok(())
    }

    async fn advance_setup_step(&mut self) -> Result<()> {
        match self.step {
            SetupStep::LocalProvider => {
                self.step = SetupStep::LocalModel;
            }
            SetupStep::LocalModel => {
                let Some(selected_base) = self.ollama_catalog_models.get(self.local_model_index)
                else {
                    self.status = "No local model selected".to_string();
                    return Ok(());
                };

                self.local_model_versions = scrape_ollama_model_versions(selected_base)
                    .await
                    .unwrap_or_default();
                if self.local_model_versions.is_empty() {
                    self.status = format!("No versions found for '{}'.", selected_base);
                } else {
                    self.local_model_version_index =
                        preferred_version_index(&self.local_model_versions);
                    self.step = SetupStep::LocalModelVersion;
                }
            }
            SetupStep::LocalModelVersion => {
                self.step = SetupStep::FrontierProvider;
            }
            SetupStep::FrontierProvider => {
                let provider = self.selected_frontier_provider();
                self.step = if provider == ModelProviderKind::Ollama {
                    self.frontier_models = self.ollama_catalog_tagged_models.clone();
                    SetupStep::FrontierModel
                } else {
                    if !self.frontier_api_key.trim().is_empty() {
                        self.frontier_models = self
                            .models_for_provider(provider, &self.frontier_api_key)
                            .await?;
                    }
                    SetupStep::FrontierApiKey
                };
            }
            SetupStep::FrontierApiKey => {
                if self.frontier_api_key.trim().is_empty() {
                    if self.frontier_models.is_empty() {
                        self.frontier_models = vec![self.preferred_frontier_model()];
                        self.frontier_model_index = 0;
                        self.status =
                            "No key entered. Using default model and saved agent key".to_string();
                    }
                    self.step = SetupStep::FrontierModel;
                } else {
                    self.frontier_models = self
                        .models_for_provider(
                            self.selected_frontier_provider(),
                            &self.frontier_api_key,
                        )
                        .await?;
                    if self.frontier_models.is_empty() {
                        self.frontier_models = vec![self.preferred_frontier_model()];
                        self.frontier_model_index = 0;
                        self.status = "No models returned for key. Using default model".to_string();
                        self.step = SetupStep::FrontierModel;
                    } else {
                        self.frontier_model_index = 0;
                        self.step = SetupStep::FrontierModel;
                    }
                }
            }
            SetupStep::FrontierModel => {
                self.verify_ollama_and_models().await?;
            }
            SetupStep::InstallOllamaPrompt | SetupStep::InstallModelPrompt => {}
        }

        Ok(())
    }

    fn previous_setup_step(&self, current: SetupStep) -> SetupStep {
        match current {
            SetupStep::LocalProvider => SetupStep::LocalProvider,
            SetupStep::LocalModel => SetupStep::LocalProvider,
            SetupStep::LocalModelVersion => SetupStep::LocalModel,
            SetupStep::FrontierProvider => SetupStep::LocalModelVersion,
            SetupStep::FrontierApiKey => SetupStep::FrontierProvider,
            SetupStep::FrontierModel => {
                if self.selected_frontier_provider() == ModelProviderKind::Ollama {
                    SetupStep::FrontierProvider
                } else {
                    SetupStep::FrontierApiKey
                }
            }
            SetupStep::InstallOllamaPrompt | SetupStep::InstallModelPrompt => {
                SetupStep::FrontierModel
            }
        }
    }

    async fn handle_install_prompt_key(
        &mut self,
        code: KeyCode,
        ollama_prompt: bool,
    ) -> Result<()> {
        match code {
            KeyCode::Up | KeyCode::Down => {
                self.install_prompt_index = 1 - self.install_prompt_index;
            }
            KeyCode::Enter => {
                if self.install_prompt_index == 0 {
                    if ollama_prompt {
                        self.status = "Installing Ollama...".to_string();
                        self.push_log("Starting Ollama installation...");
                        let output = install_ollama()?;
                        for line in output {
                            self.push_log(&line);
                        }
                        self.status =
                            "Ollama install command completed. Continuing setup...".to_string();
                        self.verify_ollama_and_models().await?;
                    } else {
                        let models_to_install = self.pending_missing_models.clone();
                        let total = models_to_install.len();
                        for (index, model) in models_to_install.iter().enumerate() {
                            self.status =
                                format!("Installing model {}/{}: {}", index + 1, total, model);
                            self.push_log(&format!("pulling model: {model}"));
                            let output = pull_ollama_model(model)?;
                            for line in output {
                                self.push_log(&line);
                            }
                        }
                        self.status = "Selected model(s) installed. Finishing setup...".to_string();
                        self.finish_setup().await?;
                    }
                } else {
                    self.status =
                        "Setup not finished. Choose installed/available models.".to_string();
                    self.step = SetupStep::FrontierModel;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_runtime_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        if modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(());
        }

        match code {
            KeyCode::Backspace => {
                self.chat_input.pop();
            }
            KeyCode::Enter => {
                if modifiers.contains(KeyModifiers::SHIFT) {
                    self.chat_input.push('\n');
                } else if !self.chat_input.trim().is_empty() {
                    let queued_message = self.chat_input.trim().to_string();
                    self.client
                        .enqueue_chat_message(self.chat_input.clone())
                        .await?;
                    self.push_me_message(&queued_message);
                    append_terminal_log(&format!("chat message queued: {queued_message}"));
                    self.chat_input.clear();
                    self.status = "chat percept queued".to_string();
                }
            }
            KeyCode::Char(ch) => {
                self.chat_input.push(ch);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_paste(&mut self, text: String) {
        if text.is_empty() {
            return;
        }

        if self.agent_state == AgentState::Setup {
            if self.step == SetupStep::FrontierApiKey {
                self.frontier_api_key.push_str(&text);
                self.status = "pasted into API key".to_string();
            }
            return;
        }

        self.chat_input.push_str(&text);
        self.status = "pasted into chat input".to_string();
    }

    async fn verify_ollama_and_models(&mut self) -> Result<()> {
        if !is_ollama_installed() {
            self.install_prompt_index = 0;
            self.step = SetupStep::InstallOllamaPrompt;
            self.status = "Ollama is not installed. Install now?".to_string();
            return Ok(());
        }

        let installed = list_installed_ollama_models()?;
        let local_model = self.selected_local_model()?;

        let mut required = vec![local_model];
        if self.selected_frontier_provider() == ModelProviderKind::Ollama {
            let frontier_model = self
                .frontier_models
                .get(self.frontier_model_index)
                .cloned()
                .ok_or_else(|| anyhow!("no frontier model selected"))?;
            required.push(frontier_model);
        }

        required.sort();
        required.dedup();
        self.pending_missing_models = required
            .into_iter()
            .filter(|model| !model_is_installed(&installed, model))
            .collect();

        if self.pending_missing_models.is_empty() {
            self.finish_setup().await?;
        } else {
            self.install_prompt_index = 0;
            self.step = SetupStep::InstallModelPrompt;
            self.status = format!(
                "Selected model(s) not installed: {}. Install now?",
                self.pending_missing_models.join(", ")
            );
        }

        Ok(())
    }

    async fn finish_setup(&mut self) -> Result<()> {
        let local_model = self.selected_local_model()?;
        let frontier_model = self
            .frontier_models
            .get(self.frontier_model_index)
            .cloned()
            .ok_or_else(|| anyhow!("no frontier model selected"))?;

        let frontier_provider = self.selected_frontier_provider();
        let normalized_key = normalize_api_key_value(&self.frontier_api_key);
        if frontier_provider == ModelProviderKind::OpenAi && !normalized_key.is_empty() {
            self.client
                .register_api_key(ModelProviderKind::OpenAi, normalized_key.clone())
                .await?;
        }
        if frontier_provider == ModelProviderKind::OpenCodeZen && !normalized_key.is_empty() {
            self.client
                .register_api_key(ModelProviderKind::OpenCodeZen, normalized_key)
                .await?;
        }

        self.client
            .configure_models(
                ModelSelection {
                    provider: ModelProviderKind::Ollama,
                    model: local_model.clone(),
                },
                ModelSelection {
                    provider: frontier_provider,
                    model: frontier_model.clone(),
                },
            )
            .await?;
        if let Err(error) = write_persisted_setup_config(&PersistedSetupConfig {
            local_provider: ModelProviderKind::Ollama,
            local_model: local_model.clone(),
            frontier_provider,
            frontier_model: frontier_model.clone(),
        }) {
            self.push_log(&format!("failed to persist setup: {error}"));
        }

        self.client.start_loop(500).await?;
        self.refresh_agent_status().await?;
        self.status = "setup complete, now running".to_string();
        append_terminal_log(&format!(
            "setup complete local={} frontier_provider={} frontier_model={}",
            local_model,
            provider_label(frontier_provider),
            frontier_model
        ));
        Ok(())
    }

    async fn models_for_provider(
        &self,
        provider: ModelProviderKind,
        api_key: &str,
    ) -> Result<Vec<String>> {
        let api_key = normalize_api_key_value(api_key);
        let mut models = match provider {
            ModelProviderKind::Ollama => self.ollama_catalog_tagged_models.clone(),
            ModelProviderKind::OpenCodeZen => {
                list_models_with_api_key(ProviderId::OpenCodeZen, &api_key)
                    .await
                    .unwrap_or_default()
            }
            ModelProviderKind::OpenAi => list_openai_models(&api_key).await.unwrap_or_default(),
        };

        models.sort();
        models.dedup();
        Ok(models)
    }

    fn selected_frontier_provider(&self) -> ModelProviderKind {
        frontier_provider_options()[self.frontier_provider_index]
    }

    fn preferred_frontier_model(&self) -> String {
        let provider = self.selected_frontier_provider();
        if let Some(selection) = &self.frontier_selection
            && selection.provider == provider
            && !selection.model.trim().is_empty()
        {
            return selection.model.clone();
        }

        default_model_for_provider(provider).to_string()
    }

    fn selected_local_model(&self) -> Result<String> {
        let base = self
            .ollama_catalog_models
            .get(self.local_model_index)
            .cloned()
            .ok_or_else(|| anyhow!("no local model selected"))?;
        let version = self
            .local_model_versions
            .get(self.local_model_version_index)
            .cloned()
            .ok_or_else(|| anyhow!("no local model version selected"))?;
        Ok(format!("{base}:{version}"))
    }

    fn draw_setup(&self, frame: &mut ratatui::Frame<'_>) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(14),
                Constraint::Min(6),
            ])
            .split(frame.area());

        frame.render_widget(
            Paragraph::new(
                "Setup mode: arrows select, Enter continue, Ctrl+V/right-click paste, Ctrl+C quit",
            )
            .block(Block::default().borders(Borders::ALL).title("Looper Setup")),
            chunks[0],
        );

        let mut lines = vec![
            setup_line(
                "1. Select a local provider",
                "Ollama",
                self.step == SetupStep::LocalProvider,
            ),
            setup_line(
                "2. Select a local model",
                self.ollama_catalog_models
                    .get(self.local_model_index)
                    .map(String::as_str)
                    .unwrap_or("(none)"),
                self.step == SetupStep::LocalModel,
            ),
            setup_line(
                "2a. Select a model version",
                self.local_model_versions
                    .get(self.local_model_version_index)
                    .map(String::as_str)
                    .unwrap_or("(none)"),
                self.step == SetupStep::LocalModelVersion,
            ),
            setup_line(
                "3. Select a frontier provider",
                provider_label(self.selected_frontier_provider()),
                self.step == SetupStep::FrontierProvider,
            ),
        ];

        if self.selected_frontier_provider() != ModelProviderKind::Ollama {
            lines.push(setup_line(
                "3a. Add API key",
                &mask(&self.frontier_api_key),
                self.step == SetupStep::FrontierApiKey,
            ));
        }

        lines.push(setup_line(
            "4. Select a frontier model",
            self.frontier_models
                .get(self.frontier_model_index)
                .map(String::as_str)
                .unwrap_or("(none)"),
            self.step == SetupStep::FrontierModel,
        ));

        lines.push(Line::from(vec![Span::styled(
            "5. Setup is complete, move to running",
            Style::default().fg(Color::Green),
        )]));

        frame.render_widget(
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Flow")),
            chunks[1],
        );

        let options = match self.step {
            SetupStep::LocalProvider => vec!["Ollama".to_string()],
            SetupStep::LocalModel => self.ollama_catalog_models.clone(),
            SetupStep::LocalModelVersion => self.local_model_versions.clone(),
            SetupStep::FrontierProvider => frontier_provider_options()
                .iter()
                .map(|item| provider_label(*item).to_string())
                .collect(),
            SetupStep::FrontierApiKey => vec![format!("API key: {}", mask(&self.frontier_api_key))],
            SetupStep::FrontierModel => self.frontier_models.clone(),
            SetupStep::InstallOllamaPrompt => vec![
                "Yes, install Ollama now".to_string(),
                "No, keep setup open".to_string(),
            ],
            SetupStep::InstallModelPrompt => vec![
                format!(
                    "Yes, install missing model(s): {}",
                    self.pending_missing_models.join(", ")
                ),
                "No, keep setup open".to_string(),
            ],
        };

        let selected = match self.step {
            SetupStep::LocalProvider => 0,
            SetupStep::LocalModel => self.local_model_index,
            SetupStep::LocalModelVersion => self.local_model_version_index,
            SetupStep::FrontierProvider => self.frontier_provider_index,
            SetupStep::FrontierApiKey => 0,
            SetupStep::FrontierModel => self.frontier_model_index,
            SetupStep::InstallOllamaPrompt | SetupStep::InstallModelPrompt => {
                self.install_prompt_index
            }
        };

        let selected = selected.min(options.len().saturating_sub(1));
        let visible_capacity = usize::from(chunks[2].height.saturating_sub(3)).max(1);
        let (start, end) = visible_window(selected, options.len(), visible_capacity);

        let mut option_lines = Vec::new();
        if start > 0 {
            option_lines.push(Line::from(Span::styled(
                format!("... {} above", start),
                Style::default().fg(Color::DarkGray),
            )));
        }

        for (index, option) in options.iter().enumerate().skip(start).take(end - start) {
            let style = if index == selected {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::Gray)
            };
            option_lines.push(Line::from(Span::styled(option.clone(), style)));
        }

        if end < options.len() {
            option_lines.push(Line::from(Span::styled(
                format!("... {} below", options.len() - end),
                Style::default().fg(Color::DarkGray),
            )));
        }

        option_lines.push(Line::from(Span::styled(
            self.status.clone(),
            Style::default().fg(Color::Yellow),
        )));

        for line in self.activity_log.iter().rev().take(6).rev() {
            option_lines.push(Line::from(Span::styled(
                line.clone(),
                Style::default().fg(Color::Blue),
            )));
        }

        frame.render_widget(
            Paragraph::new(option_lines)
                .block(Block::default().borders(Borders::ALL).title("Options")),
            chunks[2],
        );
    }

    fn draw_runtime(&self, frame: &mut ratatui::Frame<'_>) {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
            .split(frame.area());

        let left = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Length(8)])
            .split(columns[0]);

        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5),
                Constraint::Length(5),
                Constraint::Min(6),
                Constraint::Length(4),
            ])
            .split(columns[1]);

        let history_block = Block::default().borders(Borders::ALL).title("Chat History");
        let history_inner = history_block.inner(left[0]);
        frame.render_widget(history_block, left[0]);

        let history_columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(history_inner);

        let content_width = usize::from(history_columns[0].width.saturating_sub(1)).max(1);
        let (items, history_line_count) = if self.chat_history.is_empty() {
            (vec![ListItem::new("(no chat yet)")], 1_usize)
        } else {
            let mut total_lines = 0_usize;
            let mut built_items = Vec::with_capacity(self.chat_history.len());
            for (index, entry) in self.chat_history.iter().enumerate() {
                let mut lines = wrap_chat_entry_lines(entry, content_width)
                    .into_iter()
                    .map(|line| styled_chat_history_line(&line))
                    .collect::<Vec<Line>>();
                total_lines += lines.len();
                if index + 1 < self.chat_history.len() {
                    lines.push(Line::from(""));
                    total_lines += 1;
                }
                built_items.push(ListItem::new(lines));
            }
            (built_items, total_lines)
        };

        let mut list_state = ListState::default();
        if !self.chat_history.is_empty() {
            list_state.select(Some(self.chat_history.len() - 1));
        }

        let list = List::new(items).style(Style::default().fg(Color::White));
        frame.render_stateful_widget(list, history_columns[0], &mut list_state);

        let visible_rows = usize::from(history_columns[0].height).max(1);
        let content_len = history_line_count.max(1);
        let scroll_position = content_len.saturating_sub(visible_rows);
        let mut scrollbar_state = ScrollbarState::new(content_len).position(scroll_position);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            history_columns[1],
            &mut scrollbar_state,
        );

        let local = self
            .local_selection
            .as_ref()
            .map(|selection| format!("{:?}: {}", selection.provider, selection.model))
            .unwrap_or_else(|| "(unset)".to_string());
        let frontier = self
            .frontier_selection
            .as_ref()
            .map(|selection| format!("{:?}: {}", selection.provider, selection.model))
            .unwrap_or_else(|| "(unset)".to_string());

        let lpm_values = self
            .loops_per_minute_history
            .iter()
            .map(|value| value.round().max(0.0) as u64)
            .collect::<Vec<u64>>();
        frame.render_widget(
            Sparkline::default()
                .block(Block::default().borders(Borders::ALL).title("Looper LPM"))
                .style(Style::default().fg(Color::Cyan))
                .data(&lpm_values),
            right[0],
        );

        frame.render_widget(
            Paragraph::new(format!(
                "local={local}\nfrontier={frontier}\nconfigured={}\nloop={} ({}ms)",
                self.configured, self.loop_running, self.loop_interval_ms
            ))
            .block(Block::default().borders(Borders::ALL).title("Model Config")),
            right[1],
        );

        let observability = &self.observability;
        let start_ago = format_elapsed_ago(self.started_at.elapsed());
        frame.render_widget(
            Paragraph::new(format!(
                "start={} ({} ago)\nloops={} ({:.2}/min)\nfailed_tool_exec={} ({:.1}%)\nfalse_positive_surprises={} ({:.1}%)",
                self.start_timestamp,
                start_ago,
                observability.total_iterations,
                observability.loops_per_minute,
                observability.failed_tool_executions,
                observability.failed_tool_execution_percent,
                observability.false_positive_surprises,
                observability.false_positive_surprise_percent,
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Execution Summary"),
            ),
            right[2],
        );

        frame.render_widget(
            Paragraph::new(format!(
                "{}\nstop_reason={}",
                self.latest_loop_state_log,
                self.stop_reason.as_deref().unwrap_or("(none)")
            ))
            .block(Block::default().borders(Borders::ALL).title("Loop State")),
            right[3],
        );

        frame.render_widget(
            Paragraph::new(self.chat_input.as_str())
                .style(Style::default().fg(Color::Yellow))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Chat Input (Enter send, Shift+Enter newline, Ctrl+V paste)"),
                ),
            left[1],
        );
    }

    fn push_log(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }

        self.activity_log.push(trimmed.to_string());
        if self.activity_log.len() > 200 {
            self.activity_log.drain(0..(self.activity_log.len() - 200));
        }
    }

    fn push_chat_history(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }

        self.chat_history.push(trimmed.to_string());
        if self.chat_history.len() > 200 {
            self.chat_history.drain(0..(self.chat_history.len() - 200));
        }
    }

    fn push_me_message(&mut self, message: &str) {
        self.push_chat_history(&format_chat_entry("[Me]", message));
    }

    fn push_looper_message(&mut self, message: &str) {
        self.push_chat_history(&format_chat_entry("[Looper]", message));
    }

    fn record_loops_per_minute(&mut self, loops_per_minute: f64) {
        self.loops_per_minute_history
            .push(loops_per_minute.max(0.0));
        if self.loops_per_minute_history.len() > 60 {
            self.loops_per_minute_history
                .drain(0..(self.loops_per_minute_history.len() - 60));
        }
    }
}

fn setup_line(label: &str, value: &str, active: bool) -> Line<'static> {
    let style = if active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::White)
    };
    Line::from(vec![
        Span::styled(format!("{label}: "), style),
        Span::styled(value.to_string(), Style::default().fg(Color::Gray)),
    ])
}

fn format_chat_entry(prefix: &str, message: &str) -> String {
    const CONTENT_OFFSET: usize = 10;
    let padding = " ".repeat(CONTENT_OFFSET.saturating_sub(prefix.chars().count()));
    let continuation_indent = " ".repeat(CONTENT_OFFSET);
    let normalized = message.replace('\n', &format!("\n{continuation_indent}"));
    format!("{prefix}{padding}{normalized}")
}

fn wrap_chat_entry_lines(entry: &str, max_width: usize) -> Vec<String> {
    let mut wrapped = Vec::new();
    for raw_line in entry.lines() {
        wrapped.extend(wrap_line_preserving_indent(raw_line, max_width));
    }

    if wrapped.is_empty() {
        wrapped.push(String::new());
    }

    wrapped
}

fn wrap_line_preserving_indent(line: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![String::new()];
    }

    let chars = line.chars().collect::<Vec<char>>();
    if chars.len() <= max_width {
        return vec![line.to_string()];
    }

    let indent_len = line
        .chars()
        .take_while(|ch| ch.is_ascii_whitespace())
        .count()
        .min(max_width.saturating_sub(1));
    let indent = " ".repeat(indent_len);
    let mut out = Vec::new();

    let mut index = 0;
    out.push(chars[index..(index + max_width)].iter().collect::<String>());
    index += max_width;

    let continuation_width = max_width.saturating_sub(indent_len).max(1);
    while index < chars.len() {
        let end = (index + continuation_width).min(chars.len());
        let segment = chars[index..end].iter().collect::<String>();
        out.push(format!("{indent}{segment}"));
        index = end;
    }

    out
}

fn styled_chat_history_line(line: &str) -> Line<'static> {
    for prefix in ["[Looper]", "[Me]"] {
        if let Some(rest) = line.strip_prefix(prefix) {
            let pad_len = rest.chars().take_while(|ch| *ch == ' ').count();
            let suffix = rest.chars().skip(pad_len).collect::<String>();

            if pad_len > 0 {
                let gray_fill_len = pad_len.saturating_sub(1);
                return Line::from(vec![
                    Span::raw(prefix.to_string()),
                    Span::styled(
                        ":".repeat(gray_fill_len),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::raw(" "),
                    Span::raw(suffix),
                ]);
            }
        }
    }

    let leading_spaces = line.chars().take_while(|ch| *ch == ' ').count();
    if leading_spaces == 0 {
        return Line::from(line.to_string());
    }

    let suffix = line.chars().skip(leading_spaces).collect::<String>();
    let gray_fill_len = leading_spaces.saturating_sub(1);
    Line::from(vec![
        Span::styled(
            ":".repeat(gray_fill_len),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(" "),
        Span::raw(suffix),
    ])
}

fn frontier_provider_options() -> [ModelProviderKind; 3] {
    [
        ModelProviderKind::OpenAi,
        ModelProviderKind::OpenCodeZen,
        ModelProviderKind::Ollama,
    ]
}

fn provider_label(provider: ModelProviderKind) -> &'static str {
    match provider {
        ModelProviderKind::Ollama => "Ollama",
        ModelProviderKind::OpenAi => "OpenAI",
        ModelProviderKind::OpenCodeZen => "OpenCode Zen",
    }
}

fn default_model_for_provider(provider: ModelProviderKind) -> &'static str {
    match provider {
        ModelProviderKind::Ollama => "gemma3:4b",
        ModelProviderKind::OpenAi => "gpt-5.2",
        ModelProviderKind::OpenCodeZen => "kimi-k2.5",
    }
}

fn normalize_api_key_value(raw: &str) -> String {
    let trimmed = raw.trim();
    let unprefixed = trimmed
        .strip_prefix("Bearer ")
        .or_else(|| trimmed.strip_prefix("bearer "))
        .unwrap_or(trimmed);
    let unquoted = unprefixed
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim();
    unquoted.to_string()
}

async fn list_openai_models(api_key: &str) -> Result<Vec<String>> {
    let api_key = normalize_api_key_value(api_key);
    if api_key.is_empty() {
        return Ok(Vec::new());
    }

    let response = reqwest::Client::new()
        .get("https://api.openai.com/v1/models")
        .bearer_auth(api_key)
        .send()
        .await?
        .error_for_status()?;

    let value = response.json::<serde_json::Value>().await?;
    let mut models = value
        .get("data")
        .and_then(|items| items.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(|id| id.as_str()))
                .map(ToString::to_string)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();

    models.sort();
    Ok(models)
}

async fn scrape_ollama_library_models() -> Result<Vec<String>> {
    let html = reqwest::Client::new()
        .get("https://ollama.com/library")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    Ok(parse_ollama_library_tagged_models(&html))
}

async fn scrape_ollama_library_base_models() -> Result<Vec<String>> {
    let html = reqwest::Client::new()
        .get("https://ollama.com/library")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    Ok(parse_ollama_library_base_models(&html))
}

async fn scrape_ollama_model_versions(model: &str) -> Result<Vec<String>> {
    let url = format!("https://ollama.com/library/{model}");
    let html = reqwest::Client::new()
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    Ok(parse_ollama_model_versions(&html, model))
}

fn parse_ollama_library_base_models(html: &str) -> Vec<String> {
    let mut models = Vec::new();
    let marker = "href=\"/library/";
    let mut cursor = 0usize;

    while let Some(rel) = html[cursor..].find(marker) {
        let start = cursor + rel + marker.len();
        let tail = &html[start..];
        let Some(end) = tail.find('"') else {
            break;
        };

        let candidate = tail[..end].trim();
        if candidate.is_empty() || candidate.contains(':') || candidate.contains('/') {
            cursor = start + end;
            continue;
        }

        models.push(candidate.to_string());
        cursor = start + end;
    }

    models.sort();
    models.dedup();
    models
}

fn parse_ollama_library_tagged_models(html: &str) -> Vec<String> {
    let bases = parse_ollama_library_base_models(html);
    let mut tagged = Vec::new();

    for base in bases {
        let tags = extract_size_tags_for_model_card(html, &base);
        if tags.is_empty() {
            tagged.push(format!("{base}:latest"));
        } else {
            for tag in tags {
                tagged.push(format!("{base}:{tag}"));
            }
        }
    }

    tagged.sort();
    tagged.dedup();
    tagged
}

fn parse_ollama_model_versions(html: &str, model: &str) -> Vec<String> {
    let href_marker = format!("href=\"/library/{model}:");
    let mut versions = Vec::new();
    let mut cursor = 0usize;

    while let Some(rel) = html[cursor..].find(&href_marker) {
        let start = cursor + rel + href_marker.len();
        let tail = &html[start..];
        let Some(end) = tail.find('"') else {
            break;
        };

        let version = tail[..end].trim().to_lowercase();
        if !version.is_empty() && !version.contains('/') {
            versions.push(version);
        }

        cursor = start + end;
    }

    versions.sort();
    versions.dedup();
    versions
}

fn extract_size_tags_for_model_card(html: &str, model: &str) -> Vec<String> {
    let anchor = format!("href=\"/library/{model}\"");
    let Some(model_pos) = html.find(&anchor) else {
        return Vec::new();
    };

    let block_start = html[..model_pos]
        .rfind("<li x-test-model")
        .unwrap_or(model_pos);
    let block_tail = &html[block_start..];
    let block_end_rel = block_tail.find("</li>").unwrap_or(block_tail.len());
    let block = &block_tail[..block_end_rel];

    let mut tags = Vec::new();
    let marker = "x-test-size";
    let mut cursor = 0usize;
    while let Some(rel) = block[cursor..].find(marker) {
        let marker_pos = cursor + rel;
        let Some(gt_rel) = block[marker_pos..].find('>') else {
            break;
        };
        let content_start = marker_pos + gt_rel + 1;
        let Some(lt_rel) = block[content_start..].find('<') else {
            break;
        };
        let value = block[content_start..content_start + lt_rel]
            .trim()
            .to_lowercase();
        if !value.is_empty() {
            tags.push(value);
        }
        cursor = content_start + lt_rel;
    }

    tags.sort();
    tags.dedup();
    tags
}

fn preferred_version_index(versions: &[String]) -> usize {
    versions
        .iter()
        .position(|item| item == "4b" || item == "8b" || item == "7b")
        .unwrap_or(0)
}

fn is_ollama_installed() -> bool {
    Command::new("ollama").arg("--version").output().is_ok()
}

fn install_ollama() -> Result<Vec<String>> {
    let output = run_command_capture("winget", &["install", "-e", "--id", "Ollama.Ollama"])?;
    if output.is_empty() {
        return Ok(vec!["winget completed with no output".to_string()]);
    }
    Ok(output)
}

fn pull_ollama_model(model: &str) -> Result<Vec<String>> {
    run_command_capture("ollama", &["pull", model])
}

fn run_command_capture(program: &str, args: &[&str]) -> Result<Vec<String>> {
    let output = Command::new(program).args(args).output()?;
    let status = output.status;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut lines = Vec::new();

    lines.push(format!("$ {} {}", program, args.join(" ")));
    lines.push(format!("status: {}", status));
    for line in stdout.lines() {
        lines.push(line.to_string());
    }
    for line in stderr.lines() {
        lines.push(format!("stderr: {line}"));
    }

    if !status.success() {
        return Err(anyhow!("command failed: {} {}", program, args.join(" ")));
    }
    Ok(lines)
}

fn list_installed_ollama_models() -> Result<Vec<String>> {
    let output = Command::new("ollama").arg("list").output()?;
    if !output.status.success() {
        return Err(anyhow!("failed to list local Ollama models"));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut models = Vec::new();
    for line in text.lines().skip(1) {
        if let Some(first) = line.split_whitespace().next()
            && !first.is_empty()
        {
            models.push(first.to_string());
        }
    }

    Ok(models)
}

fn model_is_installed(installed: &[String], target: &str) -> bool {
    installed
        .iter()
        .any(|item| item == target || item.starts_with(&format!("{target}:")))
}

fn mask(value: &str) -> String {
    if value.is_empty() {
        return "(empty)".to_string();
    }
    "*".repeat(value.len().min(8))
}

fn visible_window(selected: usize, total: usize, capacity: usize) -> (usize, usize) {
    if total <= capacity {
        return (0, total);
    }

    let half = capacity / 2;
    let mut start = selected.saturating_sub(half);
    let mut end = start + capacity;
    if end > total {
        end = total;
        start = end.saturating_sub(capacity);
    }

    (start, end)
}

fn split_model_and_version(model: &str) -> (&str, &str) {
    model.split_once(':').unwrap_or((model, "latest"))
}

fn format_start_timestamp(now: chrono::DateTime<Local>) -> String {
    let formatted = now.format("%b %-d, %-I:%M%P").to_string();
    if let Some(trimmed) = formatted.strip_suffix("am") {
        return format!("{trimmed}a");
    }
    if let Some(trimmed) = formatted.strip_suffix("pm") {
        return format!("{trimmed}p");
    }
    formatted
}

fn format_elapsed_ago(elapsed: Duration) -> String {
    let total_seconds = elapsed.as_secs();
    let units = [
        (86_400_u64, "day"),
        (3_600_u64, "hour"),
        (60_u64, "min"),
        (1_u64, "sec"),
    ];

    let mut remaining = total_seconds;
    let mut parts = Vec::new();

    for (unit_seconds, unit_name) in units {
        if parts.len() == 2 {
            break;
        }

        let count = remaining / unit_seconds;
        if count == 0 {
            continue;
        }

        remaining %= unit_seconds;
        let suffix = if count == 1 { "" } else { "s" };
        parts.push(format!("{count} {unit_name}{suffix}"));
    }

    if parts.is_empty() {
        return "0 secs".to_string();
    }

    parts.join(", ")
}

fn read_persisted_setup_config() -> Result<Option<PersistedSetupConfig>> {
    let path = terminal_setup_config_path();
    if !path.exists() {
        return Ok(None);
    }

    let raw = fs::read_to_string(path)?;
    let config = serde_json::from_str::<PersistedSetupConfig>(&raw)?;
    Ok(Some(config))
}

fn write_persisted_setup_config(config: &PersistedSetupConfig) -> Result<()> {
    let path = terminal_setup_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let raw = serde_json::to_string_pretty(config)?;
    fs::write(path, raw)?;
    Ok(())
}

fn terminal_setup_config_path() -> PathBuf {
    user_looper_dir().join("terminal-setup.json")
}

fn terminal_log_path() -> PathBuf {
    user_looper_dir().join("terminal.log")
}

fn append_terminal_log(message: &str) {
    if message.trim().is_empty() {
        return;
    }

    let _ = append_terminal_log_inner(terminal_log_path().as_path(), message);
}

fn clear_terminal_log() {
    let path = terminal_log_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if path.exists() {
        let _ = fs::remove_file(path);
    }
}

fn append_terminal_log_inner(path: &Path, message: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    writeln!(file, "[{timestamp}] {message}")?;
    Ok(())
}

fn user_looper_dir() -> PathBuf {
    if let Some(home) = user_home_dir() {
        return home.join(".looper");
    }

    std::env::temp_dir().join(".looper")
}

fn user_home_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        return std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .or_else(|| {
                let drive = std::env::var_os("HOMEDRIVE")?;
                let path = std::env::var_os("HOMEPATH")?;
                Some(PathBuf::from(format!(
                    "{}{}",
                    drive.to_string_lossy(),
                    path.to_string_lossy()
                )))
            })
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
    }

    std::env::var_os("HOME").map(PathBuf::from)
}
