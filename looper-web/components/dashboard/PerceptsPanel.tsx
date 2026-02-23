type PerceptStatus = "Pending" | "No Surprise" | "Surprise";

export type PerceptListItem = {
  id: string;
  title: string;
  timestamp: string;
  status: PerceptStatus;
};

type PerceptsPanelProps = {
  items: PerceptListItem[];
};

function statusClass(status: PerceptStatus) {
  if (status === "Surprise") {
    return "border-zinc-900 bg-zinc-900 text-white dark:border-zinc-100 dark:bg-zinc-100 dark:text-zinc-900";
  }
  if (status === "No Surprise") {
    return "border-zinc-300 bg-zinc-100 text-zinc-900 dark:border-zinc-700 dark:bg-zinc-800 dark:text-zinc-100";
  }
  return "border-amber-700 bg-amber-600 text-white";
}

export function PerceptsPanel({ items }: PerceptsPanelProps) {
  return (
    <article className="rounded-2xl border border-zinc-300 bg-white p-4 shadow-sm dark:border-zinc-700 dark:bg-zinc-950 lg:col-span-3">
      <h2 className="text-lg font-semibold">Percepts</h2>
      <div className="mt-4 space-y-3">
        {items.length === 0 ? (
          <p className="rounded-lg border border-zinc-200 bg-zinc-50 p-3 text-sm text-zinc-600 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300">
            No percepts yet.
          </p>
        ) : (
          items.map((item) => (
            <div
              key={item.id}
              className="flex items-center justify-between gap-3 rounded-xl border border-zinc-300 bg-zinc-50 p-3 dark:border-zinc-700 dark:bg-zinc-900"
            >
              <div className="min-w-0">
                <p className="truncate text-sm font-semibold">{item.title}</p>
                <p className="mt-1 text-xs text-zinc-500 dark:text-zinc-400">{item.timestamp}</p>
              </div>
              <span
                className={`shrink-0 rounded-md border px-2 py-1 text-xs font-medium ${statusClass(item.status)}`}
              >
                {item.status}
              </span>
            </div>
          ))
        )}
      </div>
    </article>
  );
}
