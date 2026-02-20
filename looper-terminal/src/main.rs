use anyhow::Result;
use looper_harness::{ExecutionResult, LooperRuntime};

fn main() -> Result<()> {
    let mut runtime = LooperRuntime::with_internal_defaults()?;

    let message = std::env::args().skip(1).collect::<Vec<String>>().join(" ");
    if message.is_empty() {
        println!("looper-terminal first pass");
        println!("usage: cargo run -p looper-terminal -- <message>");
        return Ok(());
    }

    runtime.enqueue_chat_message(message)?;
    let report = runtime.run_iteration()?;

    println!("sensed percepts: {}", report.sensed_percepts.len());
    println!("surprising percepts: {}", report.surprising_percepts.len());
    println!("planned actions: {}", report.planned_actions.len());

    for (index, result) in report.action_results.iter().enumerate() {
        match result {
            ExecutionResult::Executed { output } => {
                println!("action {}: executed", index + 1);
                println!("output:\n{output}");
            }
            ExecutionResult::Denied(reason) => {
                println!("action {}: denied ({reason})", index + 1);
            }
            ExecutionResult::RequiresHitl => {
                println!("action {}: requires HITL", index + 1);
            }
        }
    }

    let metrics = runtime.observability();
    println!("iterations: {}", metrics.total_iterations);
    println!("failed executions: {}", metrics.failed_tool_executions);
    println!("loops per minute: {:.2}", metrics.loops_per_minute());

    Ok(())
}
