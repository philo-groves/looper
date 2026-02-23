import { randomUUID } from "node:crypto";
import { redirect } from "next/navigation";

export default function NewChatPage() {
  redirect(`/conversations/chat/${randomUUID()}`);
}
