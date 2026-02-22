export type ProcessStatus = {
  configured: boolean;
  provider: string | null;
  model: string | null;
  process_state: string;
};

export type DashboardPayload = {
  state: {
    state: string;
    reason: string | null;
    configured: boolean;
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
      | "performing_actions"
      | null;
    surprise_found: boolean;
    action_required: boolean;
    local_loop_count: number;
    frontier_loop_count: number;
    current_phase:
      | "gather_new_percepts"
      | "check_for_surprises"
      | "deeper_percept_investigation"
      | "plan_actions"
      | "execute_actions"
      | "idle";
    current_phase_started_at_unix_ms: number;
  };
  local_model: ProcessStatus;
  frontier_model: ProcessStatus;
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

export type DashboardResponse = {
  type: "event" | "response";
  event?: string;
  data?: DashboardPayload;
  id?: number;
  ok?: boolean;
  result?: unknown;
  error?: string;
};

export type LoopPhaseTransitionPayload = {
  sequence: number;
  phase: DashboardPayload["loop_visualization"]["current_phase"];
  loop_visualization: DashboardPayload["loop_visualization"];
  emitted_at_unix_ms: number;
};

export type EditableSensor = {
  id: string;
  name: string;
  policy: string;
  recentPercepts: string[];
};

export type EditableActuator = {
  id: string;
  name: string;
  policy: string;
  recentActions: string[];
};

export type Provider = "ollama" | "open_ai" | "open_code_zen";

export type SetupStepId =
  | "local_provider"
  | "local_model"
  | "local_model_version"
  | "frontier_provider"
  | "frontier_api_key"
  | "frontier_model"
  | "install_ollama"
  | "install_model";

export const SETUP_STEPS: Array<{ id: SetupStepId; label: string }> = [
  { id: "local_provider", label: "1. Select a local provider" },
  { id: "local_model", label: "2. Select a local model" },
  { id: "local_model_version", label: "2a. Select a model version" },
  { id: "frontier_provider", label: "3. Select a frontier provider" },
  { id: "frontier_api_key", label: "3a. Add API key" },
  { id: "frontier_model", label: "4. Select a frontier model" },
  { id: "install_ollama", label: "5. Install Ollama" },
  { id: "install_model", label: "6. Install selected model(s)" },
];
