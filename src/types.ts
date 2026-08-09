export type Provider = "openai" | "anthropic";
export type Effort = "none" | "low" | "high" | "max";
export type ThemeMode = "auto" | "light" | "dark";
export type SearchStrategy = "auto" | "tavily" | "anysearch";

export interface Conversation {
  id: number;
  title: string;
  provider: Provider;
  model: string;
  web_search: boolean;
  deep_think: boolean;
  effort: Effort;
  created_at: number;
  updated_at: number;
}

export interface SearchItem {
  title: string;
  url: string;
  snippet: string;
  provider: "tavily" | "anysearch";
}

export interface Message {
  id: number;
  conversation_id: number;
  role: "user" | "assistant";
  content: string;
  reasoning: string;
  search_results: SearchItem[];
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

export interface AppSettings {
  theme: ThemeMode;
  default_web_search: boolean;
  default_deep_think: boolean;
  default_effort: Effort;
  default_model: string;
  deepseek: DeepSeekSettings;
  search: SearchSettings;
}

export type ChatStatus = "idle" | "thinking" | "searching" | "analyzing" | "answering";

export interface ChatDraft {
  status: ChatStatus;
  reasoning: string;
  content: string;
  searchItems: SearchItem[];
  searchProvider: string | null;
  error: string | null;
}

export interface ChatEventPayload {
  kind:
    | "status"
    | "reasoning_delta"
    | "content_delta"
    | "search_result"
    | "done"
    | "error";
  conversation_id: number;
  text?: string;
  item?: SearchItem;
  message?: string;
  search_provider?: "tavily" | "anysearch" | null;
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
