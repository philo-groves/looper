type SensorFormFieldsProps = {
  enabled: boolean;
  onEnabledChange: (nextValue: boolean) => void;
  sensitivity: number;
  onSensitivityChange: (nextValue: number) => void;
  description: string;
  onDescriptionChange: (nextValue: string) => void;
  singular: string;
  onSingularChange: (nextValue: string) => void;
  plural: string;
  onPluralChange: (nextValue: string) => void;
};

export function SensorFormFields({
  enabled,
  onEnabledChange,
  sensitivity,
  onSensitivityChange,
  description,
  onDescriptionChange,
  singular,
  onSingularChange,
  plural,
  onPluralChange,
}: SensorFormFieldsProps) {
  return (
    <>
      <label className="flex items-center gap-2 text-sm font-medium">
        <input
          type="checkbox"
          checked={enabled}
          onChange={(event) => onEnabledChange(event.target.checked)}
          className="h-4 w-4"
        />
        Enabled
      </label>

      <div className="space-y-2">
        <label className="text-sm font-medium">Sensitivity Score (0-100)</label>
        <input
          type="number"
          min={0}
          max={100}
          value={sensitivity}
          onChange={(event) =>
            onSensitivityChange(Math.max(0, Math.min(100, Number(event.target.value) || 0)))
          }
          className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
        />
      </div>

      <div className="space-y-2">
        <label className="text-sm font-medium">About the Percepts</label>
        <textarea
          value={description}
          onChange={(event) => onDescriptionChange(event.target.value)}
          rows={2}
          className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
        />
      </div>

      <div className="grid gap-3 sm:grid-cols-2">
        <div className="space-y-2">
          <label className="text-sm font-medium">Percept Singular Name</label>
          <input
            value={singular}
            onChange={(event) => onSingularChange(event.target.value)}
            className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
          />
        </div>
        <div className="space-y-2">
          <label className="text-sm font-medium">Percept Plural Name</label>
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
