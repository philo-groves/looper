"use client";

import { useCallback, useState } from "react";

import { ActuatorsPanel } from "@/components/dashboard/ActuatorsPanel";
import { LoopStatePanel } from "@/components/dashboard/LoopStatePanel";
import { SensorsPanel } from "@/components/dashboard/SensorsPanel";
import { SetupWizard } from "@/components/dashboard/SetupWizard";
import {
  DashboardPayload,
  EditableActuator,
  EditableSensor,
} from "@/components/dashboard/types";
import { useDashboardSocket } from "@/components/dashboard/useDashboardSocket";
import { useSetupFlow } from "@/components/dashboard/useSetupFlow";

function defaultPercepts(name: string): string[] {
  return [`${name}: incoming percept`, `${name}: incoming percept`, `${name}: incoming percept`];
}

function defaultActions(name: string): string[] {
  return [`${name}: queued action`, `${name}: queued action`, `${name}: queued action`];
}

function mergeSensors(
  existing: EditableSensor[],
  incoming: DashboardPayload["sensors"],
): EditableSensor[] {
  const mapped = incoming.map((sensor) => {
    const match = existing.find((item) => item.id === sensor.name);
    return (
      match ?? {
        id: sensor.name,
        name: sensor.name,
        policy: `Sensitivity: ${sensor.sensitivity_score}%`,
        recentPercepts: defaultPercepts(sensor.name),
      }
    );
  });

  return mapped.length > 0 ? mapped : existing;
}

function mergeActuators(
  existing: EditableActuator[],
  incoming: DashboardPayload["actuators"],
): EditableActuator[] {
  const mapped = incoming.map((actuator) => {
    const match = existing.find((item) => item.id === actuator.name);
    return (
      match ?? {
        id: actuator.name,
        name: actuator.name,
        policy: `HITL: ${actuator.require_hitl ? "Yes" : "No"}`,
        recentActions: defaultActions(actuator.name),
      }
    );
  });

  return mapped.length > 0 ? mapped : existing;
}

export function Dashboard() {
  const [sensors, setSensors] = useState<EditableSensor[]>([]);
  const [actuators, setActuators] = useState<EditableActuator[]>([]);

  const handleSnapshot = useCallback((snapshot: DashboardPayload) => {
    setSensors((existing) => mergeSensors(existing, snapshot.sensors));
    setActuators((existing) => mergeActuators(existing, snapshot.actuators));
  }, []);

  const { data, socketConnected, socketError, wsCommand } = useDashboardSocket(handleSnapshot);

  const snapshot = data;
  const setupRequired = snapshot ? snapshot.state.state === "setup" || !snapshot.state.configured : true;

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
      <SensorsPanel
        sensors={sensors}
        onAddSensor={() => {
          const next = sensors.length + 1;
          setSensors((current) => [
            ...current,
            {
              id: `sensor-${Date.now()}`,
              name: `New Sensor ${next}`,
              policy: "Sensitivity: 50%",
              recentPercepts: ["New percept", "New percept", "New percept"],
            },
          ]);
        }}
      />

      <LoopStatePanel
        loopState={snapshot?.loop_visualization}
        localModelLabel={localModelLabel}
        frontierModelLabel={frontierModelLabel}
        socketConnected={socketConnected}
        socketError={socketError}
      />

      <ActuatorsPanel
        actuators={actuators}
        onAddActuator={() => {
          const next = actuators.length + 1;
          setActuators((current) => [
            ...current,
            {
              id: `actuator-${Date.now()}`,
              name: `New Actuator ${next}`,
              policy: "Rate limit: none",
              recentActions: ["New action", "New action", "New action"],
            },
          ]);
        }}
      />
    </section>
  );
}
