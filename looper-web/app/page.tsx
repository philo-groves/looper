type AgentHealth = {
  status: string;
};

async function fetchAgentHealth(): Promise<AgentHealth | null> {
  try {
    const agentBaseUrl =
      process.env.LOOPER_AGENT_URL ?? "http://127.0.0.1:10001";
    const response = await fetch(`${agentBaseUrl}/api/health`, {
      cache: "no-store",
    });
    if (!response.ok) {
      return null;
    }

    return (await response.json()) as AgentHealth;
  } catch {
    return null;
  }
}

export default async function Home() {
  const health = await fetchAgentHealth();

  return (
    <div className="flex min-h-screen items-center justify-center bg-zinc-100 text-zinc-900">
      <main className="w-full max-w-xl rounded-xl border border-zinc-300 bg-white p-8 shadow-sm">
        <h1 className="text-2xl font-semibold">Looper Web Interface</h1>
        <p className="mt-3 text-sm text-zinc-600">
          This app now talks to the long-running <code>looper-agent</code> process.
        </p>

        <div className="mt-6 rounded-lg border border-zinc-200 bg-zinc-50 p-4">
          <p className="text-sm font-medium">Agent Health</p>
          <p className="mt-1 text-sm text-zinc-700">
            {health?.status === "ok"
              ? "Connected (status: ok)"
              : "Not reachable at http://127.0.0.1:10001"}
          </p>
        </div>
      </main>
    </div>
  );
}
