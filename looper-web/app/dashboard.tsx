"use client";

import { useCallback, useEffect, useState } from "react";

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
import { HeaderBar } from "@/components/window/HeaderBar";
import { SideNav } from "@/components/window/SideNav";

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

function statusPill(connected: boolean) {
  return connected
    ? "border border-green-700 bg-zinc-100 text-zinc-900 dark:bg-zinc-900 dark:text-zinc-100"
    : "border border-red-700 bg-red-600 text-white";
}

export function Dashboard() {
  const [theme, setTheme] = useState<"light" | "dark">("light");
  const [sensors, setSensors] = useState<EditableSensor[]>([]);
  const [actuators, setActuators] = useState<EditableActuator[]>([]);
  const [isSidebarOpen, setIsSidebarOpen] = useState(true);

  const handleSnapshot = useCallback((snapshot: DashboardPayload) => {
    setSensors((existing) => mergeSensors(existing, snapshot.sensors));
    setActuators((existing) => mergeActuators(existing, snapshot.actuators));
  }, []);

  const { data, socketConnected, socketError, wsCommand } = useDashboardSocket(handleSnapshot);

  useEffect(() => {
    document.documentElement.classList.toggle("dark", theme === "dark");
    window.localStorage.setItem("looper-theme", theme);
  }, [theme]);

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
        theme={theme}
        onToggleTheme={() => setTheme((current) => (current === "light" ? "dark" : "light"))}
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
    <main className="min-h-screen w-full bg-zinc-100 text-zinc-900 dark:bg-black dark:text-zinc-100">
      <div className="flex min-h-screen w-full">
        <SideNav isOpen={isSidebarOpen} onToggle={() => setIsSidebarOpen((current) => !current)} />

        <div className="flex min-w-0 flex-1 flex-col gap-5">
          <HeaderBar
            socketConnected={socketConnected}
            theme={theme}
            onToggleTheme={() => setTheme((current) => (current === "light" ? "dark" : "light"))}
            statusPillClassName={statusPill(socketConnected)}
          />

          <section className="grid gap-5 px-4 pb-4 sm:px-6 sm:pb-6 lg:grid-cols-12">
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
        </div>
      </div>
    </main>
  );
}
