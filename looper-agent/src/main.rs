use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use looper_agent::{
    AppState, LooperRuntime, auto_start_loop_if_configured, build_router, initialize_sensor_ingress,
};

#[tokio::main]
async fn main() -> Result<()> {
    let config = AgentConfig::from_env_and_args(std::env::args().skip(1).collect())?;
    let workspace_root = config.workspace_root.clone();
    let runtime = LooperRuntime::with_internal_defaults_for_workspace(workspace_root)?;
    let state = AppState::new(runtime);
    if let Err(error) = initialize_sensor_ingress(&state).await {
        println!("sensor ingress initialization failed: {error}");
    }
    match auto_start_loop_if_configured(&state).await {
        Ok(Some(status)) => {
            println!(
                "auto-started loop: running={} interval_ms={}",
                status.running, status.interval_ms
            );
        }
        Ok(None) => {
            println!("auto-start skipped: no persisted model configuration found");
        }
        Err(error) => {
            println!("auto-start failed: {error}");
        }
    }

    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.bind_addr))?;
    println!("looper-agent listening on http://{}", config.bind_addr);
    println!("workspace root: {}", config.workspace_root.display());
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Debug)]
struct AgentConfig {
    bind_addr: SocketAddr,
    workspace_root: PathBuf,
}

impl AgentConfig {
    fn from_env_and_args(args: Vec<String>) -> Result<Self> {
        let mut bind_raw =
            std::env::var("LOOPER_AGENT_BIND").unwrap_or_else(|_| "127.0.0.1:10001".to_string());
        let mut workspace_raw = std::env::var("LOOPER_WORKSPACE_ROOT").ok();

        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--bind" => {
                    let value = args
                        .get(index + 1)
                        .ok_or_else(|| anyhow!("--bind requires a value"))?;
                    bind_raw = value.clone();
                    index += 2;
                }
                "--workspace" => {
                    let value = args
                        .get(index + 1)
                        .ok_or_else(|| anyhow!("--workspace requires a value"))?;
                    workspace_raw = Some(value.clone());
                    index += 2;
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => {
                    return Err(anyhow!("unknown argument: {other}"));
                }
            }
        }

        let workspace_root = workspace_raw
            .map(PathBuf::from)
            .unwrap_or_else(default_workspace_root);

        std::fs::create_dir_all(&workspace_root).with_context(|| {
            format!(
                "failed to create workspace directory: {}",
                workspace_root.display()
            )
        })?;

        let workspace_root = workspace_root
            .canonicalize()
            .context("failed to resolve workspace path")?;
        if !workspace_root.is_dir() {
            return Err(anyhow!(
                "workspace path must be a directory: {}",
                workspace_root.display()
            ));
        }

        Ok(Self {
            bind_addr: bind_raw.parse().context("invalid bind address")?,
            workspace_root,
        })
    }
}

fn default_workspace_root() -> PathBuf {
    match user_home_dir() {
        Some(home) => home.join(".looper").join("workspace"),
        None => std::env::temp_dir().join(".looper").join("workspace"),
    }
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
            });
    }

    std::env::var_os("HOME").map(PathBuf::from)
}

fn print_help() {
    println!("looper-agent");
    println!();
    println!("Usage:");
    println!("  looper-agent [--workspace <path>] [--bind 127.0.0.1:10001]");
    println!();
    println!("Environment variables:");
    println!(
        "  LOOPER_WORKSPACE_ROOT   Workspace directory for shell/glob/grep tools (default: ~/.looper/workspace)"
    );
    println!("  LOOPER_AGENT_BIND       HTTP bind address (default: 127.0.0.1:10001)");
}
