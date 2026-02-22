import { EditableSensor } from "@/components/dashboard/types";

type SensorsPanelProps = {
  sensors: EditableSensor[];
  onAddSensor: () => void;
};

export function SensorsPanel({ sensors, onAddSensor }: SensorsPanelProps) {
  return (
    <article className="rounded-2xl border border-zinc-300 bg-white p-4 shadow-sm dark:border-zinc-700 dark:bg-zinc-950 lg:col-span-3">
      <h2 className="text-lg font-semibold">Sensors</h2>
      <button
        type="button"
        onClick={onAddSensor}
        className="mt-3 w-full rounded-lg border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium transition hover:bg-zinc-200 dark:border-zinc-700 dark:bg-zinc-900 dark:hover:bg-zinc-800"
      >
        Add a Sensor
      </button>

      <div className="mt-4 space-y-3">
        {sensors.length === 0 ? (
          <p className="rounded-lg border border-zinc-200 bg-zinc-50 p-3 text-sm text-zinc-600 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300">
            No sensors registered.
          </p>
        ) : (
          sensors.map((sensor) => (
            <div
              key={sensor.id}
              className="rounded-xl border border-zinc-300 bg-zinc-50 p-3 dark:border-zinc-700 dark:bg-zinc-900"
            >
              <div className="flex items-center justify-between gap-3">
                <p className="text-sm font-semibold">{sensor.name}</p>
                <button
                  type="button"
                  className="rounded-md border border-zinc-300 bg-white px-2 py-1 text-xs font-medium transition hover:bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-950 dark:hover:bg-zinc-800"
                >
                  Edit
                </button>
              </div>
              <p className="mt-2 text-xs font-medium uppercase tracking-wide text-zinc-500 dark:text-zinc-400">
                Sensor Policy
              </p>
              <p className="mt-1 rounded-md border border-zinc-300 bg-white px-2 py-1 text-sm dark:border-zinc-700 dark:bg-zinc-950">
                {sensor.policy}
              </p>
              <p className="mt-2 text-xs font-medium uppercase tracking-wide text-zinc-500 dark:text-zinc-400">
                Recent Percepts
              </p>
              <div className="mt-1 space-y-1.5">
                {sensor.recentPercepts.slice(0, 3).map((percept, perceptIndex) => (
                  <p
                    key={`${sensor.id}-percept-${perceptIndex}`}
                    className="w-full rounded-md border border-zinc-300 bg-white px-2 py-1 text-sm dark:border-zinc-700 dark:bg-zinc-950"
                  >
                    {percept}
                  </p>
                ))}
              </div>
            </div>
          ))
        )}
      </div>
    </article>
  );
}
