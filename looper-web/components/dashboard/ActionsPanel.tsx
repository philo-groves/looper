export type ActionStatus = "Pending" | "Running" | "Done" | "Error";

export type ActionListItem = {
  id: string;
  title: string;
  timestamp: string;
  status: ActionStatus;
};

type ActionsPanelProps<TItem extends ActionListItem> = {
  items: TItem[];
  onClear?: () => void;
  onItemClick?: (item: TItem) => void;
  title?: string;
  emptyText?: string;
};

function statusClass(status: ActionStatus) {
  if (status === "Done") {
    return "border-zinc-900 bg-zinc-900 text-white dark:border-zinc-100 dark:bg-zinc-100 dark:text-zinc-900";
  }
  if (status === "Running") {
    return "border-blue-700 bg-blue-600 text-white";
  }
  if (status === "Error") {
    return "border-red-700 bg-red-600 text-white";
  }
  return "border-amber-700 bg-amber-600 text-white";
}

export function ActionsPanel<TItem extends ActionListItem>({
  items,
  onClear,
  onItemClick,
  title = "Actions",
  emptyText = "No actions yet.",
}: ActionsPanelProps<TItem>) {
  return (
    <article className="rounded-2xl border border-zinc-300 bg-white p-4 shadow-sm dark:border-zinc-700 dark:bg-zinc-950 lg:col-span-3">
      <div className="flex items-center justify-between gap-3">
        <h2 className="text-lg font-semibold">{title}</h2>
        {onClear ? (
          <button
            type="button"
            onClick={onClear}
            className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 text-xs font-medium dark:border-zinc-700 dark:bg-zinc-900"
          >
            Clear
          </button>
        ) : null}
      </div>
      <div className="mt-4 space-y-3">
        {items.length === 0 ? (
          <p className="rounded-lg border border-zinc-200 bg-zinc-50 p-3 text-sm text-zinc-600 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300">
            {emptyText}
          </p>
        ) : (
          items.map((item) => (
            onItemClick ? (
              <button
                key={item.id}
                type="button"
                onClick={() => onItemClick(item)}
                className="flex w-full items-center justify-between gap-3 rounded-xl border border-zinc-300 bg-zinc-50 p-3 text-left transition-colors hover:bg-zinc-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-zinc-400 dark:border-zinc-700 dark:bg-zinc-900 dark:hover:bg-zinc-800 dark:focus-visible:ring-zinc-500"
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
              </button>
            ) : (
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
            )
          ))
        )}
      </div>
    </article>
  );
}
