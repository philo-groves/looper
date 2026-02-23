"use client";

import { useRef } from "react";

import { SensorIngressConfig } from "@/components/dashboard/types";

type SensorIngressFieldsProps = {
  sensorName: string;
  ingress: SensorIngressConfig;
  onIngressChange: (nextValue: SensorIngressConfig) => void;
};

type BrowserFileWithPath = File & {
  path?: string;
  webkitRelativePath?: string;
};

function toDirectoryPath(file: BrowserFileWithPath) {
  if (typeof file.path === "string" && file.path.length > 0) {
    const slashIndex = Math.max(file.path.lastIndexOf("/"), file.path.lastIndexOf("\\"));
    if (slashIndex > 0) {
      return file.path.slice(0, slashIndex);
    }
    return file.path;
  }

  if (file.webkitRelativePath) {
    const [root] = file.webkitRelativePath.split("/");
    return root || "";
  }

  return "";
}

export function SensorIngressFields({ sensorName, ingress, onIngressChange }: SensorIngressFieldsProps) {
  const pickerRef = useRef<HTMLInputElement | null>(null);
  const isInternalSensor = sensorName.trim().toLowerCase() === "chat";

  if (isInternalSensor) {
    return (
      <div className="space-y-2">
        <label className="text-sm font-medium">Percept Source</label>
        <p className="rounded-md border border-zinc-300 bg-zinc-50 px-3 py-2 text-sm text-zinc-600 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300">
          This sensor is managed internally and always receives chat messages directly from the runtime.
        </p>
      </div>
    );
  }

  const selectedType = ingress.type === "directory" ? "directory" : "rest_api";
  const restFormat = ingress.type === "rest_api" ? ingress.format : "text";

  return (
    <div className="space-y-3">
      <div className="space-y-2">
        <label className="text-sm font-medium">Percept Source</label>
        <select
          value={selectedType}
          onChange={(event) => {
            if (event.target.value === "directory") {
              onIngressChange({ type: "directory", path: ingress.type === "directory" ? ingress.path : "" });
              return;
            }
            onIngressChange({
              type: "rest_api",
              format: ingress.type === "rest_api" ? ingress.format : "text",
            });
          }}
          className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
        >
          <option value="directory">Directory</option>
          <option value="rest_api">REST API</option>
        </select>
      </div>

      {ingress.type === "directory" ? (
        <div className="space-y-2">
          <label className="text-sm font-medium">Directory Path</label>
          <div className="flex flex-col gap-2 sm:flex-row">
            <input
              value={ingress.path}
              onChange={(event) => onIngressChange({ type: "directory", path: event.target.value })}
              className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
              placeholder="C:\\path\\to\\percepts"
            />
            <button
              type="button"
              onClick={() => {
                if (!pickerRef.current) {
                  return;
                }
                pickerRef.current.setAttribute("webkitdirectory", "");
                pickerRef.current.setAttribute("directory", "");
                pickerRef.current.click();
              }}
              className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium dark:border-zinc-700 dark:bg-zinc-800"
            >
              Choose Folder
            </button>
            <input
              ref={pickerRef}
              type="file"
              multiple
              onChange={(event) => {
                const firstFile = event.target.files?.[0] as BrowserFileWithPath | undefined;
                if (!firstFile) {
                  return;
                }
                const path = toDirectoryPath(firstFile);
                if (path) {
                  onIngressChange({ type: "directory", path });
                }
              }}
              className="hidden"
            />
          </div>
          <p className="text-xs text-zinc-500 dark:text-zinc-400">
            Each new file becomes one percept. Supported content includes text, markdown, JSON, and CSV.
          </p>
        </div>
      ) : (
        <div className="space-y-2">
          <label className="text-sm font-medium">REST Payload Format</label>
          <select
            value={restFormat}
            onChange={(event) =>
              onIngressChange({
                type: "rest_api",
                format: event.target.value === "json" ? "json" : "text",
              })
            }
            className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
          >
            <option value="text">TEXT (plain or markdown)</option>
            <option value="json">JSON</option>
          </select>
          <p className="text-xs text-zinc-500 dark:text-zinc-400">
            Endpoint: <code>/api/sensors/{encodeURIComponent(sensorName)}/percepts</code>
          </p>
        </div>
      )}
    </div>
  );
}
