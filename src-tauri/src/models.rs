use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct Conversation {
    pub id: i64,
    pub title: String,
    pub provider: String,
    pub model: String,
    pub web_search: bool,
    pub deep_think: bool,
    pub effort: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Serialize, Clone)]
pub struct Message {
    pub id: i64,
    pub conversation_id: i64,
    pub role: String,
    pub content: String,
    pub reasoning: String,
    pub search_results: Vec<SearchItem>,
    pub created_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SearchItem {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub provider: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ToolResult {
    pub tool_call_id: String,
    pub content: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DbMessageRow {
    pub id: i64,
    pub conversation_id: i64,
    pub role: String,
    pub content: String,
    pub reasoning: String,
    pub tool_calls: String,
    pub tool_results: String,
    pub search_results: String,
    pub created_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DeepSeekSettings {
    #[serde(default)]
    pub api_key: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SearchSettings {
    #[serde(default)]
    pub tavily_key: String,
    #[serde(default = "default_true")]
    pub tavily_enabled: bool,
    #[serde(default)]
    pub anysearch_key: String,
    #[serde(default = "default_true")]
    pub anysearch_enabled: bool,
    #[serde(default)]
    pub strategy: String,
    #[serde(default = "default_max_results")]
    pub max_results: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppSettings {
    #[serde(default)]
    pub theme: String,
    #[serde(default)]
    pub default_web_search: bool,
    #[serde(default)]
    pub default_deep_think: bool,
    #[serde(default = "default_effort")]
    pub default_effort: String,
    #[serde(default = "default_flash_model")]
    pub default_model: String,
    #[serde(default)]
    pub deepseek: DeepSeekSettings,
    #[serde(default)]
    pub search: SearchSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: "auto".into(),
            default_web_search: false,
            default_deep_think: false,
            default_effort: default_effort(),
            default_model: default_flash_model(),
            deepseek: DeepSeekSettings {
                api_key: String::new(),
            },
            search: SearchSettings {
                tavily_key: String::new(),
                tavily_enabled: true,
                anysearch_key: String::new(),
                anysearch_enabled: true,
                strategy: "auto".into(),
                max_results: 5,
            },
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_effort() -> String {
    "high".into()
}

fn default_flash_model() -> String {
    "deepseek-v4-flash".into()
}

fn default_max_results() -> i64 {
    5
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct ConversationPatch {
    pub title: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub web_search: Option<bool>,
    pub deep_think: Option<bool>,
    pub effort: Option<String>,
}

#[derive(Serialize)]
pub struct InitialState {
    pub conversations: Vec<Conversation>,
    pub settings: AppSettings,
}

#[derive(Serialize, Clone, Debug)]
pub struct ContextUsage {
    pub used_tokens: u64,
    pub total_tokens: u64,
    pub percent: f64,
    pub near_full: bool,
    pub full: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct WebPage {
    pub url: String,
    pub title: String,
    pub html: String,
}
