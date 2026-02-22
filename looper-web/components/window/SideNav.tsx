type SideNavProps = {
  isOpen: boolean;
  onToggle: () => void;
};

export function SideNav({ isOpen, onToggle }: SideNavProps) {
  return (
    <aside
      className={`shrink-0 border-r border-zinc-300 bg-white transition-all duration-300 dark:border-zinc-800 dark:bg-zinc-950 ${
        isOpen ? "w-72" : "w-16"
      }`}
    >
      <div className="flex items-center justify-between border-b border-zinc-300 p-3 dark:border-zinc-800">
        {isOpen ? <p className="text-sm font-semibold">Looper Workspace</p> : <span className="text-xs">Nav</span>}
        <button
          type="button"
          onClick={onToggle}
          className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 text-xs font-medium dark:border-zinc-700 dark:bg-zinc-900"
        >
          {isOpen ? "Collapse" : "Expand"}
        </button>
      </div>

      <nav className="p-3 text-sm">
        {isOpen ? (
          <ul className="space-y-3">
            <li className="relative rounded-md bg-zinc-200 px-2 py-1 pl-4 font-medium dark:bg-zinc-800">
              <span className="absolute inset-y-0 left-0 w-1 rounded-l-md bg-zinc-500 dark:bg-zinc-400" />
              Dashboard
            </li>
            <li>
              <p className="rounded-md px-2 py-1 font-medium">Conversations</p>
              <ul className="mt-1 space-y-1 pl-4 text-zinc-600 dark:text-zinc-300">
                <li className="rounded-md px-2 py-1">New Chat</li>
                <li className="rounded-md px-2 py-1">Chat History</li>
              </ul>
            </li>
            <li>
              <p className="rounded-md px-2 py-1 font-medium">Sensors</p>
              <ul className="mt-1 space-y-1 pl-4 text-zinc-600 dark:text-zinc-300">
                <li className="rounded-md px-2 py-1">Add a Sensor</li>
                <li className="rounded-md px-2 py-1">All Sensors</li>
                <li className="rounded-md px-2 py-1">Percept History</li>
              </ul>
            </li>
            <li>
              <p className="rounded-md px-2 py-1 font-medium">Actuators</p>
              <ul className="mt-1 space-y-1 pl-4 text-zinc-600 dark:text-zinc-300">
                <li className="rounded-md px-2 py-1">Add an Actuator</li>
                <li className="rounded-md px-2 py-1">All Actuators</li>
                <li className="rounded-md px-2 py-1">Action History</li>
              </ul>
            </li>
            <li>
              <p className="rounded-md px-2 py-1 font-medium">Agent Settings</p>
              <ul className="mt-1 space-y-1 pl-4 text-zinc-600 dark:text-zinc-300">
                <li className="rounded-md px-2 py-1">Agent Identity</li>
                <li className="rounded-md px-2 py-1">Loop Configuration</li>
                <li className="rounded-md px-2 py-1">Providers &amp; Models</li>
              </ul>
            </li>
          </ul>
        ) : (
          <ul className="space-y-2 text-center text-xs font-medium">
            <li className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">D</li>
            <li className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">C</li>
            <li className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">S</li>
            <li className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">A</li>
            <li className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">G</li>
          </ul>
        )}
      </nav>
    </aside>
  );
}
