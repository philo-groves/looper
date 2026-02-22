import { NextResponse } from "next/server";

export const dynamic = "force-dynamic";
export const revalidate = 0;

type AgentDashboardPayload = {
  state: {
    state: "setup" | "running" | "stopped";
    reason: string | null;
    configured: boolean;
    local_selection: { provider: string; model: string } | null;
    frontier_selection: { provider: string; model: string } | null;
    latest_iteration_id: number | null;
  };
  loop_status: {
    running: boolean;
    interval_ms: number;
  };
  observability: {
    total_iterations: number;
    loops_per_minute: number;
    local_model_tokens: number;
    frontier_model_tokens: number;
    failed_tool_execution_percent: number;
  };
  loop_visualization: {
    local_current_step:
      | "gather_new_percepts"
      | "check_for_surprises"
      | "no_surprise"
      | "surprise_found";
    frontier_current_step:
      | "deeper_percept_investigation"
      | "plan_actions"
      | "no_action_required"
      | null;
    surprise_found: boolean;
    action_required: boolean;
    local_loop_count: number;
    frontier_loop_count: number;
  };
  local_model: {
    configured: boolean;
    provider: string | null;
    model: string | null;
    process_state: string;
  };
  frontier_model: {
    configured: boolean;
    provider: string | null;
    model: string | null;
    process_state: string;
  };
  sensors: Array<{
    name: string;
    description: string;
    enabled: boolean;
    sensitivity_score: number;
    queued_percepts: number;
    unread_percepts: number;
  }>;
  actuators: Array<{
    name: string;
    description: string;
    kind: string;
    require_hitl: boolean;
    sandboxed: boolean;
    allowlist_count: number;
    denylist_count: number;
    rate_limit: { max: number; per: string } | null;
  }>;
  pending_approval_count: number;
};

export async function GET() {
  const agentBaseUrl = process.env.LOOPER_AGENT_URL ?? "http://127.0.0.1:10001";

  try {
    const response = await fetch(`${agentBaseUrl}/api/dashboard`, {
      cache: "no-store",
    });

    if (!response.ok) {
      return NextResponse.json(
        {
          connected: false,
          error: `Agent request failed with status ${response.status}`,
        },
        { status: 200 },
      );
    }

    const data = (await response.json()) as AgentDashboardPayload;
    return NextResponse.json({
      connected: true,
      updated_at: Date.now(),
      dashboard: data,
    });
  } catch {
    return NextResponse.json(
      {
        connected: false,
        error: `Cannot reach looper-agent at ${agentBaseUrl}`,
      },
      { status: 200 },
    );
  }
}
