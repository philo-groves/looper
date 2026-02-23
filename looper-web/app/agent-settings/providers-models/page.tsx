"use client";

import { useEffect, useState } from "react";

import { Provider } from "@/components/dashboard/types";
import { useDashboardSocket } from "@/components/dashboard/useDashboardSocket";

type ProviderModelsResponse = {
  models: string[];
};

const PROVIDERS: Array<{ value: Provider; label: string }> = [
  { value: "ollama", label: "Ollama" },
  { value: "open_ai", label: "OpenAI" },
  { value: "open_code_zen", label: "OpenCode Zen" },
];

const FALLBACK_MODELS: Record<Provider, string[]> = {
  ollama: ["gemma3:4b", "qwen3:8b", "gpt-oss:20b"],
  open_ai: ["gpt-5.2", "gpt-5.1", "gpt-4.1"],
  open_code_zen: ["kimi-k2.5", "kimi-k2", "deepseek-r1"],
};

function normalizeApiKey(raw: string) {
  return raw.trim().replace(/^bearer\s+/i, "").replace(/^"|"$/g, "").replace(/^'|'$/g, "");
}

function coerceProvider(input: string | null | undefined): Provider {
  if (input === "open_ai" || input === "open_code_zen" || input === "ollama") {
    return input;
  }
  return "ollama";
}

