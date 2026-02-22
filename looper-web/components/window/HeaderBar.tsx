type HeaderBarProps = {
  socketConnected: boolean;
  theme: "light" | "dark";
  onToggleTheme: () => void;
  statusPillClassName: string;
};

export function HeaderBar({
  socketConnected,
  theme,
  onToggleTheme,
  statusPillClassName,
}: HeaderBarProps) {
  return (
    <header className="w-full border-b border-zinc-300 bg-white px-4 py-3 dark:border-zinc-700 dark:bg-zinc-950 sm:px-6">
      <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
        <span />
        <div className="flex items-center gap-3">
          <span className={`rounded-full px-3 py-1 text-xs font-medium ${statusPillClassName}`}>
            {socketConnected ? "Agent Connected" : "Agent Offline"}
          </span>
          <button
            type="button"
            onClick={onToggleTheme}
            className="rounded-lg border border-zinc-300 bg-zinc-100 px-3 py-1 text-xs font-medium transition hover:bg-zinc-200 dark:border-zinc-700 dark:bg-zinc-900 dark:hover:bg-zinc-800"
          >
            {theme === "light" ? "Switch to Dark" : "Switch to Light"}
          </button>
        </div>
      </div>
    </header>
  );
}
