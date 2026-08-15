export type Provider = "openai";
export type Effort = "none" | "low" | "high" | "max";
export type ThemeMode = "auto" | "light" | "dark";
export type SearchStrategy = "auto" | "tavily" | "anysearch";

export type AgentMode = "chat" | "agent";

export type ModelType = "text" | "vision" | "image";

export interface ModelConfig {
  id: string;
  name: string;
  model_type: ModelType;
  context_tokens: number;
}

export interface ProviderConfig {
  id: string;
  name: string;
  protocol: "openai";
  api_base: string;
  api_key: string;
  models: ModelConfig[];
}

export interface ModelSelection {
  provider_id: string;
  model_id: string;
}

export interface Attachment {
  name: string;
  mime: string;
  kind: "image" | "document";
  path: string;
  size: number;
}

export interface UploadAttachment {
  name: string;
  mime: string;
  data_url: string;
  kind: "image" | "document";
}

export interface Conversation {
  id: number;
  title: string;
  provider: string;
  model: string;
  web_search: boolean;
  deep_think: boolean;
  effort: Effort;
  mode: AgentMode;
  summary?: string;
  summarized_until?: number;
  created_at: number;
  updated_at: number;
}

export interface SearchItem {
  title: string;
  url: string;
  snippet: string;
  provider: "tavily" | "anysearch";
}

export interface Artifact {
  kind: "image" | "file";
  name: string;
  path: string;
  size: number;
  note: string;
}

/** 执行时间线步骤：深度思考 / 工具调用，按发生顺序展示 */
export interface ToolStep {
  kind: "reasoning" | "search" | "tool" | "image";
  label: string;
  tool: string;
  duration_ms: number;
  items: SearchItem[];
}

export interface Message {
  id: number;
  conversation_id: number;
  role: "user" | "assistant";
  content: string;
  reasoning: string;
  search_results: SearchItem[];
  artifacts: Artifact[];
  attachments: Attachment[];
  steps: ToolStep[];
  created_at: number;
}

export interface DeepSeekSettings {
  api_key: string;
}

export interface SearchSettings {
  tavily_key: string;
  tavily_enabled: boolean;
  anysearch_key: string;
  anysearch_enabled: boolean;
  strategy: SearchStrategy;
  max_results: number;
}

export interface SiliconFlowGenSettings {
  api_key: string;
  base_url: string;
  image_model: string;
}

export interface AlibabaGenSettings {
  api_key: string;
  base_url: string;
  image_model: string;
}

export interface GenSettings {
  provider: string;
  siliconflow: SiliconFlowGenSettings;
  alibaba: AlibabaGenSettings;
}

export interface AppSettings {
  theme: ThemeMode;
  default_web_search: boolean;
  default_deep_think: boolean;
  default_effort: Effort;
  default_model: string;
  default_mode: AgentMode;
  deepseek: DeepSeekSettings;
  search: SearchSettings;
  gen: GenSettings;
  providers: ProviderConfig[];
  chat_model: ModelSelection | null;
  image_model: ModelSelection | null;
}

export type ChatStatus =
  | "idle"
  | "thinking"
  | "searching"
  | "analyzing"
  | "answering"
  | "generating";

export interface ChatDraft {
  status: ChatStatus;
  reasoning: string;
  content: string;
  searchItems: SearchItem[];
  artifacts: Artifact[];
  steps: ToolStep[];
  searchProvider: string | null;
  error: string | null;
}

export interface ChatEventPayload {
  kind:
    | "status"
    | "reasoning_delta"
    | "content_delta"
    | "search_result"
    | "tool_step"
    | "artifact"
    | "permission_request"
    | "stopped"
    | "done"
    | "error";
  conversation_id: number;
  text?: string;
  item?: SearchItem | Artifact;
  message?: string;
  tool?: string;
  path?: string;
  search_provider?: "tavily" | "anysearch" | null;
}

export interface PermissionRequest {
  conversation_id: number;
  tool: string;
  path: string;
}

export interface ModelOption {
  label: string;
  model: string;
  modelType: ModelType;
  protocol: Provider;
}

export interface ContextStatus {
  used_tokens: number;
  total_tokens: number;
  percent: number;
  near_full: boolean;
  full: boolean;
  compressed: boolean;
}

export interface WebPage {
  url: string;
  title: string;
  html: string;
}

export interface EditTarget {
  id: number;
  text: string;
}

export interface PreviewContent {
  kind: "web" | "file" | "image";
  url: string;
  title: string;
  html: string;
}
