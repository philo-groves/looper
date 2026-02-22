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

type EditableSensor = {
  id: string;
  name: string;
  policy: string;
  recentPercepts: string[];
};

type EditableActuator = {
  id: string;
  name: string;
  policy: string;
  recentActions: string[];
};

const LOCAL_STEPS = ["Gather New Percepts", "Check For Surprises", "No Surprise"];
const FRONTIER_STEPS = [
  "Deeper Percept Investigation",
  "Plan Actions",
  "No Action Required",
];

function localStepIndex(
  step: DashboardPayload["loop_visualization"]["local_current_step"] | undefined,
): number | null {
  if (step === "gather_new_percepts") {
    return 0;
  }
  if (step === "check_for_surprises" || step === "surprise_found") {
    return 1;
  }
  if (step === "no_surprise") {
    return 2;
  }
  return null;
}

function frontierStepIndex(
  step: DashboardPayload["loop_visualization"]["frontier_current_step"] | undefined,
): number | null {
  if (step === "deeper_percept_investigation") {
    return 0;
  }
  if (step === "plan_actions") {
    return 1;
  }
  if (step === "no_action_required") {
    return 2;
  }
  return null;
}

function pointOnCircle(cx: number, cy: number, radius: number, angleDegrees: number) {
  const angle = ((angleDegrees - 90) * Math.PI) / 180;
  return {
    x: cx + radius * Math.cos(angle),
    y: cy + radius * Math.sin(angle),
  };
}

function arcPath(radius: number, startAngle: number, endAngle: number) {
  const center = 160;
  const start = pointOnCircle(center, center, radius, startAngle);
  const end = pointOnCircle(center, center, radius, endAngle);
  const largeArc = endAngle - startAngle <= 180 ? 0 : 1;
  return `M ${start.x} ${start.y} A ${radius} ${radius} 0 ${largeArc} 1 ${end.x} ${end.y}`;
}

function defaultPercepts(name: string): string[] {
  return [
    `${name}: incoming percept`,
    `${name}: incoming percept`,
    `${name}: incoming percept`,
  ];
}

function defaultActions(name: string): string[] {
  return [`${name}: queued action`, `${name}: queued action`, `${name}: queued action`];
}

function mergeSensors(
  existing: EditableSensor[],
  incoming: DashboardPayload["sensors"],
): EditableSensor[] {
  const mapped = incoming.map((sensor) => {
    const match = existing.find((item) => item.id === sensor.name);
    return (
      match ?? {
        id: sensor.name,
        name: sensor.name,
        policy: `Sensitivity: ${sensor.sensitivity_score}%`,
        recentPercepts: defaultPercepts(sensor.name),
      }
    );
  });

  return mapped.length > 0 ? mapped : existing;
}

function mergeActuators(
  existing: EditableActuator[],
  incoming: DashboardPayload["actuators"],
): EditableActuator[] {
  const mapped = incoming.map((actuator) => {
    const match = existing.find((item) => item.id === actuator.name);
    return (
      match ?? {
        id: actuator.name,
        name: actuator.name,
        policy: `HITL: ${actuator.require_hitl ? "Yes" : "No"}`,
        recentActions: defaultActions(actuator.name),
      }
    );
  });

  return mapped.length > 0 ? mapped : existing;
}

function statusPill(connected: boolean) {
  return connected
    ? "border border-green-700 bg-zinc-100 text-zinc-900 dark:bg-zinc-900 dark:text-zinc-100"
    : "border border-red-700 bg-red-600 text-white";
}

