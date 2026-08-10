export type Provider = "openai" | "anthropic";
export type Effort = "none" | "low" | "high" | "max";
export type ThemeMode = "auto" | "light" | "dark";
export type SearchStrategy = "auto" | "tavily" | "anysearch";

export type AgentMode = "chat" | "image" | "video" | "build" | "agent";

export interface Conversation {
  id: number;
  title: string;
  provider: Provider;
  model: string;
  web_search: boolean;
  deep_think: boolean;
  effort: Effort;
  mode: AgentMode;
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
  kind: "image" | "video" | "file";
  name: string;
  path: string;
  size: number;
  note: string;
}

export interface Message {
  id: number;
  conversation_id: number;
  role: "user" | "assistant";
  content: string;
  reasoning: string;
  search_results: SearchItem[];
  artifacts: Artifact[];
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
  video_model_i2v: string;
  video_model_t2v: string;
}

export interface GenSettings {
  provider: string;
  siliconflow: SiliconFlowGenSettings;
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
  searchProvider: string | null;
  error: string | null;
}

export interface ChatEventPayload {
  kind:
    | "status"
    | "reasoning_delta"
    | "content_delta"
    | "search_result"
    | "artifact"
    | "permission_request"
    | "video_done"
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
  family: "flash" | "pro";
}

export interface ContextStatus {
  used_tokens: number;
  total_tokens: number;
  percent: number;
  near_full: boolean;
  full: boolean;
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
  kind: "web" | "file" | "image" | "video";
  url: string;
  title: string;
  html: string;
}
