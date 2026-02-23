"use client";

import Link from "next/link";
import { useEffect, useMemo, useState } from "react";

import { useDashboardSocket } from "@/components/dashboard/useDashboardSocket";

type ChatSession = {
  id: string;
  title: string;
  created_at_unix_ms: number;
  updated_at_unix_ms: number;
  message_count: number;
};

function formatTimestamp(unixMs: number) {
  return new Date(unixMs).toLocaleString();
}

export function ChatHistoryClient() {
  const [conversations, setConversations] = useState<ChatSession[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const { wsCommand } = useDashboardSocket();

  useEffect(() => {
    let cancelled = false;

    async function loadSessions() {
      try {
        const response = await wsCommand<{ chats: ChatSession[] }>("list_chat_sessions", {
          limit: 100,
        });
        if (!cancelled) {
          setConversations(response.chats);
          setLoadError(null);
        }
      } catch (error) {
        if (!cancelled) {
          setLoadError(error instanceof Error ? error.message : "Failed to load chat history.");
        }
      }
    }

    void loadSessions();
    const timer = window.setInterval(() => {
      void loadSessions();
    }, 1500);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [wsCommand]);

  const hasConversations = useMemo(() => conversations.length > 0, [conversations]);

  return (
    <section className="rounded-xl border border-zinc-300 bg-white p-5 dark:border-zinc-700 dark:bg-zinc-950">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-xl font-semibold">Chat History</h1>
          <p className="mt-1 text-sm text-zinc-600 dark:text-zinc-300">
            Conversations are saved locally after the first message is sent.
          </p>
        </div>
        <Link
          href="/conversations/new-chat"
          className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium hover:bg-zinc-200 dark:border-zinc-700 dark:bg-zinc-900 dark:hover:bg-zinc-800"
        >
          New Chat
        </Link>
      </div>

      {!hasConversations ? (
        <div className="mt-5 rounded-lg border border-dashed border-zinc-300 p-6 text-sm text-zinc-600 dark:border-zinc-700 dark:text-zinc-300">
          {loadError ?? "No chats in history yet. Start a new chat and send a message."}
        </div>
      ) : (
        <ul className="mt-5 space-y-3">
          {conversations.map((conversation) => (
            <li key={conversation.id}>
              <Link
                href={`/conversations/chat/${conversation.id}`}
                className="block rounded-lg border border-zinc-300 bg-zinc-50 p-4 hover:bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-900 dark:hover:bg-zinc-800"
              >
                <p className="text-sm font-semibold">{conversation.title}</p>
                <p className="mt-1 text-xs text-zinc-500 dark:text-zinc-400">
                  Updated {formatTimestamp(conversation.updated_at_unix_ms)}
                </p>
                <p className="mt-2 text-sm text-zinc-700 dark:text-zinc-200">
                  {conversation.message_count} messages
                </p>
              </Link>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
