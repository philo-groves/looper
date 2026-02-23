"use client";

import { useEffect, useMemo, useState } from "react";

import { ActionsPanel, ActionListItem } from "@/components/dashboard/ActionsPanel";
import { LoopStatePanel } from "@/components/dashboard/LoopStatePanel";
import { PerceptsPanel, PerceptListItem } from "@/components/dashboard/PerceptsPanel";
import { SetupWizard } from "@/components/dashboard/SetupWizard";
import { useDashboardSocket } from "@/components/dashboard/useDashboardSocket";
import { useSetupFlow } from "@/components/dashboard/useSetupFlow";
import { ReadOnlyDetailsModal } from "@/components/window/ReadOnlyDetailsModal";

type PersistedPercept = {
  sensor_name: string;
  content: string;
};

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
  sensed_percepts: PersistedPercept[];
  surprising_percepts: PersistedPercept[];
  planned_actions: PersistedAction[];
  action_results: PersistedActionResult[];
};

type IterationsResponse = {
  iterations: PersistedIteration[];
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

export function Dashboard() {
  type DashboardPerceptItem = PerceptListItem & {
    sensorName: string | null;
    content: string | null;
    iterationId: number | null;
  };

  type DashboardActionItem = ActionListItem & {
    actuatorName: string;
    iterationId: number | null;
    resultKind: "Executed" | "Denied" | "RequiresHitl" | "Pending";
    resultContent: string | null;
  };

  const [percepts, setPercepts] = useState<DashboardPerceptItem[]>([]);
  const [actions, setActions] = useState<DashboardActionItem[]>([]);
  const [lastIterationId, setLastIterationId] = useState<number | null>(null);
  const [perceptsCleared, setPerceptsCleared] = useState(false);
  const [actionsCleared, setActionsCleared] = useState(false);
  const [selectedPercept, setSelectedPercept] = useState<DashboardPerceptItem | null>(null);
  const [selectedAction, setSelectedAction] = useState<DashboardActionItem | null>(null);

  const { data, socketConnected, socketError, wsCommand } = useDashboardSocket();

  const snapshot = data;
  const setupRequired = snapshot ? snapshot.state.state === "setup" || !snapshot.state.configured : false;

  useEffect(() => {
    if (!snapshot || setupRequired) {
      return;
    }

    const currentSnapshot = snapshot;

    const latestId = currentSnapshot.state.latest_iteration_id;
    if (!latestId || (lastIterationId !== null && latestId <= lastIterationId)) {
      return;
    }

    let cancelled = false;

    async function syncActivity() {
      try {
        const response = await wsCommand<IterationsResponse>("list_iterations", {
          after_id: lastIterationId,
          limit: 100,
        });

        if (cancelled || response.iterations.length === 0) {
          if (!cancelled) {
            setLastIterationId(latestId);
          }
          return;
        }

        const sensorNames = new Map(
          currentSnapshot.sensors.map((sensor) => [sensor.name, sensor.percept_singular_name]),
        );
        const actuatorNames = new Map(
          currentSnapshot.actuators.map((actuator) => [actuator.name, actuator.action_singular_name]),
        );

        const nextPercepts: DashboardPerceptItem[] = [];
        const nextActions: DashboardActionItem[] = [];

        for (const iteration of response.iterations) {
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
            nextPercepts.push({
              id: `percept-${iteration.id}-${index}`,
              title: titleCase(sensorNames.get(percept.sensor_name) ?? "percept"),
              timestamp: timestampText(iteration.created_at_unix),
              status: isSurprise ? "Surprise" : "No Surprise",
              sensorName: percept.sensor_name,
              content: percept.content,
              iterationId: iteration.id,
            });
          }

          for (let index = 0; index < iteration.planned_actions.length; index += 1) {
            const action = iteration.planned_actions[index];
            const result = iteration.action_results[index];
            const details = actionResultDetails(result);
            nextActions.push({
              id: `action-${iteration.id}-${index}`,
              title: titleCase(actuatorNames.get(action.actuator_name) ?? "action"),
              timestamp: timestampText(iteration.created_at_unix),
              status: actionStatus(result),
              actuatorName: action.actuator_name,
              iterationId: iteration.id,
              resultKind: details.resultKind,
              resultContent: details.resultContent,
            });
          }
        }

        if (!perceptsCleared) {
          setPercepts((existing) => [...nextPercepts.reverse(), ...existing].slice(0, 60));
        }
        if (!actionsCleared) {
          setActions((existing) => [...nextActions.reverse(), ...existing].slice(0, 60));
        }
        setLastIterationId(response.iterations[response.iterations.length - 1].id);
      } catch {
        setLastIterationId(latestId);
      }
    }

    void syncActivity();
    return () => {
      cancelled = true;
    };
  }, [snapshot, setupRequired, lastIterationId, wsCommand, perceptsCleared, actionsCleared]);

  const {
    setupStep,
    activeSetupSteps,
    setupIndex,
    setupBusy,
    setupError,
    setupInfo,
    setupContent,
    goSetupBack,
    goSetupNext,
    completeSetup,
  } = useSetupFlow({
    socketConnected,
    setupRequired,
    wsCommand,
  });

  const pendingPercepts = useMemo(() => {
    if (perceptsCleared) {
      return [] as DashboardPerceptItem[];
    }
    if (!snapshot) {
      return [] as DashboardPerceptItem[];
    }
    const items: DashboardPerceptItem[] = [];
    for (const sensor of snapshot.sensors) {
      const count = Math.min(sensor.unread_percepts, 6);
      for (let index = 0; index < count; index += 1) {
        items.push({
          id: `pending-percept-${sensor.name}-${index}`,
          title: titleCase(sensor.percept_singular_name),
          timestamp: "Waiting to be processed",
          status: "Pending",
          sensorName: sensor.name,
          content: null,
          iterationId: null,
        });
      }
    }
    return items;
  }, [snapshot, perceptsCleared]);

  const pendingActions = useMemo(() => {
    if (actionsCleared) {
      return [] as DashboardActionItem[];
    }
    if (!snapshot || snapshot.pending_approval_count <= 0) {
      return [] as DashboardActionItem[];
    }
    const fallback = titleCase(snapshot.actuators[0]?.action_singular_name ?? "action");
    const items: DashboardActionItem[] = [];
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
  }, [snapshot, actionsCleared]);

  if (!snapshot) {
    return (
      <section className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <h1 className="text-xl font-semibold">Dashboard</h1>
        <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-300">
          {socketConnected
            ? "Loading agent state..."
            : socketError ?? "Waiting for agent connection..."}
        </p>
      </section>
    );
  }

  if (setupRequired) {
    return (
      <SetupWizard
        activeSetupSteps={activeSetupSteps}
        setupStep={setupStep}
        setupError={setupError}
        setupInfo={setupInfo}
        socketConnected={socketConnected}
        setupIndex={setupIndex}
        setupBusy={setupBusy}
        onBack={goSetupBack}
        onNext={goSetupNext}
        onComplete={() => void completeSetup()}
        setupContent={setupContent}
      />
    );
  }

  const localModelLabel = `${snapshot?.local_model.provider ?? "Local"} / ${snapshot?.local_model.model ?? "Unassigned"}`;
  const frontierModelLabel = `${snapshot?.frontier_model.provider ?? "Frontier"} / ${snapshot?.frontier_model.model ?? "Unassigned"}`;

  return (
    <section className="grid gap-5 lg:grid-cols-12">
      <PerceptsPanel
        items={[...pendingPercepts, ...percepts].slice(0, 60)}
        onItemClick={(item) => setSelectedPercept(item)}
        onClear={() => {
          setPercepts([]);
          setPerceptsCleared(true);
          setSelectedPercept(null);
        }}
      />

      <LoopStatePanel
        loopState={snapshot?.loop_visualization}
        localModelLabel={localModelLabel}
        frontierModelLabel={frontierModelLabel}
        socketConnected={socketConnected}
        socketError={socketError}
      />

      <ActionsPanel
        items={[...pendingActions, ...actions].slice(0, 60)}
        onItemClick={(item) => setSelectedAction(item)}
        onClear={() => {
          setActions([]);
          setActionsCleared(true);
          setSelectedAction(null);
        }}
      />

      <ReadOnlyDetailsModal
        open={selectedPercept !== null}
        title={selectedPercept?.title ?? "Percept"}
        dialogTitleId="dashboard-percept-details-title"
        onClose={() => setSelectedPercept(null)}
        fields={
          selectedPercept
            ? [
                { label: "Sensor", value: selectedPercept.sensorName ?? "-" },
                { label: "Status", value: selectedPercept.status },
                { label: "Observed", value: selectedPercept.timestamp },
                { label: "Iteration", value: selectedPercept.iterationId ?? "Pending" },
              ]
            : []
        }
        contentLabel="Percept Content"
        content={selectedPercept?.content ?? null}
        emptyContentText="This percept is still pending processing, so detailed content is not available yet."
      />

      <ReadOnlyDetailsModal
        open={selectedAction !== null}
        title={selectedAction?.title ?? "Action"}
        dialogTitleId="dashboard-action-details-title"
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
