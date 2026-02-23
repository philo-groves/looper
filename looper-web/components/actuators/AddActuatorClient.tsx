"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";

import { ActuatorFormFields } from "@/components/actuators/ActuatorFormFields";
import { useDashboardSocket } from "@/components/dashboard/useDashboardSocket";

type CreateActuatorResponse = {
  status: string;
  name: string;
};

type RateLimitPeriod = "minute" | "hour" | "day" | "week" | "month";
type ActuatorRegistrationType = "mcp" | "workflow";

function normalizeActionName(value: string) {
  return value.trim().toLowerCase();
}

export function AddActuatorClient() {
  const router = useRouter();
  const { socketConnected, socketError, wsCommand } = useDashboardSocket();

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [registrationType, setRegistrationType] = useState<ActuatorRegistrationType>("workflow");

  const [requireHitl, setRequireHitl] = useState(false);
  const [sandboxed, setSandboxed] = useState(false);
  const [rateLimitEnabled, setRateLimitEnabled] = useState(false);
  const [rateLimitMax, setRateLimitMax] = useState(1);
  const [rateLimitPer, setRateLimitPer] = useState<RateLimitPeriod>("hour");
  const [singular, setSingular] = useState("");
  const [plural, setPlural] = useState("");

  const [mcpName, setMcpName] = useState("");
  const [mcpConnectionType, setMcpConnectionType] = useState<"http" | "stdio">("http");
  const [mcpUrl, setMcpUrl] = useState("");

  const [workflowName, setWorkflowName] = useState("");
  const [workflowCells, setWorkflowCells] = useState("");

  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function save() {
    const trimmedName = name.trim();
    if (!trimmedName) {
      setError("Actuator name is required.");
      setStatus(null);
      return;
    }

    const trimmedDescription = description.trim();
    if (!trimmedDescription) {
      setError("About this actuator is required.");
      setStatus(null);
      return;
    }

    const singularName = normalizeActionName(singular) || normalizeActionName(trimmedName);
    const pluralName =
      normalizeActionName(plural) ||
      (singularName.endsWith("s") ? singularName : `${singularName}s`);

    if (rateLimitEnabled && rateLimitMax <= 0) {
      setError("Rate limit max must be greater than 0.");
      setStatus(null);
      return;
    }

    let details: object;
    if (registrationType === "mcp") {
      if (!mcpName.trim()) {
        setError("MCP display name is required.");
        setStatus(null);
        return;
      }
      if (!mcpUrl.trim()) {
        setError("MCP URL or executable path is required.");
        setStatus(null);
        return;
      }
      details = {
        name: mcpName.trim(),
        type: mcpConnectionType,
        url: mcpUrl.trim(),
      };
    } else {
      const cells = workflowCells
        .split("\n")
        .map((cell) => cell.trim())
        .filter((cell) => cell.length > 0);

      if (!workflowName.trim()) {
        setError("Workflow name is required.");
        setStatus(null);
        return;
      }
      if (cells.length === 0) {
        setError("At least one workflow cell is required.");
        setStatus(null);
        return;
      }

      details = {
        name: workflowName.trim(),
        cells,
      };
    }

    setSaving(true);
    setStatus(null);
    setError(null);

    try {
      const response = await fetch("/api/agent/actuators", {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          name: trimmedName,
          description: trimmedDescription,
          type: registrationType,
          details,
          policy: {
            require_hitl: requireHitl,
            sandboxed,
            rate_limit: rateLimitEnabled ? { max: Math.max(1, rateLimitMax), per: rateLimitPer } : null,
          },
        }),
      });

      if (!response.ok) {
        throw new Error(`Failed to create actuator (status ${response.status}).`);
      }

      const payload = (await response.json()) as CreateActuatorResponse;
      const createdName = payload.name || trimmedName;

      await wsCommand("update_actuator", {
        name: createdName,
        description: trimmedDescription,
        require_hitl: requireHitl,
        sandboxed,
        rate_limit: rateLimitEnabled ? { max: Math.max(1, rateLimitMax), per: rateLimitPer } : null,
        action_singular_name: singularName,
        action_plural_name: pluralName,
      });

      setStatus("Actuator created.");
      router.push(`/actuators/${encodeURIComponent(createdName)}`);
    } catch (saveError) {
      const message = saveError instanceof Error ? saveError.message : "Failed to create actuator.";
      setError(message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="space-y-4">
      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <h1 className="text-xl font-semibold">Add an Actuator</h1>
        <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-300">
          Create a new actuator and configure how its actions should be managed.
        </p>
      </article>

      {!socketConnected && socketError ? (
        <p className="rounded-md border border-red-700 bg-red-600 px-3 py-2 text-sm text-white">{socketError}</p>
      ) : null}

      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <div className="space-y-4">
          <div className="space-y-2">
            <label className="text-sm font-medium">Actuator Name</label>
            <input
              value={name}
              onChange={(event) => setName(event.target.value)}
              className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
              placeholder="e.g. docs_search"
            />
          </div>

          <div className="space-y-2">
            <label className="text-sm font-medium">Actuator Type</label>
            <select
              value={registrationType}
              onChange={(event) => setRegistrationType(event.target.value as ActuatorRegistrationType)}
              className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
            >
              <option value="workflow">Workflow</option>
              <option value="mcp">MCP</option>
            </select>
          </div>

          {registrationType === "mcp" ? (
            <div className="space-y-3 rounded-xl border border-zinc-300 bg-zinc-50 p-3 dark:border-zinc-700 dark:bg-zinc-900">
              <div className="space-y-2">
                <label className="text-sm font-medium">MCP Display Name</label>
                <input
                  value={mcpName}
                  onChange={(event) => setMcpName(event.target.value)}
                  className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-950"
                  placeholder="e.g. Docs MCP"
                />
              </div>

              <div className="space-y-2">
                <label className="text-sm font-medium">Connection Type</label>
                <select
                  value={mcpConnectionType}
                  onChange={(event) => setMcpConnectionType(event.target.value as "http" | "stdio")}
                  className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-950"
                >
                  <option value="http">HTTP</option>
                  <option value="stdio">Stdio</option>
                </select>
              </div>

              <div className="space-y-2">
                <label className="text-sm font-medium">URL / Executable Path</label>
                <input
                  value={mcpUrl}
                  onChange={(event) => setMcpUrl(event.target.value)}
                  className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-950"
                  placeholder={mcpConnectionType === "http" ? "https://example.com/mcp" : "/usr/bin/mcp-server"}
                />
              </div>
            </div>
          ) : (
            <div className="space-y-3 rounded-xl border border-zinc-300 bg-zinc-50 p-3 dark:border-zinc-700 dark:bg-zinc-900">
              <div className="space-y-2">
                <label className="text-sm font-medium">Workflow Name</label>
                <input
                  value={workflowName}
                  onChange={(event) => setWorkflowName(event.target.value)}
                  className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-950"
                  placeholder="e.g. Knowledge Lookup Workflow"
                />
              </div>

              <div className="space-y-2">
                <label className="text-sm font-medium">Workflow Cells (one per line)</label>
                <textarea
                  value={workflowCells}
                  onChange={(event) => setWorkflowCells(event.target.value)}
                  rows={4}
                  className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 font-mono text-sm dark:border-zinc-700 dark:bg-zinc-950"
                  placeholder="search docs for API\nextract relevant examples"
                />
              </div>
            </div>
          )}

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
              {saving ? "Creating..." : "Create Actuator"}
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
