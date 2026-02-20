use anyhow::Result;
use looper_harness::{AppState, ExecutionResult, LooperRuntime, build_router};

#[tokio::main]
async fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<String>>();
    if matches!(args.first().map(String::as_str), Some("serve")) {
        return run_server().await;
    }

    let mut runtime = LooperRuntime::with_internal_defaults()?;

    let message = args.join(" ");
    if message.is_empty() {
        println!("looper-terminal first pass");
        println!("usage:");
        println!("  cargo run -p looper-terminal -- serve");
        println!("  cargo run -p looper-terminal -- <message>");
        return Ok(());
    }

    runtime.enqueue_chat_message(message)?;
    let report = runtime.run_iteration().await?;

    println!("sensed percepts: {}", report.sensed_percepts.len());
    println!("surprising percepts: {}", report.surprising_percepts.len());
    println!("planned actions: {}", report.planned_actions.len());
    if let Some(iteration_id) = report.iteration_id {
        println!("iteration id: {iteration_id}");
    }

    for (index, result) in report.action_results.iter().enumerate() {
        match result {
            ExecutionResult::Executed { output } => {
                println!("action {}: executed", index + 1);
                println!("output:\n{output}");
            }
            ExecutionResult::Denied(reason) => {
                println!("action {}: denied ({reason})", index + 1);
            }
            ExecutionResult::RequiresHitl { approval_id } => {
                println!(
                    "action {}: requires HITL (approval id: {approval_id})",
                    index + 1
                );
            }
        }
    }

    let metrics = runtime.observability();
    println!("iterations: {}", metrics.total_iterations);
    println!("failed executions: {}", metrics.failed_tool_executions);
    println!("loops per minute: {:.2}", metrics.loops_per_minute());

    Ok(())
}

async fn run_server() -> Result<()> {
    let runtime = LooperRuntime::with_internal_defaults()?;
    let state = AppState::new(runtime);
    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:10001").await?;
    println!("looper-terminal server listening on http://127.0.0.1:10001");
    axum::serve(listener, app).await?;
    Ok(())
}
