"use client";

import Link from "next/link";
import { FormEvent, useEffect, useMemo, useRef, useState } from "react";

import { useDashboardSocket } from "@/components/dashboard/useDashboardSocket";

type ChatMessage = {
  id: number;
  chat_id: string;
  role: string;
  content: string;
  created_at_unix_ms: number;
};

function mergeMessages(existing: ChatMessage[], incoming: ChatMessage[]): ChatMessage[] {
  if (incoming.length === 0) {
    return existing;
  }

  const byId = new Map<number, ChatMessage>();
  for (const message of existing) {
    byId.set(message.id, message);
  }
  for (const message of incoming) {
    byId.set(message.id, message);
  }

  return [...byId.values()].sort((left, right) => left.id - right.id);
}

function timestampLabel(unixMs: number) {
  return new Date(unixMs).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function ChatClient({ chatId }: { chatId: string }) {
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [draft, setDraft] = useState("");
  const [pendingResponses, setPendingResponses] = useState(0);
  const [latestMessageId, setLatestMessageId] = useState<number | null>(null);
  const [requestError, setRequestError] = useState<string | null>(null);

  const historyAnchorRef = useRef<HTMLDivElement | null>(null);

  const { socketConnected, socketError, wsCommand } = useDashboardSocket();

  useEffect(() => {
    historyAnchorRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  useEffect(() => {
    let cancelled = false;

    async function loadInitialMessages() {
      try {
        const response = await wsCommand<{ messages: ChatMessage[] }>("list_chat_messages", {
          chat_id: chatId,
          after_id: null,
          limit: 500,
        });
        if (cancelled) {
          return;
        }

        const merged = mergeMessages([], response.messages);
        setMessages(merged);
        setLatestMessageId(merged.length > 0 ? merged[merged.length - 1].id : null);
        setPendingResponses(0);
        setRequestError(null);
      } catch (error) {
        if (!cancelled) {
          setRequestError(error instanceof Error ? error.message : "Failed to load chat history.");
        }
      }
    }

    void loadInitialMessages();

    return () => {
      cancelled = true;
    };
  }, [chatId, wsCommand]);

  useEffect(() => {
    if (!socketConnected) {
      return;
    }

    let cancelled = false;
    const timer = window.setInterval(async () => {
      try {
        const response = await wsCommand<{ messages: ChatMessage[] }>("list_chat_messages", {
          chat_id: chatId,
          after_id: latestMessageId,
          limit: 200,
        });

        if (cancelled || response.messages.length === 0) {
          return;
        }

        setMessages((existing) => mergeMessages(existing, response.messages));
        setLatestMessageId((current) => {
          const incomingLatest = response.messages[response.messages.length - 1]?.id ?? current;
          if (current === null) {
            return incomingLatest;
          }
          return Math.max(current, incomingLatest ?? current);
        });

        const looperCount = response.messages.filter(
          (message) => message.role.toLowerCase() !== "me",
        ).length;
        if (looperCount > 0) {
          setPendingResponses((current) => Math.max(0, current - looperCount));
        }
      } catch {
        if (!cancelled) {
          setRequestError("Unable to refresh conversation updates.");
        }
      }
    }, 1200);

    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [chatId, latestMessageId, socketConnected, wsCommand]);

  async function sendMessage(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const text = draft.trim();
    if (text.length === 0) {
      return;
    }

    try {
      await wsCommand("enqueue_chat_message", { message: text, chat_id: chatId });
      setDraft("");
      setPendingResponses((current) => current + 1);
      setRequestError(null);
    } catch (error) {
      setRequestError(error instanceof Error ? error.message : "Failed to send message.");
    }
  }

  const statusText = useMemo(() => {
    if (!socketConnected) {
      return socketError ?? "Waiting for agent connection...";
    }
    if (pendingResponses > 0) {
      return "Waiting for Looper response...";
    }
    return "Ready";
  }, [pendingResponses, socketConnected, socketError]);

  return (
    <section className="rounded-xl border border-zinc-300 bg-white p-4 dark:border-zinc-700 dark:bg-zinc-950 sm:p-5">
      <div className="flex flex-wrap items-start justify-between gap-3 border-b border-zinc-200 pb-4 dark:border-zinc-800">
        <div>
          <h1 className="text-xl font-semibold">Chat</h1>
          <p className="mt-1 text-sm text-zinc-600 dark:text-zinc-300">Conversation ID: {chatId}</p>
        </div>
        <div className="flex items-center gap-2">
          <Link
            href="/conversations/chat-history"
            className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm hover:bg-zinc-200 dark:border-zinc-700 dark:bg-zinc-900 dark:hover:bg-zinc-800"
          >
            Chat History
          </Link>
          <Link
            href="/conversations/new-chat"
            className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium hover:bg-zinc-200 dark:border-zinc-700 dark:bg-zinc-900 dark:hover:bg-zinc-800"
          >
            New Chat
          </Link>
        </div>
      </div>

      <div className="mt-4 h-[55vh] overflow-y-auto rounded-lg border border-zinc-200 bg-zinc-50 p-3 dark:border-zinc-800 dark:bg-zinc-900">
        {messages.length === 0 ? (
          <p className="text-sm text-zinc-600 dark:text-zinc-300">
            This chat is not in history yet. Send a message to save it.
          </p>
        ) : (
          <ul className="space-y-3">
            {messages.map((message) => (
              <li
                key={message.id}
                className={`max-w-[90%] rounded-lg border p-3 text-sm sm:max-w-[80%] ${
                  message.role === "me"
                    ? "ml-auto border-zinc-300 bg-white dark:border-zinc-700 dark:bg-zinc-950"
                    : "mr-auto border-zinc-200 bg-zinc-100 dark:border-zinc-700 dark:bg-zinc-800"
                }`}
              >
                <p className="text-xs font-semibold uppercase tracking-wide text-zinc-500 dark:text-zinc-400">
                  {message.role === "me" ? "Me" : "Looper"}
                </p>
                <p className="mt-1 whitespace-pre-wrap break-words">{message.content}</p>
                <p className="mt-2 text-right text-xs text-zinc-500 dark:text-zinc-400">
                  {timestampLabel(message.created_at_unix_ms)}
                </p>
              </li>
            ))}
          </ul>
        )}
        <div ref={historyAnchorRef} />
      </div>

      <form onSubmit={(event) => void sendMessage(event)} className="mt-4 space-y-2">
        <label htmlFor="chat-input" className="sr-only">
          Send message
        </label>
        <textarea
          id="chat-input"
          rows={3}
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          placeholder="Type a message to Looper"
          className="w-full rounded-lg border border-zinc-300 bg-white px-3 py-2 text-sm outline-none ring-0 placeholder:text-zinc-500 focus:border-zinc-500 dark:border-zinc-700 dark:bg-zinc-950 dark:placeholder:text-zinc-400"
        />
        <div className="flex flex-wrap items-center justify-between gap-2">
          <p className="text-sm text-zinc-600 dark:text-zinc-300">{requestError ?? statusText}</p>
          <button
            type="submit"
            className="rounded-md border border-zinc-900 bg-zinc-900 px-4 py-2 text-sm font-medium text-white hover:bg-black disabled:cursor-not-allowed disabled:opacity-50 dark:border-zinc-100 dark:bg-zinc-100 dark:text-black dark:hover:bg-zinc-300"
            disabled={draft.trim().length === 0 || !socketConnected}
          >
            Send
          </button>
        </div>
      </form>
    </section>
  );
}
