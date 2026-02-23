"use client";

import { useEffect, useState } from "react";

import { useDashboardSocket } from "@/components/dashboard/useDashboardSocket";

type LoopConfigurationResponse = {
  interval_ms: number;
};

export function LoopConfigurationPanel() {
  const { socketConnected, socketError, wsCommand } = useDashboardSocket();

  const [intervalMs, setIntervalMs] = useState(500);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    async function loadConfiguration() {
      setLoading(true);
      setError(null);
      try {
        const payload = await wsCommand<LoopConfigurationResponse>("get_loop_configuration", {});
        if (cancelled) {
          return;
        }
        setIntervalMs(payload.interval_ms);
      } catch (loadError) {
        if (!cancelled) {
          const message =
            loadError instanceof Error ? loadError.message : "Failed to load loop configuration.";
          setError(message);
        }
      } finally {
        if (!cancelled) {
          setLoading(false);
        }
      }
    }

    void loadConfiguration();

    return () => {
      cancelled = true;
    };
  }, [wsCommand]);

  async function saveConfiguration() {
    if (!Number.isFinite(intervalMs) || intervalMs <= 0) {
      setError("Loop interval must be greater than 0 ms.");
      setStatus(null);
      return;
    }

    setSaving(true);
    setStatus(null);
    setError(null);
    try {
      const payload = await wsCommand<LoopConfigurationResponse>("update_loop_configuration", {
        interval_ms: Math.max(1, Math.round(intervalMs)),
      });
      setIntervalMs(payload.interval_ms);
      setStatus("Loop configuration saved.");
    } catch (saveError) {
      const message =
        saveError instanceof Error ? saveError.message : "Failed to save loop configuration.";
      setError(message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="space-y-4">
      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <h1 className="text-xl font-semibold">Loop Configuration</h1>
        <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-300">
          Configure the default interval between loop iterations. This value is used for auto-start and model-configuration-triggered starts.
        </p>
      </article>

      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        {loading ? (
          <p className="text-sm text-zinc-600 dark:text-zinc-300">Loading loop configuration...</p>
        ) : (
          <div className="space-y-4">
            <div className="space-y-2">
              <label className="text-sm font-medium">Default Loop Interval (ms)</label>
              <input
                type="number"
                min={1}
                step={1}
                value={intervalMs}
                onChange={(event) => setIntervalMs(Math.max(1, Number(event.target.value) || 1))}
                className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
              />
              <p className="text-xs text-zinc-500 dark:text-zinc-400">Default: 500 ms</p>
            </div>

            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={() => void saveConfiguration()}
                disabled={saving || !socketConnected}
                className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
              >
                {saving ? "Saving..." : "Save Loop Configuration"}
              </button>
            </div>
          </div>
        )}
      </article>

      {!socketConnected && socketError ? (
        <p className="rounded-md border border-red-700 bg-red-700 px-3 py-2 text-sm text-white">{socketError}</p>
      ) : null}
      {error ? (
        <p className="rounded-md border border-red-700 bg-red-700 px-3 py-2 text-sm text-white">{error}</p>
      ) : null}
      {status ? (
        <p className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-800">
          {status}
        </p>
      ) : null}
    </section>
  );
}
