use serde::{Deserialize, Serialize};

/// 会话模式：决定模型可用的工具集合
/// chat=普通对话 / image=Chat+图片生成 / video=Chat+视频生成
/// build=编程工具(沙箱, 无生成) / agent=全部工具
pub const MODE_CHAT: &str = "chat";
pub const MODE_IMAGE: &str = "image";
pub const MODE_VIDEO: &str = "video";
pub const MODE_BUILD: &str = "build";
pub const MODE_AGENT: &str = "agent";

#[derive(Serialize, Deserialize, Clone)]
pub struct Conversation {
    pub id: i64,
    pub title: String,
    pub provider: String,
    pub model: String,
    pub web_search: bool,
    pub deep_think: bool,
    pub effort: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    pub created_at: i64,
    pub updated_at: i64,
}

fn default_mode() -> String {
    MODE_CHAT.to_string()
}

/// 工具生成的文件产物（图片/视频/文件），随消息持久化并展示在聊天界面
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Artifact {
    pub kind: String, // image | video | file
    pub name: String,
    pub path: String, // 会话目录内相对路径
    pub size: i64,
    pub note: String, // 生成说明/来源描述
}

#[derive(Serialize, Clone)]
pub struct Message {
    pub id: i64,
    pub conversation_id: i64,
    pub role: String,
    pub content: String,
    pub reasoning: String,
    pub search_results: Vec<SearchItem>,
    pub artifacts: Vec<Artifact>,
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
    pub artifacts: String,
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

/// 硅基流动生成服务配置（模块化：后续可扩展其它提供商）
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SiliconFlowGenSettings {
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_sf_base_url")]
    pub base_url: String,
    #[serde(default = "default_sf_image_model")]
    pub image_model: String,
    #[serde(default = "default_sf_video_i2v")]
    pub video_model_i2v: String,
    #[serde(default = "default_sf_video_t2v")]
    pub video_model_t2v: String,
}

/// 生成服务设置（模块化：provider 目前仅 siliconflow，后续可扩展）
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct GenSettings {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub siliconflow: SiliconFlowGenSettings,
}

fn default_sf_base_url() -> String {
    "https://api.siliconflow.cn/v1".into()
}

fn default_sf_image_model() -> String {
    "Kwai-Kolors/Kolors".into()
}

fn default_sf_video_i2v() -> String {
    "Wan-AI/Wan2.2-I2V-A14B".into()
}

fn default_sf_video_t2v() -> String {
    "Wan-AI/Wan2.2-T2V-A14B".into()
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
    pub default_mode: String,
    #[serde(default)]
    pub deepseek: DeepSeekSettings,
    #[serde(default)]
    pub search: SearchSettings,
    #[serde(default)]
    pub gen: GenSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: "auto".into(),
            default_web_search: false,
            default_deep_think: false,
            default_effort: default_effort(),
            default_model: default_flash_model(),
            default_mode: default_mode(),
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
            gen: GenSettings {
                provider: "siliconflow".into(),
                siliconflow: SiliconFlowGenSettings {
                    api_key: String::new(),
                    base_url: default_sf_base_url(),
                    image_model: default_sf_image_model(),
                    video_model_i2v: default_sf_video_i2v(),
                    video_model_t2v: default_sf_video_t2v(),
                },
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
    pub mode: Option<String>,
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
