"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";

import { useDashboardSocket } from "@/components/dashboard/useDashboardSocket";
import { SensorFormFields } from "@/components/sensors/SensorFormFields";
import { SensorIngressFields } from "@/components/sensors/SensorIngressFields";
import { SensorIngressConfig } from "@/components/dashboard/types";

type CreateSensorResponse = {
  status: string;
  name: string;
};

function normalizePerceptName(value: string) {
  return value.trim().toLowerCase();
}

export function AddSensorClient() {
  const router = useRouter();
  const { socketConnected, socketError, wsCommand } = useDashboardSocket();

  const [name, setName] = useState("");
  const [enabled, setEnabled] = useState(true);
  const [sensitivity, setSensitivity] = useState(50);
  const [description, setDescription] = useState("");
  const [singular, setSingular] = useState("");
  const [plural, setPlural] = useState("");
  const [ingress, setIngress] = useState<SensorIngressConfig>({ type: "rest_api", format: "text" });
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function save() {
    const trimmedName = name.trim();
    if (!trimmedName) {
      setError("Sensor name is required.");
      setStatus(null);
      return;
    }

    const trimmedDescription = description.trim();
    if (!trimmedDescription) {
      setError("About the percepts is required.");
      setStatus(null);
      return;
    }

    const singularName = normalizePerceptName(singular) || normalizePerceptName(trimmedName);
    const pluralName =
      normalizePerceptName(plural) ||
      (singularName.endsWith("s") ? singularName : `${singularName}s`);

    if (ingress.type === "directory" && !ingress.path.trim()) {
      setError("Directory path is required when Percept Source is Directory.");
      setStatus(null);
      return;
    }

    setSaving(true);
    setStatus(null);
    setError(null);

    try {
      const response = await fetch("/api/agent/sensors", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          name: trimmedName,
          description: trimmedDescription,
          sensitivity_score: sensitivity,
          ingress,
        }),
      });

      if (!response.ok) {
        throw new Error(`Failed to create sensor (status ${response.status}).`);
      }

      const payload = (await response.json()) as CreateSensorResponse;
      const createdName = payload.name || trimmedName;

      await wsCommand("update_sensor", {
        name: createdName,
        enabled,
        sensitivity_score: sensitivity,
        description: trimmedDescription,
        percept_singular_name: singularName,
        percept_plural_name: pluralName,
        ingress,
      });

      setStatus("Sensor created.");
      router.push(`/sensors/${encodeURIComponent(createdName)}`);
    } catch (saveError) {
      const message = saveError instanceof Error ? saveError.message : "Failed to create sensor.";
      setError(message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="space-y-4">
      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <h1 className="text-xl font-semibold">Add a Sensor</h1>
        <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-300">
          Create a new sensor and configure how its percepts should be interpreted.
        </p>
      </article>

      {!socketConnected && socketError ? (
        <p className="rounded-md border border-red-700 bg-red-600 px-3 py-2 text-sm text-white">{socketError}</p>
      ) : null}

      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <div className="space-y-4">
          <div className="space-y-2">
            <label className="text-sm font-medium">Sensor Name</label>
            <input
              value={name}
              onChange={(event) => setName(event.target.value)}
              className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
              placeholder="e.g. inbox"
            />
          </div>

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

          <SensorIngressFields sensorName={name} ingress={ingress} onIngressChange={setIngress} />

          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={() => void save()}
              disabled={saving || !socketConnected}
              className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
            >
              {saving ? "Creating..." : "Create Sensor"}
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
      </article>
    </section>
  );
}