export default function ProvidersModelsPage() {
  const { data, socketConnected, socketError, wsCommand } = useDashboardSocket();

  const [localProvider, setLocalProvider] = useState<Provider>("ollama");
  const [frontierProvider, setFrontierProvider] = useState<Provider>("open_ai");
  const [localModel, setLocalModel] = useState("");
  const [frontierModel, setFrontierModel] = useState("");
  const [localModels, setLocalModels] = useState<string[]>(FALLBACK_MODELS.ollama);
  const [frontierModels, setFrontierModels] = useState<string[]>(FALLBACK_MODELS.open_ai);

  const [openAiKey, setOpenAiKey] = useState("");
  const [openCodeZenKey, setOpenCodeZenKey] = useState("");

  const [loadingLocalModels, setLoadingLocalModels] = useState(false);
  const [loadingFrontierModels, setLoadingFrontierModels] = useState(false);
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!data) {
      return;
    }

    const nextLocalProvider = coerceProvider(data.local_model.provider);
    const nextFrontierProvider = coerceProvider(data.frontier_model.provider);
    const nextLocalModel = data.local_model.model ?? FALLBACK_MODELS[nextLocalProvider][0];
    const nextFrontierModel = data.frontier_model.model ?? FALLBACK_MODELS[nextFrontierProvider][0];

    setLocalProvider(nextLocalProvider);
    setFrontierProvider(nextFrontierProvider);
    setLocalModel(nextLocalModel);
    setFrontierModel(nextFrontierModel);
    setLocalModels((current) => (current.length > 0 ? current : FALLBACK_MODELS[nextLocalProvider]));
    setFrontierModels((current) => (current.length > 0 ? current : FALLBACK_MODELS[nextFrontierProvider]));
  }, [data]);

  async function loadModels(
    provider: Provider,
    scope: "local" | "frontier",
    apiKeyOverride?: string,
  ) {
    if (scope === "local") {
      setLoadingLocalModels(true);
    } else {
      setLoadingFrontierModels(true);
    }
    setError(null);
    setStatus(null);

    try {
      const key =
        provider === "open_ai"
          ? normalizeApiKey(apiKeyOverride ?? openAiKey)
          : provider === "open_code_zen"
            ? normalizeApiKey(apiKeyOverride ?? openCodeZenKey)
            : "";

      const response = await wsCommand<ProviderModelsResponse>("list_provider_models", {
        provider,
        api_key: key,
      });

      const models = response.models.length > 0 ? response.models : FALLBACK_MODELS[provider];
      if (scope === "local") {
        setLocalModels(models);
        if (!models.includes(localModel)) {
          setLocalModel(models[0]);
        }
      } else {
        setFrontierModels(models);
        if (!models.includes(frontierModel)) {
          setFrontierModel(models[0]);
        }
      }
    } catch (loadError) {
      const message = loadError instanceof Error ? loadError.message : "Failed to load models.";
      setError(message);
      if (scope === "local") {
        const fallback = FALLBACK_MODELS[provider];
        setLocalModels(fallback);
        if (!fallback.includes(localModel)) {
          setLocalModel(fallback[0]);
        }
      } else {
        const fallback = FALLBACK_MODELS[provider];
        setFrontierModels(fallback);
        if (!fallback.includes(frontierModel)) {
          setFrontierModel(fallback[0]);
        }
      }
    } finally {
      if (scope === "local") {
        setLoadingLocalModels(false);
      } else {
        setLoadingFrontierModels(false);
      }
    }
  }

  async function saveApiKey(provider: Provider, rawKey: string) {
    const apiKey = normalizeApiKey(rawKey);
    if (!apiKey) {
      setError("API key cannot be empty.");
      return;
    }

    setSaving(true);
    setStatus(null);
    setError(null);
    try {
      await wsCommand("register_api_key", {
        provider,
        api_key: apiKey,
      });
      setStatus(`${provider === "open_ai" ? "OpenAI" : "OpenCode Zen"} API key saved.`);
    } catch (saveError) {
      const message = saveError instanceof Error ? saveError.message : "Failed to save API key.";
      setError(message);
    } finally {
      setSaving(false);
    }
  }

  async function applyModelSelection() {
    setSaving(true);
    setStatus(null);
    setError(null);
    try {
      await wsCommand("configure_models", {
        local: {
          provider: localProvider,
          model: localModel,
        },
        frontier: {
          provider: frontierProvider,
          model: frontierModel,
        },
      });
      setStatus("Provider and model selection saved to agent settings.");
    } catch (saveError) {
      const message = saveError instanceof Error ? saveError.message : "Failed to apply model settings.";
      setError(message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="space-y-4">
      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <h1 className="text-xl font-semibold">Providers &amp; Models</h1>
        <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-300">
          Switch providers/models and assign API keys. Changes persist to the same agent settings/key files used by setup.
        </p>
      </article>

      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <h2 className="text-base font-semibold">API Keys</h2>
        <div className="mt-3 grid gap-4 md:grid-cols-2">
          <div className="space-y-2">
            <label className="text-sm font-medium">OpenAI API Key</label>
            <input
              type="password"
              value={openAiKey}
              onChange={(event) => setOpenAiKey(event.target.value)}
              placeholder="sk-..."
              className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
            />
            <button
              type="button"
              onClick={() => void saveApiKey("open_ai", openAiKey)}
              disabled={saving || !socketConnected}
              className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
            >
              Save OpenAI Key
            </button>
          </div>

          <div className="space-y-2">
            <label className="text-sm font-medium">OpenCode Zen API Key</label>
            <input
              type="password"
              value={openCodeZenKey}
              onChange={(event) => setOpenCodeZenKey(event.target.value)}
              placeholder="Bearer ..."
              className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
            />
            <button
              type="button"
              onClick={() => void saveApiKey("open_code_zen", openCodeZenKey)}
              disabled={saving || !socketConnected}
              className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
            >
              Save OpenCode Zen Key
            </button>
          </div>
        </div>
      </article>

      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <h2 className="text-base font-semibold">Model Selection</h2>
        <div className="mt-3 grid gap-5 md:grid-cols-2">
          <div className="space-y-3 rounded-xl border border-zinc-300 bg-zinc-50 p-4 dark:border-zinc-700 dark:bg-zinc-900">
            <h3 className="text-sm font-semibold">Local Model</h3>
            <div className="space-y-2">
              <label className="text-sm">Provider</label>
              <select
                value={localProvider}
                onChange={(event) => {
                  const provider = event.target.value as Provider;
                  setLocalProvider(provider);
                  const defaults = FALLBACK_MODELS[provider];
                  setLocalModels(defaults);
                  setLocalModel(defaults[0]);
                }}
                className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
              >
                {PROVIDERS.map((provider) => (
                  <option key={provider.value} value={provider.value}>
                    {provider.label}
                  </option>
                ))}
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm">Model</label>
              <select
                value={localModel}
                onChange={(event) => setLocalModel(event.target.value)}
                className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
              >
                {localModels.map((model) => (
                  <option key={model} value={model}>
                    {model}
                  </option>
                ))}
              </select>
            </div>

            <button
              type="button"
              onClick={() => void loadModels(localProvider, "local")}
              disabled={loadingLocalModels || !socketConnected}
              className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
            >
              {loadingLocalModels ? "Loading local models..." : "Refresh Local Models"}
            </button>
          </div>

          <div className="space-y-3 rounded-xl border border-zinc-300 bg-zinc-50 p-4 dark:border-zinc-700 dark:bg-zinc-900">
            <h3 className="text-sm font-semibold">Frontier Model</h3>
            <div className="space-y-2">
              <label className="text-sm">Provider</label>
              <select
                value={frontierProvider}
                onChange={(event) => {
                  const provider = event.target.value as Provider;
                  setFrontierProvider(provider);
                  const defaults = FALLBACK_MODELS[provider];
                  setFrontierModels(defaults);
                  setFrontierModel(defaults[0]);
                }}
                className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
              >
                {PROVIDERS.map((provider) => (
                  <option key={provider.value} value={provider.value}>
                    {provider.label}
                  </option>
                ))}
              </select>
            </div>

            <div className="space-y-2">
              <label className="text-sm">Model</label>
              <select
                value={frontierModel}
                onChange={(event) => setFrontierModel(event.target.value)}
                className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
              >
                {frontierModels.map((model) => (
                  <option key={model} value={model}>
                    {model}
                  </option>
                ))}
              </select>
            </div>

            <button
              type="button"
              onClick={() => void loadModels(frontierProvider, "frontier")}
              disabled={loadingFrontierModels || !socketConnected}
              className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
            >
              {loadingFrontierModels ? "Loading frontier models..." : "Refresh Frontier Models"}
            </button>
          </div>
        </div>

        <div className="mt-4 flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={() => void applyModelSelection()}
            disabled={saving || !socketConnected || !localModel || !frontierModel}
            className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
          >
            {saving ? "Saving..." : "Apply Providers & Models"}
          </button>
          {!socketConnected && socketError ? (
            <span className="rounded-md border border-red-700 bg-red-600 px-3 py-2 text-xs text-white">
              {socketError}
            </span>
          ) : null}
          {error ? (
            <span className="rounded-md border border-red-700 bg-red-600 px-3 py-2 text-xs text-white">{error}</span>
          ) : null}
          {status ? (
            <span className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-xs dark:border-zinc-700 dark:bg-zinc-800">
              {status}
            </span>
          ) : null}
        </div>
      </article>
    </section>
  );
}
