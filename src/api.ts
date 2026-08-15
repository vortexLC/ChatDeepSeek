import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AgentMode,
  AppSettings,
  ChatEventPayload,
  ContextStatus,
  Conversation,
  Effort,
  Message,
  ModelConfig,
  ProviderConfig,
  UploadAttachment,
  WebPage,
} from "./types";

export interface ConversationUpdate {
  title?: string;
  provider?: string;
  model?: string;
  web_search?: boolean;
  deep_think?: boolean;
  effort?: Effort;
  mode?: AgentMode;
}

export async function getInitialState(): Promise<{
  conversations: Conversation[];
  settings: AppSettings;
}> {
  return invoke("get_initial_state");
}

export async function listConversations(): Promise<Conversation[]> {
  return invoke("list_conversations");
}

export async function createConversation(): Promise<Conversation> {
  return invoke("create_conversation");
}

export async function updateConversation(
  id: number,
  patch: ConversationUpdate
): Promise<void> {
  return invoke("update_conversation", { id, patch });
}

export async function deleteConversation(id: number): Promise<void> {
  return invoke("delete_conversation", { id });
}

export async function getMessages(id: number): Promise<Message[]> {
  return invoke("get_messages", { id });
}

export async function getContextStatus(id: number): Promise<ContextStatus> {
  return invoke("get_context_status", { conversationId: id });
}

export async function getSettings(): Promise<AppSettings> {
  return invoke("get_settings");
}

export async function saveSettings(settings: AppSettings): Promise<void> {
  return invoke("save_settings", { settings });
}

export async function clearAllConversations(): Promise<void> {
  return invoke("clear_all_conversations");
}

export async function sendMessage(
  conversationId: number,
  content: string,
  attachments: UploadAttachment[] = []
): Promise<void> {
  return invoke("send_message", { conversationId, content, attachments });
}

export async function editAndResend(
  conversationId: number,
  messageId: number,
  content: string,
  attachments: UploadAttachment[] = []
): Promise<void> {
  return invoke("edit_and_resend", {
    conversationId,
    messageId,
    content,
    attachments,
  });
}

export async function fetchWebPage(url: string): Promise<WebPage> {
  return invoke("fetch_webpage", { url });
}

export async function fetchFileContent(
  conversationId: number,
  path: string
): Promise<WebPage> {
  return invoke("fetch_file_content", { conversationId, path });
}

export async function getArtifactAbsPath(
  conversationId: number,
  path: string
): Promise<string> {
  return invoke("get_artifact_abs_path", { conversationId, path });
}

export async function respondPermission(
  conversationId: number,
  approve: boolean
): Promise<void> {
  return invoke("respond_permission", { conversationId, approve });
}

export async function stopGeneration(conversationId: number): Promise<void> {
  return invoke("stop_generation", { conversationId });
}

export async function testModel(
  provider: ProviderConfig,
  model: ModelConfig
): Promise<string> {
  return invoke("test_model", { provider, model });
}

export function onChatEvent(
  handler: (payload: ChatEventPayload) => void
): Promise<() => void> {
  return listen<ChatEventPayload>("chat_event", (event) => handler(event.payload));
}
