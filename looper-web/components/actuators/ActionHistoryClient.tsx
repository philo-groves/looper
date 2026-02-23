"use client";

import { useEffect, useMemo, useRef, useState } from "react";

import { ActionListItem, ActionsPanel } from "@/components/dashboard/ActionsPanel";
import { useDashboardSocket } from "@/components/dashboard/useDashboardSocket";
import { ReadOnlyDetailsModal } from "@/components/window/ReadOnlyDetailsModal";

type PersistedActionResult =
  | { Executed: { output: string } }
  | { Denied: string }
  | { RequiresHitl: { approval_id: number } };

type PersistedAction = {
  actuator_name: string;
};

type PersistedIteration = {
  id: number;
  created_at_unix: number;
  planned_actions: PersistedAction[];
  action_results: PersistedActionResult[];
};

type IterationsResponse = {
  iterations: PersistedIteration[];
};

type HistoryActionListItem = ActionListItem & {
  actuatorName: string;
  iterationId: number | null;
  resultKind: "Executed" | "Denied" | "RequiresHitl" | "Pending";
  resultContent: string | null;
};

function titleCase(value: string) {
  if (!value) {
    return value;
  }
  return value.charAt(0).toUpperCase() + value.slice(1);
}

function timestampText(unixSeconds: number) {
  return new Date(unixSeconds * 1000).toLocaleString();
}

function actionStatus(result: PersistedActionResult | undefined): ActionListItem["status"] {
  if (!result) {
    return "Pending";
  }
  if ("Executed" in result) {
    return "Done";
  }
  if ("Denied" in result) {
    return "Error";
  }
  return "Pending";
}

function actionResultDetails(result: PersistedActionResult | undefined) {
  if (!result) {
    return {
      resultKind: "Pending" as const,
      resultContent: null,
    };
  }
  if ("Executed" in result) {
    return {
      resultKind: "Executed" as const,
      resultContent: result.Executed.output,
    };
  }
  if ("Denied" in result) {
    return {
      resultKind: "Denied" as const,
      resultContent: result.Denied,
    };
  }
  return {
    resultKind: "RequiresHitl" as const,
    resultContent: `Approval ID: ${result.RequiresHitl.approval_id}`,
  };
}

function buildHistoryActions(
  iterations: PersistedIteration[],
  actuatorNames: Map<string, string>,
): HistoryActionListItem[] {
  const items: HistoryActionListItem[] = [];

  for (const iteration of iterations) {
    for (let index = 0; index < iteration.planned_actions.length; index += 1) {
      const action = iteration.planned_actions[index];
      const result = iteration.action_results[index];
      const details = actionResultDetails(result);

      items.push({
        id: `action-${iteration.id}-${index}`,
        actuatorName: action.actuator_name,
        iterationId: iteration.id,
        title: titleCase(actuatorNames.get(action.actuator_name) ?? "action"),
        timestamp: timestampText(iteration.created_at_unix),
        status: actionStatus(result),
        resultKind: details.resultKind,
        resultContent: details.resultContent,
      });
    }
  }

  return items;
}

