"use client";

import { ReactNode, useEffect, useState } from "react";

import { useDashboardSocket } from "@/components/dashboard/useDashboardSocket";
import { HeaderBar } from "@/components/window/HeaderBar";
import { SideNav } from "@/components/window/SideNav";

type WorkspaceShellProps = {
  children: ReactNode;
};

function statusPill(connected: boolean) {
  return connected
    ? "border border-green-700 bg-zinc-100 text-green-700 dark:bg-zinc-900 dark:text-green-500"
    : "border border-red-700 bg-zinc-100 text-red-700 dark:bg-zinc-900 dark:text-red-500";
}

export function WorkspaceShell({ children }: WorkspaceShellProps) {
  const [isSidebarOpen, setIsSidebarOpen] = useState(true);
  const [theme, setTheme] = useState<"light" | "dark">("light");
  const { socketConnected } = useDashboardSocket();

  useEffect(() => {
    document.documentElement.classList.toggle("dark", theme === "dark");
    window.localStorage.setItem("looper-theme", theme);
  }, [theme]);

  return (
    <main className="min-h-screen w-full bg-zinc-100 text-zinc-900 dark:bg-black dark:text-zinc-100">
      <div className="flex min-h-screen w-full">
        <SideNav isOpen={isSidebarOpen} onToggle={() => setIsSidebarOpen((current) => !current)} />

        <div className="flex min-w-0 flex-1 flex-col gap-5">
          <HeaderBar
            socketConnected={socketConnected}
            theme={theme}
            onToggleTheme={() => setTheme((current) => (current === "light" ? "dark" : "light"))}
            statusPillClassName={statusPill(socketConnected)}
          />

          <div className="px-4 pb-4 sm:px-6 sm:pb-6">{children}</div>
        </div>
      </div>
    </main>
  );
}