function LoopRing({
  title,
  modelLabel,
  steps,
  activeStep,
  totalLoops,
}: {
  title: string;
  modelLabel: string;
  steps: string[];
  activeStep: number | null;
  totalLoops: number;
}) {
  const ringRadius = 122;
  const labelRadius = 145;
  const segmentGap = 8;

  return (
    <article className="rounded-2xl border border-zinc-300 bg-white p-4 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
      <h3 className="mb-3 text-center text-base font-semibold">{title}</h3>
      <p className="mb-3 text-center text-xs text-zinc-600 dark:text-zinc-300">Total loops: {totalLoops}</p>
      <div className="relative mx-auto h-80 w-80">
        <svg viewBox="0 0 320 320" className="h-full w-full">
          {steps.map((step, index) => {
            const baseStart = index * 120;
            const start = baseStart + segmentGap;
            const end = baseStart + 120 - segmentGap;
            const isActive = activeStep === index;

            return (
              <path
                key={step}
                d={arcPath(ringRadius, start, end)}
                fill="none"
                strokeLinecap="round"
                className={
                  isActive
                    ? "stroke-[14] text-black transition-colors duration-500 dark:text-white"
                    : "stroke-[14] text-zinc-300 transition-colors duration-500 dark:text-zinc-700"
                }
                stroke="currentColor"
              />
            );
          })}
        </svg>

        <div className="absolute inset-0 flex items-center justify-center">
          <div className="flex h-44 w-44 items-center justify-center rounded-full border border-zinc-300 bg-zinc-50 px-6 text-center text-sm font-medium dark:border-zinc-700 dark:bg-zinc-900">
            {modelLabel}
          </div>
        </div>

        {steps.map((step, index) => {
          const angle = index * 120 + 60;
          const point = pointOnCircle(160, 160, labelRadius, angle);
          const isActive = activeStep === index;

          return (
            <div
              key={`${step}-label`}
              className={`absolute w-32 -translate-x-1/2 -translate-y-1/2 rounded-xl border px-2 py-1 text-center text-xs leading-tight transition-colors duration-500 ${
                isActive
                  ? "border-zinc-300 bg-zinc-100 text-zinc-900 dark:border-zinc-600 dark:bg-zinc-800 dark:text-zinc-50"
                  : "border-zinc-200 bg-white text-zinc-600 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300"
              }`}
              style={{ left: point.x, top: point.y }}
            >
              {step}
            </div>
          );
        })}
      </div>
    </article>
  );
}

