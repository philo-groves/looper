"use client";

import { ReactNode, useEffect, useState } from "react";

import { Provider, SetupStepId } from "@/components/dashboard/types";

type UseSetupFlowParams = {
  socketConnected: boolean;
  setupRequired: boolean;
  wsCommand: <T>(method: string, params: unknown) => Promise<T>;
};

type ProviderModelsResponse = {
  models: string[];
};

type OllamaModelVersionsResponse = {
  versions: string[];
};

const DEFAULT_LOCAL_MODEL_OPTIONS = ["gemma3", "qwen3", "gpt-oss"];
const DEFAULT_LOCAL_VERSION_OPTIONS = ["latest", "4b", "8b", "12b"];

const DEFAULT_FRONTIER_MODEL_OPTIONS: Record<Provider, string[]> = {
  ollama: ["gemma3:4b", "qwen3:8b", "gpt-oss:20b"],
  open_ai: ["gpt-5.2", "gpt-5.1", "gpt-4.1"],
  open_code_zen: ["kimi-k2.5", "kimi-k2", "deepseek-r1"],
};

function normalizeApiKey(raw: string) {
  return raw.trim().replace(/^bearer\s+/i, "").replace(/^"|"$/g, "").replace(/^'|'$/g, "");
}

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

export function useSetupFlow({ socketConnected, setupRequired, wsCommand }: UseSetupFlowParams) {
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
          { model: chosenModel },
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
  }, [socketConnected, setupRequired, localModel, localModelVersion, wsCommand]);

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

  function renderSetupContent(): ReactNode {
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
      return (
        <div className="space-y-2">
          <label className="text-sm font-medium">Frontier model</label>
          <select
            value={frontierModel}
            onChange={(event) => setFrontierModel(event.target.value)}
            className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
          >
            {frontierModels.map((item) => (
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

  return {
    setupStep,
    activeSetupSteps,
    setupIndex,
    setupBusy,
    setupError,
    setupInfo,
    setupContent: renderSetupContent(),
    goSetupBack,
    goSetupNext,
    completeSetup,
  };
}
