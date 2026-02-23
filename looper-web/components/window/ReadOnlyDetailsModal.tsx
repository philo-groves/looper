"use client";

import { ReactNode, useEffect } from "react";

type DetailField = {
  label: string;
  value: ReactNode;
};

type ReadOnlyDetailsModalProps = {
  open: boolean;
  title: string;
  dialogTitleId: string;
  onClose: () => void;
  fields: DetailField[];
  contentLabel: string;
  content: string | null;
  emptyContentText: string;
};

export function ReadOnlyDetailsModal({
  open,
  title,
  dialogTitleId,
  onClose,
  fields,
  contentLabel,
  content,
  emptyContentText,
}: ReadOnlyDetailsModalProps) {
  useEffect(() => {
    if (!open) {
      return;
    }

    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        onClose();
      }
    }

    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [open, onClose]);

  if (!open) {
    return null;
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby={dialogTitleId}
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <div
        className="w-full max-w-2xl rounded-2xl border border-zinc-300 bg-white p-5 shadow-lg dark:border-zinc-700 dark:bg-zinc-950"
        onClick={(event) => event.stopPropagation()}
      >
        <div className="flex items-start justify-between gap-3">
          <div>
            <h2 id={dialogTitleId} className="text-lg font-semibold">
              {title}
            </h2>
            <p className="mt-1 text-xs text-zinc-500 dark:text-zinc-400">Read-only details</p>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-zinc-300 bg-zinc-100 px-2 py-1 text-xs font-medium dark:border-zinc-700 dark:bg-zinc-900"
          >
            Close
          </button>
        </div>

        <dl className="mt-4 grid gap-3 text-sm sm:grid-cols-2">
          {fields.map((field, index) => (
            <div
              key={`${field.label}-${index}`}
              className="rounded-lg border border-zinc-300 bg-zinc-50 p-3 dark:border-zinc-700 dark:bg-zinc-900"
            >
              <dt className="text-xs text-zinc-500 dark:text-zinc-400">{field.label}</dt>
              <dd className="mt-1 font-medium">{field.value}</dd>
            </div>
          ))}
        </dl>

        <div className="mt-4">
          <p className="text-xs text-zinc-500 dark:text-zinc-400">{contentLabel}</p>
          {content ? (
            <pre className="mt-2 max-h-72 overflow-auto whitespace-pre-wrap rounded-lg border border-zinc-300 bg-zinc-50 p-3 text-sm text-zinc-800 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-200">
              {content}
            </pre>
          ) : (
            <p className="mt-2 rounded-lg border border-zinc-300 bg-zinc-50 p-3 text-sm text-zinc-600 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300">
              {emptyContentText}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
