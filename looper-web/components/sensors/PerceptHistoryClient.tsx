"use client";

import { useEffect, useMemo, useRef, useState } from "react";

import { PerceptListItem, PerceptsPanel } from "@/components/dashboard/PerceptsPanel";
import { useDashboardSocket } from "@/components/dashboard/useDashboardSocket";
import { ReadOnlyDetailsModal } from "@/components/window/ReadOnlyDetailsModal";

type PersistedPercept = {
  sensor_name: string;
  content: string;
};

type PersistedIteration = {
  id: number;
  created_at_unix: number;
  sensed_percepts: PersistedPercept[];
  surprising_percepts: PersistedPercept[];
};

type IterationsResponse = {
  iterations: PersistedIteration[];
};

type HistoryPerceptListItem = PerceptListItem & {
  sensorName: string;
  content: string;
  iterationId: number;
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

function buildHistoryPercepts(
  iterations: PersistedIteration[],
  sensorNames: Map<string, string>,
): HistoryPerceptListItem[] {
  const items: HistoryPerceptListItem[] = [];

  for (const iteration of iterations) {
    const surprisingPool = [...iteration.surprising_percepts];
    for (let index = 0; index < iteration.sensed_percepts.length; index += 1) {
      const percept = iteration.sensed_percepts[index];
      const surpriseIndex = surprisingPool.findIndex(
        (candidate) =>
          candidate.sensor_name === percept.sensor_name && candidate.content === percept.content,
      );
      const isSurprise = surpriseIndex >= 0;
      if (isSurprise) {
        surprisingPool.splice(surpriseIndex, 1);
      }

      items.push({
        id: `percept-${iteration.id}-${index}`,
        iterationId: iteration.id,
        sensorName: percept.sensor_name,
        content: percept.content,
        title: titleCase(sensorNames.get(percept.sensor_name) ?? "percept"),
        timestamp: timestampText(iteration.created_at_unix),
        status: isSurprise ? "Surprise" : "No Surprise",
      });
    }
  }

  return items;
}

export function PerceptHistoryClient() {
  const { data: snapshot, socketConnected, socketError, wsCommand } = useDashboardSocket();
  const sensorNamesRef = useRef<Map<string, string>>(new Map());
  const [historyItems, setHistoryItems] = useState<HistoryPerceptListItem[]>([]);
  const [lastLoadedIterationId, setLastLoadedIterationId] = useState<number | null>(null);
  const [selectedPercept, setSelectedPercept] = useState<HistoryPerceptListItem | null>(null);
  const [initialLoadDone, setInitialLoadDone] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  const latestIterationId = snapshot?.state.latest_iteration_id ?? null;

  useEffect(() => {
    sensorNamesRef.current = new Map(
      (snapshot?.sensors ?? []).map((sensor) => [sensor.name, sensor.percept_singular_name]),
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

        const next = buildHistoryPercepts(response.iterations, sensorNamesRef.current);
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
          setLoadError("Unable to load percept history.");
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

  const pendingPercepts = useMemo(() => {
    if (!snapshot) {
      return [] as PerceptListItem[];
    }

    const items: PerceptListItem[] = [];
    for (const sensor of snapshot.sensors) {
      const count = Math.min(sensor.unread_percepts, 6);
      for (let index = 0; index < count; index += 1) {
        items.push({
          id: `pending-percept-${sensor.name}-${index}`,
          title: titleCase(sensor.percept_singular_name),
          timestamp: "Waiting to be processed",
          status: "Pending",
        });
      }
    }
    return items;
  }, [snapshot]);

  return (
    <section className="space-y-4">
      <header className="rounded-2xl border border-zinc-300 bg-white p-4 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <h1 className="text-xl font-semibold">Percept History</h1>
        <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-300">
          {socketConnected
            ? "Select a processed percept to inspect read-only details."
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
          Loading percept history...
        </p>
      ) : (
        <PerceptsPanel
          title="Percepts"
          items={[...pendingPercepts, ...historyItems].slice(0, 200)}
          emptyText="No percept history yet."
          onItemClick={(item) => {
            const selected = historyItems.find((candidate) => candidate.id === item.id);
            if (!selected) {
              return;
            }
            setSelectedPercept(selected);
          }}
        />
      )}

      <ReadOnlyDetailsModal
        open={selectedPercept !== null}
        title={selectedPercept?.title ?? "Percept"}
        dialogTitleId="percept-details-title"
        onClose={() => setSelectedPercept(null)}
        fields={
          selectedPercept
            ? [
                { label: "Sensor", value: selectedPercept.sensorName },
                { label: "Status", value: selectedPercept.status },
                { label: "Observed", value: selectedPercept.timestamp },
                { label: "Iteration", value: selectedPercept.iterationId },
              ]
            : []
        }
        contentLabel="Percept Content"
        content={selectedPercept?.content ?? null}
        emptyContentText="No percept content is available."
      />
    </section>
  );
}
