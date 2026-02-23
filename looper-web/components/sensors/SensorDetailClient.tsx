"use client";

import { useEffect, useMemo, useState } from "react";

import { useDashboardSocket } from "@/components/dashboard/useDashboardSocket";
import { SensorIngressConfig } from "@/components/dashboard/types";
import { SensorFormFields } from "@/components/sensors/SensorFormFields";
import { SensorIngressFields } from "@/components/sensors/SensorIngressFields";

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
  const [ingress, setIngress] = useState<SensorIngressConfig>({ type: "rest_api", format: "text" });
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
        description: "Conversational messages that should always be considered surprising.",
        enabled: true,
        sensitivity_score: 100,
        queued_percepts: 0,
        unread_percepts: 0,
        percept_singular_name: "Incoming Message",
        percept_plural_name: "Incoming Messages",
        ingress: { type: "internal" as const },
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
    setIngress(sensor.ingress);
  }, [sensor]);

  async function save() {
    if (!sensorName) {
      return;
    }
    if (ingress.type === "directory" && !ingress.path.trim()) {
      setError("Directory path is required when Percept Source is Directory.");
      setStatus(null);
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
        ingress,
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
            <SensorFormFields
              enabled={enabled}
              onEnabledChange={setEnabled}
              sensitivity={sensitivity}
              onSensitivityChange={setSensitivity}
              description={description}
              onDescriptionChange={setDescription}
              singular={singular}
              onSingularChange={setSingular}
              plural={plural}
              onPluralChange={setPlural}
            />

            <SensorIngressFields
              sensorName={sensorName}
              ingress={ingress}
              onIngressChange={setIngress}
            />

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