export function Dashboard() {
  const [theme, setTheme] = useState<"light" | "dark">(() => {
    if (typeof window === "undefined") {
      return "light";
    }

    const stored = window.localStorage.getItem("looper-theme");
    if (stored === "dark" || stored === "light") {
      return stored;
    }

    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  });
  const [data, setData] = useState<DashboardResponse | null>(null);
  const [sensors, setSensors] = useState<EditableSensor[]>([]);
  const [actuators, setActuators] = useState<EditableActuator[]>([]);
  const [isSidebarOpen, setIsSidebarOpen] = useState(true);

  useEffect(() => {
    document.documentElement.classList.toggle("dark", theme === "dark");
    window.localStorage.setItem("looper-theme", theme);
  }, [theme]);

  useEffect(() => {
    let active = true;

    async function fetchSnapshot() {
      try {
        const response = await fetch("/api/dashboard", { cache: "no-store" });
        const payload = (await response.json()) as DashboardResponse;
        if (active) {
          setData(payload);
          const dashboard = payload.dashboard;
          if (dashboard) {
            setSensors((existing) => mergeSensors(existing, dashboard.sensors));
            setActuators((existing) => mergeActuators(existing, dashboard.actuators));
          }
        }
      } catch {
        if (active) {
          setData({ connected: false, error: "Failed to fetch dashboard." });
        }
      }
    }

    void fetchSnapshot();
    const timer = window.setInterval(() => {
      void fetchSnapshot();
    }, 1500);

    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, []);

  const snapshot = data?.dashboard;

  const loopState = snapshot?.loop_visualization;

  const localModelLabel = `${snapshot?.local_model.provider ?? "Local"} / ${snapshot?.local_model.model ?? "Unassigned"}`;
  const frontierModelLabel = `${snapshot?.frontier_model.provider ?? "Frontier"} / ${snapshot?.frontier_model.model ?? "Unassigned"}`;

  return (
    <main className="min-h-screen w-full bg-zinc-100 text-zinc-900 dark:bg-black dark:text-zinc-100">
      <div className="flex min-h-screen w-full">
        <aside
          className={`shrink-0 border-r border-zinc-300 bg-white transition-all duration-300 dark:border-zinc-800 dark:bg-zinc-950 ${
            isSidebarOpen ? "w-72" : "w-16"
          }`}
        >
          <div className="flex items-center justify-between border-b border-zinc-300 p-3 dark:border-zinc-800">
            {isSidebarOpen ? <p className="text-sm font-semibold">Looper Workspace</p> : <span className="text-xs">Nav</span>}
            <button
              type="button"
              onClick={() => setIsSidebarOpen((current) => !current)}
              className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 text-xs font-medium dark:border-zinc-700 dark:bg-zinc-900"
            >
              {isSidebarOpen ? "Collapse" : "Expand"}
            </button>
          </div>

          <nav className="p-3 text-sm">
            {isSidebarOpen ? (
              <ul className="space-y-3">
                <li className="relative rounded-md bg-zinc-200 px-2 py-1 pl-4 font-medium dark:bg-zinc-800">
                  <span className="absolute inset-y-0 left-0 w-1 rounded-l-md bg-zinc-500 dark:bg-zinc-400" />
                  Dashboard
                </li>
                <li>
                  <p className="rounded-md px-2 py-1 font-medium">Conversations</p>
                  <ul className="mt-1 space-y-1 pl-4 text-zinc-600 dark:text-zinc-300">
                    <li className="rounded-md px-2 py-1">New Chat</li>
                    <li className="rounded-md px-2 py-1">Chat History</li>
                  </ul>
                </li>
                <li>
                  <p className="rounded-md px-2 py-1 font-medium">Sensors</p>
                  <ul className="mt-1 space-y-1 pl-4 text-zinc-600 dark:text-zinc-300">
                    <li className="rounded-md px-2 py-1">Add a Sensor</li>
                    <li className="rounded-md px-2 py-1">All Sensors</li>
                    <li className="rounded-md px-2 py-1">Percept History</li>
                  </ul>
                </li>
                <li>
                  <p className="rounded-md px-2 py-1 font-medium">Actuators</p>
                  <ul className="mt-1 space-y-1 pl-4 text-zinc-600 dark:text-zinc-300">
                    <li className="rounded-md px-2 py-1">Add an Actuator</li>
                    <li className="rounded-md px-2 py-1">All Actuators</li>
                    <li className="rounded-md px-2 py-1">Action History</li>
                  </ul>
                </li>
                <li>
                  <p className="rounded-md px-2 py-1 font-medium">Agent Settings</p>
                  <ul className="mt-1 space-y-1 pl-4 text-zinc-600 dark:text-zinc-300">
                    <li className="rounded-md px-2 py-1">Agent Identity</li>
                    <li className="rounded-md px-2 py-1">Loop Configuration</li>
                    <li className="rounded-md px-2 py-1">Providers &amp; Models</li>
                  </ul>
                </li>
              </ul>
            ) : (
              <ul className="space-y-2 text-center text-xs font-medium">
                <li className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">D</li>
                <li className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">C</li>
                <li className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">S</li>
                <li className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">A</li>
                <li className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">G</li>
              </ul>
            )}
          </nav>
        </aside>

        <div className="flex min-w-0 flex-1 flex-col gap-5">
          <header className="w-full border-b py-3 px-4 sm:px-6 border-zinc-300 bg-white dark:border-zinc-700 dark:bg-zinc-950">
            <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
              <span>{/* Add "Looper Workspace" here only when the sidenav is collapsed */}</span>

              <div className="flex items-center gap-3">
                <span className={`rounded-full px-3 py-1 text-xs font-medium ${statusPill(Boolean(data?.connected))}`}>
                  {data?.connected ? "Agent Connected" : "Agent Offline"}
                </span>
                <button
                  type="button"
                  onClick={() => setTheme((current) => (current === "light" ? "dark" : "light"))}
                  className="rounded-lg border border-zinc-300 bg-zinc-100 px-3 py-1 text-xs font-medium transition hover:bg-zinc-200 dark:border-zinc-700 dark:bg-zinc-900 dark:hover:bg-zinc-800"
                >
                  {theme === "light" ? "Switch to Dark" : "Switch to Light"}
                </button>
              </div>
            </div>
          </header>

          <section className="grid gap-5 px-4 pb-4 sm:px-6 sm:pb-6 lg:grid-cols-12">
          <article className="rounded-2xl border border-zinc-300 bg-white p-4 shadow-sm dark:border-zinc-700 dark:bg-zinc-950 lg:col-span-3">
            <h2 className="text-lg font-semibold">Sensors</h2>
            <button
              type="button"
              onClick={() => {
                const next = sensors.length + 1;
                setSensors((current) => [
                  ...current,
                  {
                    id: `sensor-${Date.now()}`,
                    name: `New Sensor ${next}`,
                    policy: "Sensitivity: 50%",
                    recentPercepts: ["New percept", "New percept", "New percept"],
                  },
                ]);
              }}
              className="mt-3 w-full rounded-lg border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium transition hover:bg-zinc-200 dark:border-zinc-700 dark:bg-zinc-900 dark:hover:bg-zinc-800"
            >
              Add a Sensor
            </button>

            <div className="mt-4 space-y-3">
              {sensors.length === 0 ? (
                <p className="rounded-lg border border-zinc-200 bg-zinc-50 p-3 text-sm text-zinc-600 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300">
                  No sensors registered.
                </p>
              ) : (
                sensors.map((sensor) => (
                  <div
                    key={sensor.id}
                    className="rounded-xl border border-zinc-300 bg-zinc-50 p-3 dark:border-zinc-700 dark:bg-zinc-900"
                  >
                    <div className="flex items-center justify-between gap-3">
                      <p className="text-sm font-semibold">{sensor.name}</p>
                      <button
                        type="button"
                        className="rounded-md border border-zinc-300 bg-white px-2 py-1 text-xs font-medium transition hover:bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-950 dark:hover:bg-zinc-800"
                      >
                        Edit
                      </button>
                    </div>
                    <p className="mt-2 text-xs font-medium uppercase tracking-wide text-zinc-500 dark:text-zinc-400">
                      Sensor Policy
                    </p>
                    <p className="mt-1 rounded-md border border-zinc-300 bg-white px-2 py-1 text-sm dark:border-zinc-700 dark:bg-zinc-950">
                      {sensor.policy}
                    </p>
                    <p className="mt-2 text-xs font-medium uppercase tracking-wide text-zinc-500 dark:text-zinc-400">
                      Recent Percepts
                    </p>
                    <div className="mt-1 space-y-1.5">
                      {sensor.recentPercepts.slice(0, 3).map((percept, perceptIndex) => (
                        <p
                          key={`${sensor.id}-percept-${perceptIndex}`}
                          className="w-full rounded-md border border-zinc-300 bg-white px-2 py-1 text-sm dark:border-zinc-700 dark:bg-zinc-950"
                        >
                          {percept}
                        </p>
                      ))}
                    </div>
                  </div>
                ))
              )}
            </div>
          </article>

          <section className="space-y-4 rounded-2xl border border-zinc-300 bg-white p-4 shadow-sm dark:border-zinc-700 dark:bg-zinc-950 lg:col-span-6">
            <h2 className="text-lg font-semibold">Looper State</h2>
            <div className="grid gap-4 xl:grid-cols-2">
              <LoopRing
                title="Local Model Loop"
                modelLabel={localModelLabel}
                steps={LOCAL_STEPS}
                activeStep={localStepIndex(loopState?.local_current_step)}
                totalLoops={loopState?.local_loop_count ?? 0}
              />
              <LoopRing
                title="Frontier Model Loop"
                modelLabel={frontierModelLabel}
                steps={FRONTIER_STEPS}
                activeStep={frontierStepIndex(loopState?.frontier_current_step)}
                totalLoops={loopState?.frontier_loop_count ?? 0}
              />
            </div>

            <div className="grid gap-3 rounded-xl border border-zinc-300 bg-zinc-50 p-4 text-sm dark:border-zinc-700 dark:bg-zinc-900 sm:grid-cols-2">
              <div className="rounded-lg border border-zinc-300 bg-white p-3 dark:border-zinc-700 dark:bg-zinc-950">
                <p className="font-semibold">Local Branch</p>
                <p className="mt-1 text-zinc-600 dark:text-zinc-300">
                  After Check For Surprises: {loopState?.surprise_found ? "Surprise Found" : "No Surprise"}
                </p>
              </div>
              <div className="rounded-lg border border-zinc-300 bg-white p-3 dark:border-zinc-700 dark:bg-zinc-950">
                <p className="font-semibold">Frontier Branch</p>
                <p className="mt-1 text-zinc-600 dark:text-zinc-300">
                  After Plan Actions: {loopState?.action_required ? "Action Required" : "No Action Required"}
                </p>
              </div>
              <div className="rounded-lg border border-zinc-300 bg-white p-3 dark:border-zinc-700 dark:bg-zinc-950 sm:col-span-2">
                <p className="font-semibold">Loop Flow</p>
                <p className="mt-1 text-zinc-600 dark:text-zinc-300">
                  {`Sensors -> Local Model -> ${
                    loopState?.surprise_found ? "Frontier Model" : "Gather New Percepts"
                  }${
                    loopState?.surprise_found
                      ? ` -> ${loopState.action_required ? "Actuators" : "No Action Required"} -> Gather New Percepts`
                      : ""
                  }`}
                </p>
              </div>
            </div>

            {!data?.connected && data?.error ? (
              <p className="rounded-lg border border-zinc-300 bg-zinc-200 p-3 text-sm dark:border-zinc-700 dark:bg-zinc-800">
                {data.error}
              </p>
            ) : null}
          </section>

          <article className="rounded-2xl border border-zinc-300 bg-white p-4 shadow-sm dark:border-zinc-700 dark:bg-zinc-950 lg:col-span-3">
            <h2 className="text-lg font-semibold">Actuators</h2>
            <button
              type="button"
              onClick={() => {
                const next = actuators.length + 1;
                setActuators((current) => [
                  ...current,
                  {
                    id: `actuator-${Date.now()}`,
                    name: `New Actuator ${next}`,
                    policy: "Rate limit: none",
                    recentActions: ["New action", "New action", "New action"],
                  },
                ]);
              }}
              className="mt-3 w-full rounded-lg border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium transition hover:bg-zinc-200 dark:border-zinc-700 dark:bg-zinc-900 dark:hover:bg-zinc-800"
            >
              Add an Actuator
            </button>

            <div className="mt-4 space-y-3">
              {actuators.length === 0 ? (
                <p className="rounded-lg border border-zinc-200 bg-zinc-50 p-3 text-sm text-zinc-600 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300">
                  No actuators registered.
                </p>
              ) : (
                actuators.map((actuator) => (
                  <div
                    key={actuator.id}
                    className="rounded-xl border border-zinc-300 bg-zinc-50 p-3 dark:border-zinc-700 dark:bg-zinc-900"
                  >
                    <div className="flex items-center justify-between gap-3">
                      <p className="text-sm font-semibold">{actuator.name}</p>
                      <button
                        type="button"
                        className="rounded-md border border-zinc-300 bg-white px-2 py-1 text-xs font-medium transition hover:bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-950 dark:hover:bg-zinc-800"
                      >
                        Edit
                      </button>
                    </div>
                    <p className="mt-2 text-xs font-medium uppercase tracking-wide text-zinc-500 dark:text-zinc-400">
                      Actuator Policy
                    </p>
                    <p className="mt-1 rounded-md border border-zinc-300 bg-white px-2 py-1 text-sm dark:border-zinc-700 dark:bg-zinc-950">
                      {actuator.policy}
                    </p>
                    <p className="mt-2 text-xs font-medium uppercase tracking-wide text-zinc-500 dark:text-zinc-400">
                      Recent Actions
                    </p>
                    <div className="mt-1 space-y-1.5">
                      {actuator.recentActions.slice(0, 3).map((action, actionIndex) => (
                        <p
                          key={`${actuator.id}-action-${actionIndex}`}
                          className="w-full rounded-md border border-zinc-300 bg-white px-2 py-1 text-sm dark:border-zinc-700 dark:bg-zinc-950"
                        >
                          {action}
                        </p>
                      ))}
                    </div>
                  </div>
                ))
              )}
            </div>
          </article>
          </section>
        </div>
      </div>
    </main>
  );
}
