"use client";

import { useEffect, useMemo, useState } from "react";

import { useDashboardSocket } from "@/components/dashboard/useDashboardSocket";

type SensorDetailClientProps = {
  sensorName: string;
};

export function SensorDetailClient({ sensorName }: SensorDetailClientProps) {
  const { data, socketConnected, socketError, wsCommand } = useDashboardSocket();

  const [enabled, setEnabled] = useState(true);
  const [sensitivity, setSensitivity] = useState(50);
  const [description, setDescription] = useState("");
  const [singular, setSingular] = useState("");
  const [plural, setPlural] = useState("");
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const sensor = useMemo(() => {
    if (!sensorName) {
      return null;
    }
    if (!data) {
      return null;
    }

    const exact = data.sensors.find((item) => item.name === sensorName);
    if (exact) {
      return exact;
    }

    const caseInsensitive = data.sensors.find(
      (item) => item.name.toLowerCase() === sensorName.toLowerCase(),
    );
    if (caseInsensitive) {
      return caseInsensitive;
    }

    if (sensorName.toLowerCase() === "chat") {
      return {
        name: "chat",
        description: "Receiver of chat messages in percept form",
        enabled: true,
        sensitivity_score: 100,
        queued_percepts: 0,
        unread_percepts: 0,
        percept_singular_name: "Incoming Message",
        percept_plural_name: "Incoming Messages",
      };
    }

    return null;
  }, [data, sensorName]);

  useEffect(() => {
    if (!sensor) {
      return;
    }
    setEnabled(sensor.enabled);
    setSensitivity(sensor.sensitivity_score);
    setDescription(sensor.description);
    setSingular(sensor.percept_singular_name);
    setPlural(sensor.percept_plural_name);
  }, [sensor]);

  async function save() {
    if (!sensorName) {
      return;
    }

    setSaving(true);
    setStatus(null);
    setError(null);
    try {
      await wsCommand("update_sensor", {
        name: sensorName,
        enabled,
        sensitivity_score: sensitivity,
        description,
        percept_singular_name: singular,
        percept_plural_name: plural,
      });
      setStatus("Sensor settings updated.");
    } catch (saveError) {
      const message = saveError instanceof Error ? saveError.message : "Failed to save sensor.";
      setError(message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="space-y-4">
      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <h1 className="text-xl font-semibold">Sensor: {sensorName || "..."}</h1>
        <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-300">
          View and edit sensor configuration.
        </p>
      </article>

      {!socketConnected && socketError ? (
        <p className="rounded-md border border-red-700 bg-red-600 px-3 py-2 text-sm text-white">{socketError}</p>
      ) : null}

      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        {!data ? (
          <p className="text-sm text-zinc-600 dark:text-zinc-300">Loading sensor details...</p>
        ) : !sensor ? (
          <p className="text-sm text-zinc-600 dark:text-zinc-300">Sensor not found.</p>
        ) : (
          <div className="space-y-4">
            <label className="flex items-center gap-2 text-sm font-medium">
              <input
                type="checkbox"
                checked={enabled}
                onChange={(event) => setEnabled(event.target.checked)}
                className="h-4 w-4"
              />
              Enabled
            </label>

            <div className="space-y-2">
              <label className="text-sm font-medium">Sensitivity Score (0-100)</label>
              <input
                type="number"
                min={0}
                max={100}
                value={sensitivity}
                onChange={(event) =>
                  setSensitivity(Math.max(0, Math.min(100, Number(event.target.value) || 0)))
                }
                className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
              />
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">Description</label>
              <textarea
                value={description}
                onChange={(event) => setDescription(event.target.value)}
                rows={2}
                className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
              />
            </div>

            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-2">
                <label className="text-sm font-medium">Percept Singular Name</label>
                <input
                  value={singular}
                  onChange={(event) => setSingular(event.target.value)}
                  className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
                />
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium">Percept Plural Name</label>
                <input
                  value={plural}
                  onChange={(event) => setPlural(event.target.value)}
                  className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
                />
              </div>
            </div>

            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={() => void save()}
                disabled={saving || !socketConnected}
                className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
              >
                {saving ? "Saving..." : "Save Sensor"}
              </button>
              {status ? (
                <span className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-xs dark:border-zinc-700 dark:bg-zinc-800">
                  {status}
                </span>
              ) : null}
              {error ? (
                <span className="rounded-md border border-red-700 bg-red-600 px-3 py-2 text-xs text-white">
                  {error}
                </span>
              ) : null}
            </div>
          </div>
        )}
      </article>
    </section>
  );
}
