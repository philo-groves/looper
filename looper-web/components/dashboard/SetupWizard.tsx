import { ReactNode } from "react";

import { SETUP_STEPS, SetupStepId } from "@/components/dashboard/types";

type SetupWizardProps = {
  activeSetupSteps: SetupStepId[];
  setupStep: SetupStepId;
  setupError: string | null;
  setupInfo: string | null;
  socketConnected: boolean;
  setupIndex: number;
  setupBusy: boolean;
  onBack: () => void;
  onNext: () => void;
  onComplete: () => void;
  setupContent: ReactNode;
};

export function SetupWizard({
  activeSetupSteps,
  setupStep,
  setupError,
  setupInfo,
  socketConnected,
  setupIndex,
  setupBusy,
  onBack,
  onNext,
  onComplete,
  setupContent,
}: SetupWizardProps) {
  return (
    <section className="mx-auto w-full max-w-4xl rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
      <div className="flex items-center justify-between gap-3">
        <h1 className="text-2xl font-semibold">Looper Setup</h1>
      </div>

        <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-300">
          Setup mode matches terminal setup steps. Workspace features unlock after setup completes.
        </p>

        <div className="mt-4 grid gap-4 md:grid-cols-2">
          <div className="rounded-xl border border-zinc-300 bg-zinc-50 p-3 dark:border-zinc-700 dark:bg-zinc-900">
            <p className="text-sm font-semibold">Setup Steps</p>
            <ul className="mt-2 space-y-1 text-sm">
              {SETUP_STEPS.filter((item) => activeSetupSteps.includes(item.id)).map((step) => (
                <li
                  key={step.id}
                  className={`rounded-md px-2 py-1 ${
                    step.id === setupStep
                      ? "bg-zinc-200 font-medium dark:bg-zinc-800"
                      : "text-zinc-600 dark:text-zinc-300"
                  }`}
                >
                  {step.label}
                </li>
              ))}
            </ul>
          </div>

          <div className="rounded-xl border border-zinc-300 bg-zinc-50 p-3 dark:border-zinc-700 dark:bg-zinc-900">
            <p className="text-sm font-semibold">Current Step</p>
            <div className="mt-2">{setupContent}</div>

            {setupError ? (
              <p className="mt-3 rounded-md border border-red-700 bg-red-600 px-3 py-2 text-sm text-white">
                {setupError}
              </p>
            ) : null}
            {setupInfo ? (
              <p className="mt-3 rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-800">
                {setupInfo}
              </p>
            ) : null}
            {!socketConnected ? (
              <p className="mt-3 rounded-md border border-red-700 bg-red-600 px-3 py-2 text-sm text-white">
                Agent connection required for setup.
              </p>
            ) : null}

            <div className="mt-4 flex gap-2">
              <button
                type="button"
                onClick={onBack}
                disabled={setupIndex <= 0 || setupBusy}
                className="rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-950"
              >
                Back
              </button>
              {setupStep !== "install_model" ? (
                <button
                  type="button"
                  onClick={onNext}
                  disabled={setupBusy}
                  className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
                >
                  Continue
                </button>
              ) : (
                <button
                  type="button"
                  onClick={onComplete}
                  disabled={setupBusy || !socketConnected}
                  className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
                >
                  {setupBusy ? "Completing..." : "Complete Setup"}
                </button>
              )}
            </div>
          </div>
        </div>
    </section>
  );
}
