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

type DashboardResponse = {
  type: "event" | "response";
  event?: string;
  data?: DashboardPayload;
  id?: number;
  ok?: boolean;
  result?: unknown;
  error?: string;
};

type LoopPhaseTransitionPayload = {
  sequence: number;
  phase: DashboardPayload["loop_visualization"]["current_phase"];
  loop_visualization: DashboardPayload["loop_visualization"];
  emitted_at_unix_ms: number;
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

type Provider = "ollama" | "open_ai" | "open_code_zen";

type SetupStepId =
  | "local_provider"
  | "local_model"
  | "local_model_version"
  | "frontier_provider"
  | "frontier_api_key"
  | "frontier_model"
  | "install_ollama"
  | "install_model";

type WsResponseEnvelope = {
  type: "response";
  id?: number;
  ok?: boolean;
  result?: unknown;
  error?: string;
};

const SETUP_STEPS: Array<{ id: SetupStepId; label: string }> = [
  { id: "local_provider", label: "1. Select a local provider" },
  { id: "local_model", label: "2. Select a local model" },
  { id: "local_model_version", label: "2a. Select a model version" },
  { id: "frontier_provider", label: "3. Select a frontier provider" },
  { id: "frontier_api_key", label: "3a. Add API key" },
  { id: "frontier_model", label: "4. Select a frontier model" },
  { id: "install_ollama", label: "5. Install Ollama" },
  { id: "install_model", label: "6. Install selected model(s)" },
];

const DEFAULT_LOCAL_MODEL_OPTIONS = ["gemma3", "qwen3", "gpt-oss"];
const DEFAULT_LOCAL_VERSION_OPTIONS = ["latest", "4b", "8b", "12b"];

const DEFAULT_FRONTIER_MODEL_OPTIONS: Record<Provider, string[]> = {
  ollama: ["gemma3:4b", "qwen3:8b", "gpt-oss:20b"],
  open_ai: ["gpt-5.2", "gpt-5.1", "gpt-4.1"],
  open_code_zen: ["kimi-k2.5", "kimi-k2", "deepseek-r1"],
};

type ProviderModelsResponse = {
  models: string[];
};

type OllamaModelVersionsResponse = {
  versions: string[];
};

function wsUrl() {
  const configured = process.env.NEXT_PUBLIC_LOOPER_AGENT_WS_URL;
  if (configured && configured.length > 0) {
    return configured;
  }
  return "ws://127.0.0.1:10001/api/ws";
}

function normalizeApiKey(raw: string) {
  return raw.trim().replace(/^bearer\s+/i, "").replace(/^"|"$/g, "").replace(/^'|'$/g, "");
}

async function wsCommand<T>(method: string, params: unknown): Promise<T> {
  const socket = new window.WebSocket(wsUrl());

  return new Promise<T>((resolve, reject) => {
    let done = false;

    function finishError(message: string) {
      if (done) {
        return;
      }
      done = true;
      socket.close();
      reject(new Error(message));
    }

    function finishOk(value: T) {
      if (done) {
        return;
      }
      done = true;
      socket.close();
      resolve(value);
    }

    socket.onerror = () => finishError("websocket request failed");

    socket.onopen = () => {
      socket.send(
        JSON.stringify({
          id: 1,
          method,
          params,
        }),
      );
    };

    socket.onmessage = (event) => {
      try {
        const payload = JSON.parse(event.data) as WsResponseEnvelope | DashboardResponse;
        if (payload.type !== "response" || payload.id !== 1) {
          return;
        }

        if (!payload.ok) {
          finishError(payload.error ?? "request failed");
          return;
        }

        finishOk(payload.result as T);
      } catch {
        finishError("invalid websocket response payload");
      }
    };
  });
}

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

function phaseLabel(
  phase: DashboardPayload["loop_visualization"]["current_phase"] | undefined,
): string {
  if (!phase) {
    return "Unknown";
  }

  return phase
    .split("_")
    .map((chunk) => chunk.charAt(0).toUpperCase() + chunk.slice(1))
    .join(" ");
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
  const [theme, setTheme] = useState<"light" | "dark">("light");
  const [themeReady, setThemeReady] = useState(false);
  const [data, setData] = useState<DashboardPayload | null>(null);
  const [socketConnected, setSocketConnected] = useState(false);
  const [socketError, setSocketError] = useState<string | null>(null);
  const [sensors, setSensors] = useState<EditableSensor[]>([]);
  const [actuators, setActuators] = useState<EditableActuator[]>([]);
  const [isSidebarOpen, setIsSidebarOpen] = useState(true);
  const [setupStep, setSetupStep] = useState<SetupStepId>("local_provider");
  const [localProvider] = useState<Provider>("ollama");
  const [localModel, setLocalModel] = useState("gemma3");
  const [localModelVersion, setLocalModelVersion] = useState("latest");
  const [localModelOptions, setLocalModelOptions] = useState<string[]>(DEFAULT_LOCAL_MODEL_OPTIONS);
  const [localModelVersions, setLocalModelVersions] = useState<string[]>(DEFAULT_LOCAL_VERSION_OPTIONS);
  const [localModelsLoading, setLocalModelsLoading] = useState(false);
  const [localVersionsLoading, setLocalVersionsLoading] = useState(false);
  const [frontierProvider, setFrontierProvider] = useState<Provider>("open_ai");
  const [frontierApiKey, setFrontierApiKey] = useState("");
  const [frontierModel, setFrontierModel] = useState("gpt-5.2");
  const [frontierModels, setFrontierModels] = useState<string[]>(
    DEFAULT_FRONTIER_MODEL_OPTIONS.open_ai,
  );
  const [frontierModelsLoading, setFrontierModelsLoading] = useState(false);
  const [setupBusy, setSetupBusy] = useState(false);
  const [setupError, setSetupError] = useState<string | null>(null);
  const [setupInfo, setSetupInfo] = useState<string | null>(null);

  useEffect(() => {
    const stored = window.localStorage.getItem("looper-theme");
    if (stored === "dark" || stored === "light") {
      setTheme(stored);
      setThemeReady(true);
      return;
    }

    setTheme(window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light");
    setThemeReady(true);
  }, []);

  useEffect(() => {
    if (!themeReady) {
      return;
    }
    document.documentElement.classList.toggle("dark", theme === "dark");
    window.localStorage.setItem("looper-theme", theme);
  }, [theme, themeReady]);

  useEffect(() => {
    let ws: WebSocket | null = null;
    let reconnectTimer: number | null = null;
    let closedByCleanup = false;

    function connect() {
      ws = new window.WebSocket(wsUrl());

      ws.onopen = () => {
        setSocketConnected(true);
        setSocketError(null);
      };

      ws.onmessage = (event) => {
        try {
          const payload = JSON.parse(event.data) as DashboardResponse;
          if (payload.type === "event" && payload.event === "dashboard_snapshot" && payload.data) {
            const snapshot = payload.data;
            setData(snapshot);
            setSensors((existing) => mergeSensors(existing, snapshot.sensors));
            setActuators((existing) => mergeActuators(existing, snapshot.actuators));
            return;
          }

          if (payload.type === "event" && payload.event === "loop_phase_transition" && payload.data) {
            const phaseEvent = payload.data as unknown as LoopPhaseTransitionPayload;
            setData((current) => {
              if (!current) {
                return current;
              }
              return {
                ...current,
                loop_visualization: phaseEvent.loop_visualization,
              };
            });
          }
        } catch {
          setSocketError("Received invalid websocket payload.");
        }
      };

      ws.onerror = () => {
        setSocketConnected(false);
        setSocketError("Websocket connection error.");
      };

      ws.onclose = () => {
        setSocketConnected(false);
        if (!closedByCleanup) {
          reconnectTimer = window.setTimeout(connect, 1200);
        }
      };
    }

    connect();

    return () => {
      closedByCleanup = true;
      if (reconnectTimer !== null) {
        window.clearTimeout(reconnectTimer);
      }
      ws?.close();
    };
  }, []);

  const snapshot = data;
  const setupRequired = snapshot ? snapshot.state.state === "setup" || !snapshot.state.configured : true;

  const loopState = data?.loop_visualization;

  const localModelLabel = `${snapshot?.local_model.provider ?? "Local"} / ${snapshot?.local_model.model ?? "Unassigned"}`;
  const frontierModelLabel = `${snapshot?.frontier_model.provider ?? "Frontier"} / ${snapshot?.frontier_model.model ?? "Unassigned"}`;

  function setupStepsForProvider(provider: Provider): SetupStepId[] {
    if (provider === "ollama") {
      return [
        "local_provider",
        "local_model",
        "local_model_version",
        "frontier_provider",
        "frontier_model",
        "install_ollama",
        "install_model",
      ];
    }

    return [
      "local_provider",
      "local_model",
      "local_model_version",
      "frontier_provider",
      "frontier_api_key",
      "frontier_model",
      "install_ollama",
      "install_model",
    ];
  }

  const activeSetupSteps = setupStepsForProvider(frontierProvider);
  const setupIndex = activeSetupSteps.indexOf(setupStep);

  useEffect(() => {
    if (!socketConnected || !setupRequired) {
      return;
    }

    let cancelled = false;

    async function hydrateOllamaLists() {
      try {
        const basePayload = await wsCommand<ProviderModelsResponse>("list_ollama_base_models", {});
        if (cancelled) {
          return;
        }

        const models =
          basePayload.models.length > 0 ? basePayload.models : DEFAULT_LOCAL_MODEL_OPTIONS;
        setLocalModelOptions(models);

        const chosenModel = models.includes(localModel) ? localModel : models[0];
        if (chosenModel !== localModel) {
          setLocalModel(chosenModel);
        }

        const versionPayload = await wsCommand<OllamaModelVersionsResponse>(
          "list_ollama_model_versions",
          {
            model: chosenModel,
          },
        );
        if (cancelled) {
          return;
        }

        const versions =
          versionPayload.versions.length > 0
            ? versionPayload.versions
            : DEFAULT_LOCAL_VERSION_OPTIONS;
        setLocalModelVersions(versions);
        if (!versions.includes(localModelVersion)) {
          setLocalModelVersion(versions[0]);
        }
      } catch {
        if (!cancelled) {
          setLocalModelOptions(DEFAULT_LOCAL_MODEL_OPTIONS);
          setLocalModelVersions(DEFAULT_LOCAL_VERSION_OPTIONS);
        }
      }
    }

    void hydrateOllamaLists();

    return () => {
      cancelled = true;
    };
  }, [socketConnected, setupRequired, localModel, localModelVersion]);

  async function loadFrontierModels(provider: Provider, apiKey: string) {
    setFrontierModelsLoading(true);
    setSetupError(null);

    try {
      const payload = await wsCommand<ProviderModelsResponse>("list_provider_models", {
        provider,
        api_key: normalizeApiKey(apiKey),
      });

      const models = payload.models.length > 0 ? payload.models : DEFAULT_FRONTIER_MODEL_OPTIONS[provider];
      setFrontierModels(models);
      if (!models.includes(frontierModel)) {
        setFrontierModel(models[0]);
      }
    } catch (error) {
      setFrontierModels(DEFAULT_FRONTIER_MODEL_OPTIONS[provider]);
      if (!DEFAULT_FRONTIER_MODEL_OPTIONS[provider].includes(frontierModel)) {
        setFrontierModel(DEFAULT_FRONTIER_MODEL_OPTIONS[provider][0]);
      }
      const message = error instanceof Error ? error.message : "Failed to load model list.";
      setSetupError(message);
    } finally {
      setFrontierModelsLoading(false);
    }
  }

  async function loadOllamaBaseModels() {
    setLocalModelsLoading(true);
    setSetupError(null);

    try {
      const payload = await wsCommand<ProviderModelsResponse>("list_ollama_base_models", {});
      const models = payload.models.length > 0 ? payload.models : DEFAULT_LOCAL_MODEL_OPTIONS;
      setLocalModelOptions(models);
      if (!models.includes(localModel)) {
        const nextModel = models[0];
        setLocalModel(nextModel);
        void loadOllamaModelVersions(nextModel);
      }
    } catch (error) {
      setLocalModelOptions(DEFAULT_LOCAL_MODEL_OPTIONS);
      const message = error instanceof Error ? error.message : "Failed to load Ollama model list.";
      setSetupError(message);
    } finally {
      setLocalModelsLoading(false);
    }
  }

  async function loadOllamaModelVersions(model: string) {
    setLocalVersionsLoading(true);
    setSetupError(null);

    try {
      const payload = await wsCommand<OllamaModelVersionsResponse>("list_ollama_model_versions", {
        model,
      });
      const versions = payload.versions.length > 0 ? payload.versions : DEFAULT_LOCAL_VERSION_OPTIONS;
      setLocalModelVersions(versions);
      if (!versions.includes(localModelVersion)) {
        setLocalModelVersion(versions[0]);
      }
    } catch (error) {
      setLocalModelVersions(DEFAULT_LOCAL_VERSION_OPTIONS);
      const message = error instanceof Error ? error.message : "Failed to load Ollama model versions.";
      setSetupError(message);
    } finally {
      setLocalVersionsLoading(false);
    }
  }

  function goSetupBack() {
    const previousIndex = Math.max(0, setupIndex - 1);
    setSetupStep(activeSetupSteps[previousIndex]);
  }

  function goSetupNext() {
    const nextIndex = Math.min(activeSetupSteps.length - 1, setupIndex + 1);
    setSetupStep(activeSetupSteps[nextIndex]);
  }

  async function completeSetup() {
    setSetupBusy(true);
    setSetupError(null);
    setSetupInfo(null);

    try {
      const cleanedKey = normalizeApiKey(frontierApiKey);
      if (frontierProvider !== "ollama" && cleanedKey.length > 0) {
        await wsCommand("register_api_key", {
          provider: frontierProvider,
          api_key: cleanedKey,
        });
      }

      await wsCommand("configure_models", {
        local: {
          provider: localProvider,
          model: `${localModel}:${localModelVersion}`,
        },
        frontier: {
          provider: frontierProvider,
          model: frontierModel,
        },
      });

      await wsCommand("loop_start", { interval_ms: 500 });
      setSetupInfo("Setup complete. Waiting for live runtime status...");
    } catch (error) {
      const message = error instanceof Error ? error.message : "Setup failed.";
      setSetupError(message);
    } finally {
      setSetupBusy(false);
    }
  }

  function renderSetupContent() {
    if (setupStep === "local_provider") {
      return <p className="text-sm">Local provider is fixed to <strong>Ollama</strong> for now.</p>;
    }

    if (setupStep === "local_model") {
      return (
        <div className="space-y-2">
          <label className="text-sm font-medium">Local model base</label>
          <select
            value={localModel}
            onChange={(event) => {
              const model = event.target.value;
              setLocalModel(model);
              void loadOllamaModelVersions(model);
            }}
            className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
          >
            {localModelOptions.map((item) => (
              <option key={item} value={item}>
                {item}
              </option>
            ))}
          </select>
          <button
            type="button"
            onClick={() => void loadOllamaBaseModels()}
            disabled={localModelsLoading}
            className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
          >
            {localModelsLoading ? "Loading models..." : "Refresh model list"}
          </button>
        </div>
      );
    }

    if (setupStep === "local_model_version") {
      return (
        <div className="space-y-2">
          <label className="text-sm font-medium">Local model version</label>
          <select
            value={localModelVersion}
            onChange={(event) => setLocalModelVersion(event.target.value)}
            className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
          >
            {localModelVersions.map((item) => (
              <option key={item} value={item}>
                {item}
              </option>
            ))}
          </select>
          <button
            type="button"
            onClick={() => void loadOllamaModelVersions(localModel)}
            disabled={localVersionsLoading}
            className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
          >
            {localVersionsLoading ? "Loading versions..." : "Refresh version list"}
          </button>
        </div>
      );
    }

    if (setupStep === "frontier_provider") {
      return (
        <div className="space-y-2">
          <label className="text-sm font-medium">Frontier provider</label>
          <select
            value={frontierProvider}
            onChange={(event) => {
              const provider = event.target.value as Provider;
              setFrontierProvider(provider);
              const defaults = DEFAULT_FRONTIER_MODEL_OPTIONS[provider];
              setFrontierModels(defaults);
              setFrontierModel(defaults[0]);
              void loadFrontierModels(provider, frontierApiKey);
              setSetupStep(provider === "ollama" ? "frontier_model" : "frontier_api_key");
            }}
            className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
          >
            <option value="open_ai">OpenAI</option>
            <option value="open_code_zen">OpenCode Zen</option>
            <option value="ollama">Ollama</option>
          </select>
        </div>
      );
    }

    if (setupStep === "frontier_api_key") {
      return (
        <div className="space-y-2">
          <label className="text-sm font-medium">Frontier API key (optional if already saved)</label>
          <input
            value={frontierApiKey}
            onChange={(event) => setFrontierApiKey(event.target.value)}
            type="password"
            placeholder="sk-..."
            className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
          />
          <button
            type="button"
            onClick={() => void loadFrontierModels(frontierProvider, frontierApiKey)}
            disabled={frontierModelsLoading}
            className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
          >
            {frontierModelsLoading ? "Loading models..." : "Load model list"}
          </button>
        </div>
      );
    }

    if (setupStep === "frontier_model") {
      const options = frontierModels;
      return (
        <div className="space-y-2">
          <label className="text-sm font-medium">Frontier model</label>
          <select
            value={frontierModel}
            onChange={(event) => setFrontierModel(event.target.value)}
            className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
          >
            {options.map((item) => (
              <option key={item} value={item}>
                {item}
              </option>
            ))}
          </select>
        </div>
      );
    }

    if (setupStep === "install_ollama") {
      return <p className="text-sm">Confirm Ollama is installed and running before continuing setup.</p>;
    }

    return (
      <p className="text-sm">
        Confirm selected models are installed, then click <strong>Complete Setup</strong>.
      </p>
    );
  }

  if (setupRequired) {
    return (
      <main className="min-h-screen w-full bg-zinc-100 px-4 py-6 text-zinc-900 dark:bg-black dark:text-zinc-100 sm:px-6">
        <section className="mx-auto w-full max-w-4xl rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
          <div className="flex items-center justify-between gap-3">
            <h1 className="text-2xl font-semibold">Looper Setup</h1>
            <button
              type="button"
              onClick={() => setTheme((current) => (current === "light" ? "dark" : "light"))}
              className="rounded-lg border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium dark:border-zinc-700 dark:bg-zinc-900"
            >
              {theme === "light" ? "Switch to Dark" : "Switch to Light"}
            </button>
          </div>

          <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-300">
            Setup mode matches terminal setup steps. Workspace features unlock after setup completes.
          </p>

          <div className="mt-4 grid gap-4 md:grid-cols-2">
            <div className="rounded-xl border border-zinc-300 bg-zinc-50 p-3 dark:border-zinc-700 dark:bg-zinc-900">
              <p className="text-sm font-semibold">Setup Steps</p>
              <ul className="mt-2 space-y-1 text-sm">
                {SETUP_STEPS.filter((item) => activeSetupSteps.includes(item.id)).map((step) => (
                  <li
                    key={step.id}
                    className={`rounded-md px-2 py-1 ${
                      step.id === setupStep
                        ? "bg-zinc-200 font-medium dark:bg-zinc-800"
                        : "text-zinc-600 dark:text-zinc-300"
                    }`}
                  >
                    {step.label}
                  </li>
                ))}
              </ul>
            </div>

            <div className="rounded-xl border border-zinc-300 bg-zinc-50 p-3 dark:border-zinc-700 dark:bg-zinc-900">
              <p className="text-sm font-semibold">Current Step</p>
              <div className="mt-2">{renderSetupContent()}</div>

              {setupError ? (
                <p className="mt-3 rounded-md border border-red-700 bg-red-600 px-3 py-2 text-sm text-white">
                  {setupError}
                </p>
              ) : null}
              {setupInfo ? (
                <p className="mt-3 rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-800">
                  {setupInfo}
                </p>
              ) : null}
              {!socketConnected ? (
                <p className="mt-3 rounded-md border border-red-700 bg-red-600 px-3 py-2 text-sm text-white">
                  Agent connection required for setup.
                </p>
              ) : null}

              <div className="mt-4 flex gap-2">
                <button
                  type="button"
                  onClick={goSetupBack}
                  disabled={setupIndex <= 0 || setupBusy}
                  className="rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-950"
                >
                  Back
                </button>
                {setupStep !== "install_model" ? (
                  <button
                    type="button"
                    onClick={goSetupNext}
                    disabled={setupBusy}
                    className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
                  >
                    Continue
                  </button>
                ) : (
                  <button
                    type="button"
                    onClick={() => void completeSetup()}
                    disabled={setupBusy || !socketConnected}
                    className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
                  >
                    {setupBusy ? "Completing..." : "Complete Setup"}
                  </button>
                )}
              </div>
            </div>
          </div>
        </section>
      </main>
    );
  }

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
                <span className={`rounded-full px-3 py-1 text-xs font-medium ${statusPill(socketConnected)}`}>
                  {socketConnected ? "Agent Connected" : "Agent Offline"}
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
            <p className="text-sm text-zinc-600 dark:text-zinc-300">
              Current phase: {phaseLabel(loopState?.current_phase)}
            </p>
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

            {!socketConnected && socketError ? (
              <p className="rounded-lg border border-zinc-300 bg-zinc-200 p-3 text-sm dark:border-zinc-700 dark:bg-zinc-800">
                {socketError}
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
