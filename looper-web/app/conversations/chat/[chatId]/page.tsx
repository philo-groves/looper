import { ChatClient } from "@/components/conversations/ChatClient";

type ChatPageProps = {
  params: Promise<{ chatId: string }>;
};

export default async function ChatPage({ params }: ChatPageProps) {
  const resolved = await params;
  return <ChatClient chatId={resolved.chatId} />;
}
