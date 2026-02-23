export type ConversationRole = "me" | "looper";

export type ChatMessage = {
  id: string;
  role: ConversationRole;
  text: string;
  created_at_unix_ms: number;
};

export type ChatConversation = {
  id: string;
  title: string;
  created_at_unix_ms: number;
  updated_at_unix_ms: number;
  messages: ChatMessage[];
};

type PersistedConversations = {
  conversations: ChatConversation[];
};

type NewChatMessage = {
  role: ConversationRole;
  text: string;
};

const CONVERSATION_STORAGE_KEY = "looper-chat-conversations-v1";
const CONVERSATION_STORAGE_EVENT = "looper-chat-conversations-updated";
const EMPTY_CONVERSATIONS: ChatConversation[] = [];

let cachedRawConversations: string | null | undefined;
let cachedSnapshot: ChatConversation[] = EMPTY_CONVERSATIONS;

function conversationTitleFromMessage(text: string) {
  const cleaned = text.trim().replace(/\s+/g, " ");
  if (cleaned.length === 0) {
    return "New Chat";
  }
  return cleaned.slice(0, 48);
}

function messageId() {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `msg-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

function readStorage(): PersistedConversations {
  if (typeof window === "undefined") {
    return { conversations: [] };
  }

  const raw = window.localStorage.getItem(CONVERSATION_STORAGE_KEY);
  if (!raw) {
    return { conversations: [] };
  }

  try {
    const parsed = JSON.parse(raw) as PersistedConversations;
    if (!parsed || !Array.isArray(parsed.conversations)) {
      return { conversations: [] };
    }
    return {
      conversations: parsed.conversations.filter(
        (conversation) => conversation && typeof conversation.id === "string",
      ),
    };
  } catch {
    return { conversations: [] };
  }
}

function normalizedConversations(payload: PersistedConversations): ChatConversation[] {
  return payload.conversations
    .filter((conversation) => conversation.messages.length > 0)
    .sort((left, right) => right.updated_at_unix_ms - left.updated_at_unix_ms);
}

function snapshotFromRaw(raw: string | null): ChatConversation[] {
  if (!raw) {
    return EMPTY_CONVERSATIONS;
  }

  try {
    const parsed = JSON.parse(raw) as PersistedConversations;
    if (!parsed || !Array.isArray(parsed.conversations)) {
      return EMPTY_CONVERSATIONS;
    }

    const normalized = normalizedConversations({
      conversations: parsed.conversations.filter(
        (conversation) => conversation && typeof conversation.id === "string",
      ),
    });

    return normalized.length > 0 ? normalized : EMPTY_CONVERSATIONS;
  } catch {
    return EMPTY_CONVERSATIONS;
  }
}

function writeStorage(payload: PersistedConversations) {
  if (typeof window === "undefined") {
    return;
  }
  const raw = JSON.stringify(payload);
  window.localStorage.setItem(CONVERSATION_STORAGE_KEY, raw);
  cachedRawConversations = raw;
  cachedSnapshot = snapshotFromRaw(raw);
  window.dispatchEvent(new Event(CONVERSATION_STORAGE_EVENT));
}

export function subscribeConversations(onStoreChange: () => void): () => void {
  if (typeof window === "undefined") {
    return () => {};
  }

  const handleStorage = (event: StorageEvent) => {
    if (event.key === null || event.key === CONVERSATION_STORAGE_KEY) {
      onStoreChange();
    }
  };

  window.addEventListener("storage", handleStorage);
  window.addEventListener(CONVERSATION_STORAGE_EVENT, onStoreChange);

  return () => {
    window.removeEventListener("storage", handleStorage);
    window.removeEventListener(CONVERSATION_STORAGE_EVENT, onStoreChange);
  };
}

export function getConversationsSnapshot(): ChatConversation[] {
  if (typeof window === "undefined") {
    return EMPTY_CONVERSATIONS;
  }

  const raw = window.localStorage.getItem(CONVERSATION_STORAGE_KEY);
  if (cachedRawConversations === raw && cachedRawConversations !== undefined) {
    return cachedSnapshot;
  }

  cachedRawConversations = raw;
  cachedSnapshot = snapshotFromRaw(raw);
  return cachedSnapshot;
}

export function getConversationsServerSnapshot(): ChatConversation[] {
  return EMPTY_CONVERSATIONS;
}

export function listConversations(): ChatConversation[] {
  return normalizedConversations(readStorage());
}

export function getConversation(chatId: string): ChatConversation | null {
  const payload = readStorage();
  return payload.conversations.find((conversation) => conversation.id === chatId) ?? null;
}

export function appendConversationMessages(
  chatId: string,
  messages: NewChatMessage[],
): ChatConversation | null {
  const normalized = messages
    .map((message) => ({
      role: message.role,
      text: message.text.trim(),
    }))
    .filter((message) => message.text.length > 0);

  if (normalized.length === 0) {
    return null;
  }

  const payload = readStorage();
  const now = Date.now();
  const firstFromMe = normalized.find((message) => message.role === "me");

  const messageRecords: ChatMessage[] = normalized.map((message) => ({
    id: messageId(),
    role: message.role,
    text: message.text,
    created_at_unix_ms: Date.now(),
  }));

  const existingIndex = payload.conversations.findIndex((conversation) => conversation.id === chatId);

  if (existingIndex < 0) {
    const nextConversation: ChatConversation = {
      id: chatId,
      title: firstFromMe ? conversationTitleFromMessage(firstFromMe.text) : "New Chat",
      created_at_unix_ms: now,
      updated_at_unix_ms: now,
      messages: messageRecords,
    };

    payload.conversations.push(nextConversation);
    writeStorage(payload);
    return nextConversation;
  }

  const existing = payload.conversations[existingIndex];
  const shouldRename = existing.title.trim().length === 0 || existing.title === "New Chat";
  const nextTitle =
    shouldRename && firstFromMe
      ? conversationTitleFromMessage(firstFromMe.text)
      : existing.title || "New Chat";

  const updated: ChatConversation = {
    ...existing,
    title: nextTitle,
    updated_at_unix_ms: now,
    messages: [...existing.messages, ...messageRecords],
  };

  payload.conversations[existingIndex] = updated;
  writeStorage(payload);
  return updated;
}
