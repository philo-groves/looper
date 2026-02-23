"use client";

import Link from "next/link";

import { useDashboardSocket } from "@/components/dashboard/useDashboardSocket";

export default function AllSensorsPage() {
  const { data, socketConnected, socketError } = useDashboardSocket();
  const sensors = data?.sensors ?? [];

  return (
    <section className="space-y-4">
      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <h1 className="text-xl font-semibold">All Sensors</h1>
        <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-300">
          View registered sensors and open individual sensor settings.
        </p>
      </article>

      {!socketConnected && socketError ? (
        <p className="rounded-md border border-red-700 bg-red-600 px-3 py-2 text-sm text-white">{socketError}</p>
      ) : null}

      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        {sensors.length === 0 ? (
          <p className="text-sm text-zinc-600 dark:text-zinc-300">No sensors found.</p>
        ) : (
          <div className="space-y-3">
            {sensors.map((sensor) => (
              <div
                key={sensor.name}
                className="flex items-center justify-between gap-3 rounded-xl border border-zinc-300 bg-zinc-50 p-3 dark:border-zinc-700 dark:bg-zinc-900"
              >
                <div>
                  <p className="text-sm font-semibold">{sensor.name}</p>
                  <p className="mt-1 text-xs text-zinc-500 dark:text-zinc-400">{sensor.description}</p>
                </div>
                <Link
                  href={`/sensors/${encodeURIComponent(sensor.name)}`}
                  className="rounded-md border border-zinc-300 bg-white px-3 py-2 text-xs font-medium dark:border-zinc-700 dark:bg-zinc-950"
                >
                  View / Edit
                </Link>
              </div>
            ))}
          </div>
        )}
      </article>
    </section>
  );
}
