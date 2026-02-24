"use client";

import { useCallback, useEffect, useMemo, useState } from "react";

type InternalPluginStatus = {
  id: string;
  name: string;
  version: string;
  path: string;
  imported: boolean;
  enabled: boolean;
  user_enabled: boolean;
  status_message: string | null;
  sensors: string[];
  actuators: string[];
};

type PluginStatusResponse = {
  plugins: InternalPluginStatus[];
};

export default function AllPluginsPage() {
  const [plugins, setPlugins] = useState<InternalPluginStatus[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [pendingPluginId, setPendingPluginId] = useState<string | null>(null);

  const loadStatuses = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const response = await fetch("/api/agent/plugins/status", {
        cache: "no-store",
      });
      if (!response.ok) {
        throw new Error(`Failed to load plugin statuses (status ${response.status}).`);
      }
      const payload = (await response.json()) as PluginStatusResponse;
      setPlugins(payload.plugins ?? []);
    } catch (loadError) {
      const message =
        loadError instanceof Error
          ? loadError.message
          : "Failed to load plugin statuses.";
      setError(message);
      setPlugins([]);
    } finally {
      setLoading(false);
    }
  }, []);

  async function setPluginEnabled(pluginId: string, enabled: boolean) {
    setPendingPluginId(pluginId);
    setError(null);
    try {
      const response = await fetch(`/api/agent/plugins/${encodeURIComponent(pluginId)}/enabled`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ enabled }),
      });
      if (!response.ok) {
        throw new Error(`Failed to update plugin status (status ${response.status}).`);
      }
      await loadStatuses();
    } catch (toggleError) {
      const message =
        toggleError instanceof Error
          ? toggleError.message
          : "Failed to update plugin status.";
      setError(message);
    } finally {
      setPendingPluginId(null);
    }
  }

  useEffect(() => {
    void loadStatuses();
  }, [loadStatuses]);

  const counts = useMemo(() => {
    const enabled = plugins.filter(
      (plugin) => plugin.enabled && plugin.user_enabled,
    ).length;
    const disabled = plugins.length - enabled;
    return { total: plugins.length, enabled, disabled };
  }, [plugins]);

  return (
    <section className="space-y-4">
      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <h1 className="text-xl font-semibold">All Plugins</h1>
            <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-300">
              Bundled internal plugins are loaded at startup. Plugins with missing requirements stay disabled and include guidance.
            </p>
          </div>
          <button
            type="button"
            onClick={() => void loadStatuses()}
            disabled={loading}
            className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-xs font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-900"
          >
            {loading ? "Refreshing..." : "Refresh"}
          </button>
        </div>
        <div className="mt-4 flex flex-wrap gap-2 text-xs">
          <span className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">
            total {counts.total}
          </span>
          <span className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">
            enabled {counts.enabled}
          </span>
          <span className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">
            disabled {counts.disabled}
          </span>
        </div>
      </article>

      {error ? (
        <p className="rounded-md border border-red-700 bg-red-600 px-3 py-2 text-sm text-white">{error}</p>
      ) : null}

      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        {loading ? (
          <p className="text-sm text-zinc-600 dark:text-zinc-300">Loading plugin statuses...</p>
        ) : plugins.length === 0 ? (
          <p className="text-sm text-zinc-600 dark:text-zinc-300">No bundled plugins found.</p>
        ) : (
          <div className="space-y-3">
            {plugins.map((plugin) => {
              const effectivelyEnabled = plugin.enabled && plugin.user_enabled;
              return (
                <div
                  key={plugin.id}
                  className="rounded-xl border border-zinc-300 bg-zinc-50 p-3 dark:border-zinc-700 dark:bg-zinc-900"
                >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <p className="text-sm font-semibold">
                    {plugin.name} <span className="text-xs font-normal text-zinc-500">v{plugin.version}</span>
                  </p>
                  <span className="rounded-md border border-zinc-300 bg-white px-2 py-1 text-xs dark:border-zinc-700 dark:bg-zinc-950">
                    {effectivelyEnabled ? "enabled" : "disabled"}
                  </span>
                </div>
                <p className="mt-2 text-xs text-zinc-500 dark:text-zinc-400">{plugin.path}</p>
                <div className="mt-2 flex flex-wrap gap-2 text-xs">
                  <span className="rounded-md border border-zinc-300 bg-white px-2 py-1 dark:border-zinc-700 dark:bg-zinc-950">
                    imported {plugin.imported ? "yes" : "no"}
                  </span>
                  <span className="rounded-md border border-zinc-300 bg-white px-2 py-1 dark:border-zinc-700 dark:bg-zinc-950">
                    sensors {plugin.sensors.length}
                  </span>
                  <span className="rounded-md border border-zinc-300 bg-white px-2 py-1 dark:border-zinc-700 dark:bg-zinc-950">
                    actuators {plugin.actuators.length}
                  </span>
                  <span className="rounded-md border border-zinc-300 bg-white px-2 py-1 dark:border-zinc-700 dark:bg-zinc-950">
                    user {plugin.user_enabled ? "enabled" : "disabled"}
                  </span>
                </div>
                <div className="mt-2">
                  <button
                    type="button"
                    disabled={pendingPluginId === plugin.id}
                    onClick={() =>
                      void setPluginEnabled(plugin.id, !plugin.user_enabled)
                    }
                    className="rounded-md border border-zinc-300 bg-white px-3 py-2 text-xs font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-950"
                  >
                    {pendingPluginId === plugin.id
                      ? "Saving..."
                      : plugin.user_enabled
                        ? "Disable Plugin"
                        : "Enable Plugin"}
                  </button>
                </div>
                {plugin.status_message ? (
                  <p className="mt-2 rounded-md border border-zinc-300 bg-white px-2 py-2 text-xs text-zinc-700 dark:border-zinc-700 dark:bg-zinc-950 dark:text-zinc-300">
                    {plugin.status_message}
                  </p>
                ) : null}
                <div className="mt-2 grid gap-2 text-xs md:grid-cols-2">
                  <div>
                    <p className="font-medium">Sensors</p>
                    <p className="text-zinc-500 dark:text-zinc-400">
                      {plugin.sensors.length > 0 ? plugin.sensors.join(", ") : "none"}
                    </p>
                  </div>
                  <div>
                    <p className="font-medium">Actuators</p>
                    <p className="text-zinc-500 dark:text-zinc-400">
                      {plugin.actuators.length > 0 ? plugin.actuators.join(", ") : "none"}
                    </p>
                  </div>
                </div>
                </div>
              );
            })}
          </div>
        )}
      </article>
    </section>
  );
}
