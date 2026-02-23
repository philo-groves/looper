"use client";

import { useEffect, useMemo, useState } from "react";

import { ActuatorFormFields } from "@/components/actuators/ActuatorFormFields";
import { useDashboardSocket } from "@/components/dashboard/useDashboardSocket";

type ActuatorDetailClientProps = {
  actuatorName: string;
};

type RateLimitPeriod = "minute" | "hour" | "day" | "week" | "month";

function normalizeActionName(value: string) {
  return value.trim().toLowerCase();
}

export function ActuatorDetailClient({ actuatorName }: ActuatorDetailClientProps) {
  const { data, socketConnected, socketError, wsCommand } = useDashboardSocket();

  const [description, setDescription] = useState("");
  const [requireHitl, setRequireHitl] = useState(false);
  const [sandboxed, setSandboxed] = useState(false);
  const [singular, setSingular] = useState("");
  const [plural, setPlural] = useState("");
  const [rateLimitEnabled, setRateLimitEnabled] = useState(false);
  const [rateLimitMax, setRateLimitMax] = useState(1);
  const [rateLimitPer, setRateLimitPer] = useState<RateLimitPeriod>("hour");
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const actuator = useMemo(() => {
    if (!actuatorName || !data) {
      return null;
    }

    const exact = data.actuators.find((item) => item.name === actuatorName);
    if (exact) {
      return exact;
    }

    return data.actuators.find((item) => item.name.toLowerCase() === actuatorName.toLowerCase()) ?? null;
  }, [actuatorName, data]);

  useEffect(() => {
    if (!actuator) {
      return;
    }
    setDescription(actuator.description);
    setRequireHitl(actuator.require_hitl);
    setSandboxed(actuator.sandboxed);
    setSingular(actuator.action_singular_name);
    setPlural(actuator.action_plural_name);
    setRateLimitEnabled(actuator.rate_limit !== null);
    setRateLimitMax(Math.max(1, actuator.rate_limit?.max ?? 1));
    setRateLimitPer((actuator.rate_limit?.per as RateLimitPeriod) ?? "hour");
  }, [actuator]);

  async function save() {
    if (!actuator) {
      return;
    }

    const trimmedDescription = description.trim();
    if (!trimmedDescription) {
      setError("Description is required.");
      setStatus(null);
      return;
    }

    const singularName = normalizeActionName(singular);
    if (!singularName) {
      setError("Action singular name is required.");
      setStatus(null);
      return;
    }

    const pluralName = normalizeActionName(plural) || (singularName.endsWith("s") ? singularName : `${singularName}s`);

    if (rateLimitEnabled && rateLimitMax <= 0) {
      setError("Rate limit max must be greater than 0.");
      setStatus(null);
      return;
    }

    setSaving(true);
    setStatus(null);
    setError(null);
    try {
      await wsCommand("update_actuator", {
        name: actuator.name,
        description: trimmedDescription,
        require_hitl: requireHitl,
        sandboxed,
        rate_limit: rateLimitEnabled ? { max: Math.max(1, rateLimitMax), per: rateLimitPer } : null,
        action_singular_name: singularName,
        action_plural_name: pluralName,
      });
      setPlural(pluralName);
      setStatus("Actuator settings updated.");
    } catch (saveError) {
      const message = saveError instanceof Error ? saveError.message : "Failed to save actuator.";
      setError(message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="space-y-4">
      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <h1 className="text-xl font-semibold">Actuator: {actuatorName || "..."}</h1>
        <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-300">
          View and edit actuator configuration.
        </p>
      </article>

      {!socketConnected && socketError ? (
        <p className="rounded-md border border-red-700 bg-red-600 px-3 py-2 text-sm text-white">{socketError}</p>
      ) : null}

      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        {!data ? (
          <p className="text-sm text-zinc-600 dark:text-zinc-300">Loading actuator details...</p>
        ) : !actuator ? (
          <p className="text-sm text-zinc-600 dark:text-zinc-300">Actuator not found.</p>
        ) : (
          <div className="space-y-4">
            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-2">
                <label className="text-sm font-medium">Actuator Kind</label>
                <input
                  value={actuator.kind}
                  disabled
                  className="w-full rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
                />
              </div>
              <div className="space-y-2">
                <label className="text-sm font-medium">Policy Summary</label>
                <input
                  value={`allowlist ${actuator.allowlist_count} / denylist ${actuator.denylist_count}`}
                  disabled
                  className="w-full rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
                />
              </div>
            </div>

            <ActuatorFormFields
              description={description}
              onDescriptionChange={setDescription}
              requireHitl={requireHitl}
              onRequireHitlChange={setRequireHitl}
              sandboxed={sandboxed}
              onSandboxedChange={setSandboxed}
              rateLimitEnabled={rateLimitEnabled}
              onRateLimitEnabledChange={setRateLimitEnabled}
              rateLimitMax={rateLimitMax}
              onRateLimitMaxChange={setRateLimitMax}
              rateLimitPer={rateLimitPer}
              onRateLimitPerChange={setRateLimitPer}
              singular={singular}
              onSingularChange={setSingular}
              plural={plural}
              onPluralChange={setPlural}
            />

            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={() => void save()}
                disabled={saving || !socketConnected}
                className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
              >
                {saving ? "Saving..." : "Save Actuator"}
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
