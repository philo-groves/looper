import { DashboardPayload } from "@/components/dashboard/types";
import { LoopRing } from "@/components/dashboard/LoopRing";

const LOCAL_STEPS = ["No Surprise", "Gather New Percepts", "Check For Surprises"];
const FRONTIER_STEPS = [
  "Plan Actions",
  "Perform Actions",
  "Action Error / Information Needed",
];

function localStepIndex(
  step: DashboardPayload["loop_visualization"]["local_current_step"] | undefined,
): number | null {
  if (step === "no_surprise") {
    return 0;
  }
  if (step === "gather_new_percepts") {
    return 1;
  }
  if (step === "check_for_surprises") {
    return 2;
  }
  return null;
}

function frontierStepIndex(
  step: DashboardPayload["loop_visualization"]["frontier_current_step"] | undefined,
): number | null {
  if (step === "deeper_percept_investigation" || step === "plan_actions") {
    return 0;
  }
  if (step === "performing_actions") {
    return 1;
  }
  return null;
}

type LoopStatePanelProps = {
  loopState: DashboardPayload["loop_visualization"] | undefined;
  localModelLabel: string;
  frontierModelLabel: string;
  socketConnected: boolean;
  socketError: string | null;
};

export function LoopStatePanel({
  loopState,
  localModelLabel,
  frontierModelLabel,
  socketConnected,
  socketError,
}: LoopStatePanelProps) {
  const frontierWorking =
    loopState?.current_phase === "deeper_percept_investigation" ||
    loopState?.current_phase === "plan_actions" ||
    loopState?.current_phase === "execute_actions";

  const actionsCompletedState =
    loopState?.current_phase === "idle" && Boolean(loopState?.action_required);

  const surpriseFoundState = loopState?.current_phase === "deeper_percept_investigation";

  const localActiveStep = frontierWorking || actionsCompletedState
    ? null
    : localStepIndex(loopState?.local_current_step);

  const frontierActiveStep = actionsCompletedState
    ? null
    : frontierStepIndex(loopState?.frontier_current_step);

  return (
    <section className="space-y-4 rounded-2xl border border-zinc-300 bg-white p-4 shadow-sm dark:border-zinc-700 dark:bg-zinc-950 lg:col-span-6">
      <h2 className="text-lg font-semibold">Looper State</h2>

      <div className="relative rounded-2xl py-4">
        <div className="flex flex-row justify-between gap-4 xl:items-center">
          <LoopRing
            title="Local Model Loop"
            modelLabel={localModelLabel}
            steps={LOCAL_STEPS}
            activeStep={localActiveStep}
            totalLoops={loopState?.local_loop_count ?? 0}
            rotationDegrees={30}
          />

          <LoopRing
            title="Frontier Model Loop"
            modelLabel={frontierModelLabel}
            steps={FRONTIER_STEPS}
            activeStep={frontierActiveStep}
            totalLoops={loopState?.frontier_loop_count ?? 0}
            rotationDegrees={-30}
          />
        </div>

        <div className="pointer-events-none absolute inset-0 hidden xl:block">
          <div className="absolute inset-x-0 top-[5.4rem] flex flex-col items-center gap-2">
            <p
              className={`rounded-xl border px-2 py-1 text-center text-xs leading-tight transition-colors duration-200 ${
                surpriseFoundState
                  ? "border-zinc-300 bg-zinc-100 text-zinc-900 dark:border-zinc-600 dark:bg-zinc-800 dark:text-zinc-50"
                  : "border-zinc-200 bg-white text-zinc-600 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300"
              }`}
            >
              Surprise Found
            </p>
            <div
              className={`h-3.5 rounded-full transition-colors duration-200 ${
                surpriseFoundState ? "bg-zinc-900 dark:bg-zinc-100" : "bg-zinc-300 dark:bg-zinc-700"
              }`}
              style={{ width: "calc(100% - 462px)" }}
            />
          </div>

          <div className="absolute inset-x-0 bottom-14 flex flex-col items-center gap-2">
            <p
              className={`rounded-xl border px-2 py-1 text-center text-xs leading-tight transition-colors duration-200 ${
                actionsCompletedState
                  ? "border-zinc-300 bg-zinc-100 text-zinc-900 dark:border-zinc-600 dark:bg-zinc-800 dark:text-zinc-50"
                  : "border-zinc-200 bg-white text-zinc-600 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300"
              }`}
            >
              Actions Completed
            </p>
            <div
              className={`h-3.5 rounded-full transition-colors duration-200 ${
                actionsCompletedState ? "bg-zinc-900 dark:bg-zinc-100" : "bg-zinc-300 dark:bg-zinc-700"
              }`}
              style={{ width: "calc(100% - 462px)" }}
            />
          </div>
        </div>
      </div>

      <div className="grid gap-3 rounded-xl border border-zinc-300 bg-zinc-50 p-4 text-sm dark:border-zinc-700 dark:bg-zinc-900 sm:grid-cols-2">
        <div className="rounded-lg border border-zinc-300 bg-white p-3 dark:border-zinc-700 dark:bg-zinc-950">
          <p className="font-semibold">Local Branch</p>
          <p className="mt-1 text-zinc-600 dark:text-zinc-300">
            After Check For Surprises: {loopState?.surprise_found ? "Surprise Found" : "No Surprise"}
          </p>
        </div>
        <div className="rounded-lg border border-zinc-300 bg-white p-3 dark:border-zinc-700 dark:bg-zinc-950">
          <p className="font-semibold">Frontier Branch</p>
          <p className="mt-1 text-zinc-600 dark:text-zinc-300">
            After Plan Actions: {loopState?.action_required ? "Action Required" : "No Action Required"}
          </p>
        </div>
        <div className="rounded-lg border border-zinc-300 bg-white p-3 dark:border-zinc-700 dark:bg-zinc-950 sm:col-span-2">
          <p className="font-semibold">Loop Flow</p>
          <p className="mt-1 text-zinc-600 dark:text-zinc-300">
            {`Sensors -> Local Model -> ${
              loopState?.surprise_found ? "Frontier Model" : "Gather New Percepts"
            }${
              loopState?.surprise_found
                ? ` -> ${loopState.action_required ? "Actuators" : "No Action Required"} -> Gather New Percepts`
                : ""
            }`}
          </p>
        </div>
      </div>

      {!socketConnected && socketError ? (
        <p className="rounded-lg border border-zinc-300 bg-zinc-200 p-3 text-sm dark:border-zinc-700 dark:bg-zinc-800">
          {socketError}
        </p>
      ) : null}
    </section>
  );
}