export function ActionHistoryClient() {
  const { data: snapshot, socketConnected, socketError, wsCommand } = useDashboardSocket();
  const actuatorNamesRef = useRef<Map<string, string>>(new Map());
  const [historyItems, setHistoryItems] = useState<HistoryActionListItem[]>([]);
  const [lastLoadedIterationId, setLastLoadedIterationId] = useState<number | null>(null);
  const [selectedAction, setSelectedAction] = useState<HistoryActionListItem | null>(null);
  const [initialLoadDone, setInitialLoadDone] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  const latestIterationId = snapshot?.state.latest_iteration_id ?? null;

  useEffect(() => {
    actuatorNamesRef.current = new Map(
      (snapshot?.actuators ?? []).map((actuator) => [actuator.name, actuator.action_singular_name]),
    );
  }, [snapshot]);

  useEffect(() => {
    let cancelled = false;

    async function syncHistory() {
      if (latestIterationId === null) {
        if (!cancelled) {
          setInitialLoadDone(true);
          setLoadError(null);
        }
        return;
      }

      if (lastLoadedIterationId !== null && latestIterationId <= lastLoadedIterationId) {
        if (!cancelled && !initialLoadDone) {
          setInitialLoadDone(true);
        }
        return;
      }

      try {
        const response = await wsCommand<IterationsResponse>("list_iterations", {
          after_id: lastLoadedIterationId,
          limit: 200,
        });

        if (cancelled) {
          return;
        }

        const next = buildHistoryActions(response.iterations, actuatorNamesRef.current);
        if (next.length > 0) {
          setHistoryItems((existing) => [...next.reverse(), ...existing].slice(0, 200));
        }

        setLastLoadedIterationId(
          response.iterations.length > 0
            ? response.iterations[response.iterations.length - 1].id
            : latestIterationId,
        );
        setLoadError(null);
      } catch {
        if (!cancelled) {
          setLoadError("Unable to load action history.");
          if (lastLoadedIterationId === null) {
            setLastLoadedIterationId(latestIterationId);
          }
        }
      } finally {
        if (!cancelled) {
          setInitialLoadDone(true);
        }
      }
    }

    void syncHistory();

    return () => {
      cancelled = true;
    };
  }, [initialLoadDone, lastLoadedIterationId, latestIterationId, wsCommand]);

  const pendingActions = useMemo(() => {
    if (!snapshot || snapshot.pending_approval_count <= 0) {
      return [] as HistoryActionListItem[];
    }

    const fallback = titleCase(snapshot.actuators[0]?.action_singular_name ?? "action");
    const items: HistoryActionListItem[] = [];
    for (let index = 0; index < Math.min(snapshot.pending_approval_count, 6); index += 1) {
      items.push({
        id: `pending-action-${index}`,
        title: fallback,
        timestamp: "Awaiting approval",
        status: snapshot.loop_visualization.current_phase === "execute_actions" ? "Running" : "Pending",
        actuatorName: "",
        iterationId: null,
        resultKind: "Pending",
        resultContent: null,
      });
    }
    return items;
  }, [snapshot]);

  return (
    <section className="space-y-4">
      <header className="rounded-2xl border border-zinc-300 bg-white p-4 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <h1 className="text-xl font-semibold">Action History</h1>
        <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-300">
          {socketConnected
            ? "Select an action to inspect read-only details."
            : socketError ?? "Waiting for agent connection..."}
        </p>
      </header>

      {loadError ? (
        <p className="rounded-lg border border-zinc-300 bg-zinc-50 p-3 text-sm text-zinc-700 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-200">
          {loadError}
        </p>
      ) : null}

      {!initialLoadDone ? (
        <p className="rounded-lg border border-zinc-300 bg-white p-3 text-sm text-zinc-600 shadow-sm dark:border-zinc-700 dark:bg-zinc-950 dark:text-zinc-300">
          Loading action history...
        </p>
      ) : (
        <ActionsPanel
          title="Actions"
          items={[...pendingActions, ...historyItems].slice(0, 200)}
          emptyText="No action history yet."
          onItemClick={(item) => setSelectedAction(item)}
        />
      )}

      <ReadOnlyDetailsModal
        open={selectedAction !== null}
        title={selectedAction?.title ?? "Action"}
        dialogTitleId="action-details-title"
        onClose={() => setSelectedAction(null)}
        fields={
          selectedAction
            ? [
                { label: "Actuator", value: selectedAction.actuatorName || "-" },
                { label: "Status", value: selectedAction.status },
                { label: "Observed", value: selectedAction.timestamp },
                { label: "Iteration", value: selectedAction.iterationId ?? "Pending" },
              ]
            : []
        }
        contentLabel="Action Result"
        content={selectedAction?.resultContent ?? null}
        emptyContentText="This action is pending, so no result content is available yet."
      />
    </section>
  );
}
