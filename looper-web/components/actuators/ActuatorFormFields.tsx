type RateLimitPeriod = "minute" | "hour" | "day" | "week" | "month";

type ActuatorFormFieldsProps = {
  description: string;
  onDescriptionChange: (nextValue: string) => void;
  requireHitl: boolean;
  onRequireHitlChange: (nextValue: boolean) => void;
  sandboxed: boolean;
  onSandboxedChange: (nextValue: boolean) => void;
  rateLimitEnabled: boolean;
  onRateLimitEnabledChange: (nextValue: boolean) => void;
  rateLimitMax: number;
  onRateLimitMaxChange: (nextValue: number) => void;
  rateLimitPer: RateLimitPeriod;
  onRateLimitPerChange: (nextValue: RateLimitPeriod) => void;
  singular: string;
  onSingularChange: (nextValue: string) => void;
  plural: string;
  onPluralChange: (nextValue: string) => void;
};

export function ActuatorFormFields({
  description,
  onDescriptionChange,
  requireHitl,
  onRequireHitlChange,
  sandboxed,
  onSandboxedChange,
  rateLimitEnabled,
  onRateLimitEnabledChange,
  rateLimitMax,
  onRateLimitMaxChange,
  rateLimitPer,
  onRateLimitPerChange,
  singular,
  onSingularChange,
  plural,
  onPluralChange,
}: ActuatorFormFieldsProps) {
  return (
    <>
      <div className="space-y-2">
        <label className="text-sm font-medium">About this Actuator</label>
        <textarea
          value={description}
          onChange={(event) => onDescriptionChange(event.target.value)}
          rows={3}
          className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
        />
      </div>

      <div className="grid gap-3 sm:grid-cols-2">
        <label className="flex items-center gap-2 text-sm font-medium">
          <input
            type="checkbox"
            checked={requireHitl}
            onChange={(event) => onRequireHitlChange(event.target.checked)}
            className="h-4 w-4"
          />
          Require Human Approval (HITL)
        </label>

        <label className="flex items-center gap-2 text-sm font-medium">
          <input
            type="checkbox"
            checked={sandboxed}
            onChange={(event) => onSandboxedChange(event.target.checked)}
            className="h-4 w-4"
          />
          Run in Sandbox
        </label>
      </div>

      <div className="space-y-3 rounded-xl border border-zinc-300 bg-zinc-50 p-3 dark:border-zinc-700 dark:bg-zinc-900">
        <label className="flex items-center gap-2 text-sm font-medium">
          <input
            type="checkbox"
            checked={rateLimitEnabled}
            onChange={(event) => onRateLimitEnabledChange(event.target.checked)}
            className="h-4 w-4"
          />
          Enable Rate Limit
        </label>

        {rateLimitEnabled ? (
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="space-y-2">
              <label className="text-sm font-medium">Max Executions</label>
              <input
                type="number"
                min={1}
                value={rateLimitMax}
                onChange={(event) => onRateLimitMaxChange(Math.max(1, Number(event.target.value) || 1))}
                className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-950"
              />
            </div>

            <div className="space-y-2">
              <label className="text-sm font-medium">Per</label>
              <select
                value={rateLimitPer}
                onChange={(event) => onRateLimitPerChange(event.target.value as RateLimitPeriod)}
                className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-950"
              >
                <option value="minute">Minute</option>
                <option value="hour">Hour</option>
                <option value="day">Day</option>
                <option value="week">Week</option>
                <option value="month">Month</option>
              </select>
            </div>
          </div>
        ) : null}
      </div>

      <div className="grid gap-3 sm:grid-cols-2">
        <div className="space-y-2">
          <label className="text-sm font-medium">Action Singular Name</label>
          <input
            value={singular}
            onChange={(event) => onSingularChange(event.target.value)}
            className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
          />
        </div>

        <div className="space-y-2">
          <label className="text-sm font-medium">Action Plural Name</label>
          <input
            value={plural}
            onChange={(event) => onPluralChange(event.target.value)}
            className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
          />
        </div>
      </div>
    </>
  );
}
