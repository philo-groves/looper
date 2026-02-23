"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";

type SideNavProps = {
  isOpen: boolean;
  onToggle: () => void;
};

export function SideNav({ isOpen, onToggle }: SideNavProps) {
  const pathname = usePathname();

  function isActive(path: string) {
    if (path === "/") {
      return pathname === "/";
    }
    return pathname === path || pathname.startsWith(`${path}/`);
  }

  function linkClass(path: string) {
    if (isActive(path)) {
      return "relative block w-full rounded-md bg-zinc-200 px-2 py-1 pl-4 font-medium dark:bg-zinc-800";
    }
    return "block w-full rounded-md px-2 py-1 text-zinc-700 hover:bg-zinc-100 dark:text-zinc-300 dark:hover:bg-zinc-900";
  }

  return (
    <aside
      className={`shrink-0 border-r border-zinc-300 bg-white transition-all duration-300 dark:border-zinc-800 dark:bg-zinc-950 ${
        isOpen ? "w-[250px]" : "w-16"
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
            <li className="w-full">
              <Link href="/" className={linkClass("/")}>
                {isActive("/") ? (
                  <span className="absolute inset-y-0 left-0 w-1 rounded-l-md bg-zinc-500 dark:bg-zinc-400" />
                ) : null}
                Dashboard
              </Link>
            </li>
            <li className="w-full">
              <p className="rounded-md px-2 py-1 font-medium">Conversations</p>
              <ul className="mt-1 space-y-1 pl-4 text-zinc-600 dark:text-zinc-300">
                <li className="w-full">
                  <Link href="/conversations/new-chat" className={linkClass("/conversations/new-chat")}>
                    {isActive("/conversations/new-chat") ? (
                      <span className="absolute inset-y-0 left-0 w-1 rounded-l-md bg-zinc-500 dark:bg-zinc-400" />
                    ) : null}
                    New Chat
                  </Link>
                </li>
                <li className="w-full">
                  <Link
                    href="/conversations/chat-history"
                    className={linkClass("/conversations/chat-history")}
                  >
                    {isActive("/conversations/chat-history") ? (
                      <span className="absolute inset-y-0 left-0 w-1 rounded-l-md bg-zinc-500 dark:bg-zinc-400" />
                    ) : null}
                    Chat History
                  </Link>
                </li>
              </ul>
            </li>
            <li className="w-full">
              <p className="rounded-md px-2 py-1 font-medium">Sensors</p>
              <ul className="mt-1 space-y-1 pl-4 text-zinc-600 dark:text-zinc-300">
                <li className="w-full">
                  <Link href="/sensors/add" className={linkClass("/sensors/add")}>
                    {isActive("/sensors/add") ? (
                      <span className="absolute inset-y-0 left-0 w-1 rounded-l-md bg-zinc-500 dark:bg-zinc-400" />
                    ) : null}
                    Add a Sensor
                  </Link>
                </li>
                <li className="w-full">
                  <Link href="/sensors/all" className={linkClass("/sensors/all")}>
                    {isActive("/sensors/all") ? (
                      <span className="absolute inset-y-0 left-0 w-1 rounded-l-md bg-zinc-500 dark:bg-zinc-400" />
                    ) : null}
                    All Sensors
                  </Link>
                </li>
                <li className="w-full">
                  <Link
                    href="/sensors/percept-history"
                    className={linkClass("/sensors/percept-history")}
                  >
                    {isActive("/sensors/percept-history") ? (
                      <span className="absolute inset-y-0 left-0 w-1 rounded-l-md bg-zinc-500 dark:bg-zinc-400" />
                    ) : null}
                    Percept History
                  </Link>
                </li>
              </ul>
            </li>
            <li className="w-full">
              <p className="rounded-md px-2 py-1 font-medium">Actuators</p>
              <ul className="mt-1 space-y-1 pl-4 text-zinc-600 dark:text-zinc-300">
                <li className="w-full">
                  <Link href="/actuators/add" className={linkClass("/actuators/add")}>
                    {isActive("/actuators/add") ? (
                      <span className="absolute inset-y-0 left-0 w-1 rounded-l-md bg-zinc-500 dark:bg-zinc-400" />
                    ) : null}
                    Add an Actuator
                  </Link>
                </li>
                <li className="w-full">
                  <Link href="/actuators/all" className={linkClass("/actuators/all")}>
                    {isActive("/actuators/all") ? (
                      <span className="absolute inset-y-0 left-0 w-1 rounded-l-md bg-zinc-500 dark:bg-zinc-400" />
                    ) : null}
                    All Actuators
                  </Link>
                </li>
                <li className="w-full">
                  <Link
                    href="/actuators/action-history"
                    className={linkClass("/actuators/action-history")}
                  >
                    {isActive("/actuators/action-history") ? (
                      <span className="absolute inset-y-0 left-0 w-1 rounded-l-md bg-zinc-500 dark:bg-zinc-400" />
                    ) : null}
                    Action History
                  </Link>
                </li>
              </ul>
            </li>
            <li className="w-full">
              <p className="rounded-md px-2 py-1 font-medium">Agent Settings</p>
              <ul className="mt-1 space-y-1 pl-4 text-zinc-600 dark:text-zinc-300">
                <li className="w-full">
                  <Link href="/agent-settings/identity" className={linkClass("/agent-settings/identity")}>
                    {isActive("/agent-settings/identity") ? (
                      <span className="absolute inset-y-0 left-0 w-1 rounded-l-md bg-zinc-500 dark:bg-zinc-400" />
                    ) : null}
                    Agent Identity
                  </Link>
                </li>
                <li className="w-full">
                  <Link
                    href="/agent-settings/loop-configuration"
                    className={linkClass("/agent-settings/loop-configuration")}
                  >
                    {isActive("/agent-settings/loop-configuration") ? (
                      <span className="absolute inset-y-0 left-0 w-1 rounded-l-md bg-zinc-500 dark:bg-zinc-400" />
                    ) : null}
                    Loop Configuration
                  </Link>
                </li>
                <li className="w-full">
                  <Link
                    href="/agent-settings/providers-models"
                    className={linkClass("/agent-settings/providers-models")}
                  >
                    {isActive("/agent-settings/providers-models") ? (
                      <span className="absolute inset-y-0 left-0 w-1 rounded-l-md bg-zinc-500 dark:bg-zinc-400" />
                    ) : null}
                    Providers &amp; Models
                  </Link>
                </li>
              </ul>
            </li>
          </ul>
        ) : (
          <ul className="space-y-2 text-center text-xs font-medium">
            <li className="w-full">
              <Link href="/" className="block rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">D</Link>
            </li>
            <li className="w-full">
              <Link href="/conversations/new-chat" className="block rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">C</Link>
            </li>
            <li className="w-full">
              <Link href="/sensors/all" className="block rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">S</Link>
            </li>
            <li className="w-full">
              <Link href="/actuators/all" className="block rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">A</Link>
            </li>
            <li className="w-full">
              <Link href="/agent-settings/identity" className="block rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900">G</Link>
            </li>
          </ul>
        )}
      </nav>
    </aside>
  );
}
