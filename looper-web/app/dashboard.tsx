"use client";

import { useEffect, useState } from "react";

type ProcessStatus = {
  configured: boolean;
  provider: string | null;
  model: string | null;
  process_state: string;
};

type DashboardPayload = {
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

type DashboardResponse = {
  connected: boolean;
  updated_at?: number;
  error?: string;
  dashboard?: DashboardPayload;
};

function statusTone(value: string): string {
  if (value === "running") {
    return "status status-ok";
  }
  if (value === "idle") {
    return "status status-warn";
  }
  if (value === "stopped") {
    return "status status-off";
  }
  return "status";
}

export function Dashboard() {
  const [data, setData] = useState<DashboardResponse | null>(null);

  useEffect(() => {
    let active = true;

    async function fetchSnapshot() {
      try {
        const response = await fetch("/api/dashboard", { cache: "no-store" });
        const payload = (await response.json()) as DashboardResponse;
        if (active) {
          setData(payload);
        }
      } catch {
        if (active) {
          setData({ connected: false, error: "Failed to fetch dashboard." });
        }
      }
    }

    void fetchSnapshot();
    const timer = setInterval(() => {
      void fetchSnapshot();
    }, 1500);

    return () => {
      active = false;
      clearInterval(timer);
    };
  }, []);

  const updatedAtText = data?.updated_at
    ? new Date(data.updated_at).toLocaleTimeString()
    : "No live data yet";

  const snapshot = data?.dashboard;

  return (
    <main className="dashboard-shell">
      <header className="dashboard-header">
        <div>
          <p className="eyebrow">Looper Control Surface</p>
          <h1>Agent Dashboard</h1>
        </div>
        <div className="header-status">
          <span className={data?.connected ? "status status-ok" : "status status-off"}>
            {data?.connected ? "Agent connected" : "Agent offline"}
          </span>
          <span className="muted">Updated {updatedAtText}</span>
        </div>
      </header>

      <section className="dashboard-grid">
        <article className="panel">
          <h2>Sensors</h2>
          {snapshot?.sensors?.length ? (
            snapshot.sensors.map((sensor) => (
              <div key={sensor.name} className="item-card">
                <div className="item-head">
                  <strong>{sensor.name}</strong>
                  <span className={sensor.enabled ? "status status-ok" : "status status-off"}>
                    {sensor.enabled ? "enabled" : "disabled"}
                  </span>
                </div>
                <p>{sensor.description}</p>
                <div className="stat-row">
                  <span>Unread {sensor.unread_percepts}</span>
                  <span>Queued {sensor.queued_percepts}</span>
                  <span>Sensitivity {sensor.sensitivity_score}</span>
                </div>
              </div>
            ))
          ) : (
            <p className="muted">No sensors registered.</p>
          )}
        </article>

        <article className="panel panel-center">
          <h2>Model Processes</h2>
          <div className="item-card">
            <div className="item-head">
              <strong>Local Model</strong>
              <span className={statusTone(snapshot?.local_model.process_state ?? "")}>{snapshot?.local_model.process_state ?? "unknown"}</span>
            </div>
            <p>
              {(snapshot?.local_model.provider ?? "unconfigured").toString()} / {snapshot?.local_model.model ?? "-"}
            </p>
          </div>

          <div className="item-card">
            <div className="item-head">
              <strong>Frontier Model</strong>
              <span className={statusTone(snapshot?.frontier_model.process_state ?? "")}>{snapshot?.frontier_model.process_state ?? "unknown"}</span>
            </div>
            <p>
              {(snapshot?.frontier_model.provider ?? "unconfigured").toString()} / {snapshot?.frontier_model.model ?? "-"}
            </p>
          </div>

          <div className="item-card">
            <div className="item-head">
              <strong>Loop State</strong>
              <span className={snapshot?.loop_status.running ? "status status-ok" : "status status-warn"}>
                {snapshot?.loop_status.running ? "running" : "paused"}
              </span>
            </div>
            <div className="stat-row">
              <span>Interval {snapshot?.loop_status.interval_ms ?? 0}ms</span>
              <span>Iterations {snapshot?.observability.total_iterations ?? 0}</span>
              <span>LPM {(snapshot?.observability.loops_per_minute ?? 0).toFixed(2)}</span>
            </div>
            <div className="stat-row">
              <span>Local tokens {snapshot?.observability.local_model_tokens ?? 0}</span>
              <span>Frontier tokens {snapshot?.observability.frontier_model_tokens ?? 0}</span>
              <span>Approvals {snapshot?.pending_approval_count ?? 0}</span>
            </div>
            <p className="muted">
              Agent state: {snapshot?.state.state ?? "unknown"}
              {snapshot?.state.reason ? ` (${snapshot.state.reason})` : ""}
            </p>
          </div>
        </article>

        <article className="panel">
          <h2>Actuators</h2>
          {snapshot?.actuators?.length ? (
            snapshot.actuators.map((actuator) => (
              <div key={actuator.name} className="item-card">
                <div className="item-head">
                  <strong>{actuator.name}</strong>
                  <span className="status">{actuator.kind}</span>
                </div>
                <p>{actuator.description}</p>
                <div className="stat-row">
                  <span>HITL {actuator.require_hitl ? "yes" : "no"}</span>
                  <span>Sandbox {actuator.sandboxed ? "yes" : "no"}</span>
                  <span>Allow {actuator.allowlist_count}</span>
                  <span>Deny {actuator.denylist_count}</span>
                </div>
                {actuator.rate_limit ? (
                  <p className="muted">
                    Rate limit: {actuator.rate_limit.max} / {actuator.rate_limit.per}
                  </p>
                ) : null}
              </div>
            ))
          ) : (
            <p className="muted">No actuators registered.</p>
          )}
        </article>
      </section>

      {!data?.connected && data?.error ? <p className="error-line">{data.error}</p> : null}
    </main>
  );
}
