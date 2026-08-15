use serde::{Deserialize, Serialize};

// ==================== 会话模式 ====================
/// chat=普通对话+图片生成 / agent=编程工具+联网搜索+图片生成
pub const MODE_CHAT: &str = "chat";
pub const MODE_AGENT: &str = "agent";

// ==================== 协议 / 模型类型 ====================
pub const PROTOCOL_OPENAI: &str = "openai";

pub const MODEL_TYPE_TEXT: &str = "text";
pub const MODEL_TYPE_VISION: &str = "vision";
pub const MODEL_TYPE_IMAGE: &str = "image";

// ==================== 上下文用量 ====================
/// 未配置模型时的默认上下文容量（token 数）
pub const CONTEXT_DEFAULT_TOKENS: u64 = 131_072;
/// 上下文自动压缩触发阈值（用量占比达到该值后尝试摘要早期对话）
pub const CONTEXT_COMPRESS_THRESHOLD: f64 = 0.6;
/// 压缩后至少保留的最近消息条数（用户 + 助手回复各一条）
pub const CONTEXT_KEEP_LAST_MSGS: usize = 2;
/// 压缩保留窗口的 token 预算占比：最近消息累计用量达到
/// 总容量 × 该比例后，更早的消息进入摘要。按 token 而非固定条数
/// 保留——Agent 单条消息可含 16 轮工具结果（上万 token），固定条数
/// 会导致"已压缩但仍超限"
pub const CONTEXT_KEEP_BUDGET_RATIO: f64 = 0.4;
/// 每张图片附件占用的估算 token 数
pub const CONTEXT_IMAGE_TOKENS: u64 = 1_000;
/// 请求固定开销估算（system prompt + 工具 JSON 定义）：
/// Agent 模式带全套工具定义约 4k，Chat 模式仅 system 约 1k。
/// 计入用量估算避免压缩触发偏晚
pub const CONTEXT_OVERHEAD_AGENT: u64 = 4_000;
pub const CONTEXT_OVERHEAD_CHAT: u64 = 1_000;

// ==================== 模型提供商 ====================

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ModelConfig {
    pub id: String,
    pub name: String,
    /// text | vision | image
    pub model_type: String,
    pub context_tokens: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    /// openai
    pub protocol: String,
    pub api_base: String,
    pub api_key: String,
    pub models: Vec<ModelConfig>,
}

impl ProviderConfig {
    pub fn find_model(&self, model_id: &str) -> Option<&ModelConfig> {
        self.models.iter().find(|m| m.id == model_id)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ModelSelection {
    pub provider_id: String,
    pub model_id: String,
}

// ==================== 附件 ====================

/// 用户上传的附件（保存于会话目录 uploads/）
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Attachment {
    pub name: String,
    pub mime: String,
    /// image | document
    pub kind: String,
    /// 会话目录内相对路径
    pub path: String,
    pub size: i64,
}

// ==================== 会话 ====================

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
    /// 上下文自动压缩后的早期对话摘要（空表示未压缩）
    #[serde(default)]
    pub summary: String,
    /// 已纳入摘要的最大消息 id（其之前的消息不再直接发送）
    #[serde(default)]
    pub summarized_until: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

fn default_mode() -> String {
    MODE_CHAT.to_string()
}

// ==================== 消息与产物 ====================

/// 工具生成的文件产物（图片/文件），随消息持久化并展示在聊天界面
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Artifact {
    /// image | file
    pub kind: String,
    pub name: String,
    /// 会话目录内相对路径
    pub path: String,
    pub size: i64,
    /// 生成说明/来源描述
    pub note: String,
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
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// 执行时间线：思考与各次工具调用按发生顺序记录，前端按时间线层级展示
    #[serde(default)]
    pub steps: Vec<ToolStep>,
    pub created_at: i64,
}

/// 执行时间线中的一个步骤（深度思考 / 工具调用），随消息持久化
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ToolStep {
    /// reasoning | search | tool | image
    pub kind: String,
    /// 展示摘要（如：联网搜索 "DeepSeek R1"、读取文件 main.rs、执行命令 npm test）
    pub label: String,
    /// 工具名（kind=reasoning 时为空）
    pub tool: String,
    /// 耗时（毫秒）
    pub duration_ms: u64,
    /// 仅 search 步骤携带：来源列表，点击步骤展开查看
    #[serde(default)]
    pub items: Vec<SearchItem>,
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

/// 完整的消息库行（含工具调用等列，仅后端使用）
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
    pub attachments: String,
    pub steps: String,
    pub created_at: i64,
}

// ==================== 设置 ====================

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

/// 硅基流动生成服务配置（遗留兼容，新配置统一走 providers 体系）
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SiliconFlowGenSettings {
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_sf_base_url")]
    pub base_url: String,
    #[serde(default = "default_sf_image_model")]
    pub image_model: String,
}

/// 阿里云百炼（DashScope）生成服务配置（遗留兼容）
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct AlibabaGenSettings {
    #[serde(default)]
    pub api_key: String,
    /// OpenAI 兼容模式地址
    #[serde(default = "default_ali_base_url")]
    pub base_url: String,
    #[serde(default = "default_ali_image_model")]
    pub image_model: String,
}

/// 生成服务设置（provider: siliconflow / alibaba）——遗留兼容字段
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct GenSettings {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub siliconflow: SiliconFlowGenSettings,
    #[serde(default)]
    pub alibaba: AlibabaGenSettings,
}

fn default_sf_base_url() -> String {
    "https://api.siliconflow.cn/v1".into()
}

fn default_sf_image_model() -> String {
    "Kwai-Kolors/Kolors".into()
}

fn default_ali_base_url() -> String {
    "https://dashscope.aliyuncs.com/compatible-mode/v1".into()
}

fn default_ali_image_model() -> String {
    "wanx-v1".into()
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
    /// 模型提供商列表（用户自定义名称与 API）
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// 对话模型 / 图片 的选择
    #[serde(default)]
    pub chat_model: Option<ModelSelection>,
    #[serde(default)]
    pub image_model: Option<ModelSelection>,
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
                },
                alibaba: AlibabaGenSettings {
                    api_key: String::new(),
                    base_url: default_ali_base_url(),
                    image_model: default_ali_image_model(),
                },
            },
            providers: Vec::new(),
            chat_model: None,
            image_model: None,
        }
    }
}

impl AppSettings {
    /// 通过选择引用查找提供商与模型
    pub fn find_model(&self, sel: &ModelSelection) -> Option<(&ProviderConfig, &ModelConfig)> {
        self.providers
            .iter()
            .find(|p| p.id == sel.provider_id)
            .and_then(|p| p.find_model(&sel.model_id).map(|m| (p, m)))
    }

    /// 对话模型：text / vision
    pub fn chat_models(&self) -> Vec<(&ProviderConfig, &ModelConfig)> {
        self.providers
            .iter()
            .flat_map(|p| {
                p.models
                    .iter()
                    .filter(|m| m.model_type == MODEL_TYPE_TEXT || m.model_type == MODEL_TYPE_VISION)
                    .map(move |m| (p, m))
            })
            .collect()
    }

    pub fn image_models(&self) -> Vec<(&ProviderConfig, &ModelConfig)> {
        self.providers
            .iter()
            .flat_map(|p| {
                p.models
                    .iter()
                    .filter(|m| m.model_type == MODEL_TYPE_IMAGE)
                    .map(move |m| (p, m))
            })
            .collect()
    }

    /// 解析当前对话使用的对话模型：
    /// 优先会话内选择的模型，其次使用设置中的对话模型选择
    pub fn resolve_chat_model(&self, conv: &Conversation) -> Option<(&ProviderConfig, &ModelConfig)> {
        if let Some(p) = self.providers.iter().find(|p| p.id == conv.provider) {
            if let Some(m) = p.find_model(&conv.model) {
                return Some((p, m));
            }
        }
        if let Some(sel) = &self.chat_model {
            if let Some(found) = self.find_model(sel) {
                return Some(found);
            }
        }
        self.chat_models().into_iter().next()
    }

    /// 当前对话模型的上下文容量（token 数），未配置时使用默认值
    pub fn chat_context_total(&self, conv: &Conversation) -> u64 {
        self.resolve_chat_model(conv)
            .map(|(_, m)| m.context_tokens)
            .filter(|t| *t > 0)
            .unwrap_or(CONTEXT_DEFAULT_TOKENS)
    }

    /// 解析图片生成模型
    pub fn resolve_image_model(&self) -> Option<(&ProviderConfig, &ModelConfig)> {
        self.image_model
            .as_ref()
            .and_then(|sel| self.find_model(sel))
            .or_else(|| self.image_models().into_iter().next())
    }
}

/// 遗留设置迁移：旧版 deepseek / gen 配置 -> 统一提供商体系
pub fn migrate_legacy_providers(s: &AppSettings) -> AppSettings {
    if !s.providers.is_empty() {
        return s.clone();
    }
    let mut out = s.clone();
    let mut providers: Vec<ProviderConfig> = Vec::new();
    let mut chat_sel: Option<ModelSelection> = None;
    let mut image_sel: Option<ModelSelection> = None;

    // DeepSeek 官方（OpenAI 兼容协议）
    if !s.deepseek.api_key.trim().is_empty() {
        let pid = "deepseek".to_string();
        let flash_id = "m_flash".to_string();
        let pro_id = "m_pro".to_string();
        providers.push(ProviderConfig {
            id: pid.clone(),
            name: "DeepSeek 官方".into(),
            protocol: PROTOCOL_OPENAI.into(),
            api_base: "https://api.deepseek.com/v1".into(),
            api_key: s.deepseek.api_key.clone(),
            models: vec![
                ModelConfig { id: flash_id.clone(), name: "deepseek-v4-flash".into(), model_type: MODEL_TYPE_TEXT.into(), context_tokens: 131_072 },
                ModelConfig { id: pro_id.clone(), name: "deepseek-v4-pro".into(), model_type: MODEL_TYPE_TEXT.into(), context_tokens: 131_072 },
            ],
        });
        chat_sel = Some(ModelSelection { provider_id: pid, model_id: flash_id });
    }
    // 硅基流动（OpenAI 协议）
    let sf = &s.gen.siliconflow;
    if !sf.api_key.trim().is_empty() {
        let pid = "siliconflow".to_string();
        let mut models = Vec::new();
        let img_id = "m_sf_img".to_string();
        if !sf.image_model.is_empty() {
            models.push(ModelConfig { id: img_id.clone(), name: sf.image_model.clone(), model_type: MODEL_TYPE_IMAGE.into(), context_tokens: 131_072 });
            image_sel = Some(ModelSelection { provider_id: pid.clone(), model_id: img_id });
        }
        providers.push(ProviderConfig {
            id: pid,
            name: "硅基流动".into(),
            protocol: PROTOCOL_OPENAI.into(),
            api_base: sf.base_url.clone(),
            api_key: sf.api_key.clone(),
            models,
        });
    }
    // 阿里云百炼（OpenAI 兼容协议）
    let ali = &s.gen.alibaba;
    if !ali.api_key.trim().is_empty() {
        let pid = "alibaba".to_string();
        let mut models = Vec::new();
        let img_id = "m_ali_img".to_string();
        if !ali.image_model.is_empty() {
            models.push(ModelConfig { id: img_id.clone(), name: ali.image_model.clone(), model_type: MODEL_TYPE_IMAGE.into(), context_tokens: 131_072 });
            image_sel = Some(ModelSelection { provider_id: pid.clone(), model_id: img_id });
        }
        providers.push(ProviderConfig {
            id: pid,
            name: "阿里云百炼".into(),
            protocol: PROTOCOL_OPENAI.into(),
            api_base: ali.base_url.clone(),
            api_key: ali.api_key.clone(),
            models,
        });
    }

    if providers.is_empty() {
        // 全新环境：预置 DeepSeek 提供商（无 Key，用户自行填写）
        providers.push(ProviderConfig {
            id: "deepseek".into(),
            name: "DeepSeek 官方".into(),
            protocol: PROTOCOL_OPENAI.into(),
            api_base: "https://api.deepseek.com/v1".into(),
            api_key: String::new(),
            models: vec![
                ModelConfig { id: "m_flash".into(), name: "deepseek-v4-flash".into(), model_type: MODEL_TYPE_TEXT.into(), context_tokens: 131_072 },
                ModelConfig { id: "m_pro".into(), name: "deepseek-v4-pro".into(), model_type: MODEL_TYPE_TEXT.into(), context_tokens: 131_072 },
            ],
        });
        chat_sel = Some(ModelSelection { provider_id: "deepseek".into(), model_id: "m_flash".into() });
    }

    out.providers = providers;
    out.chat_model = chat_sel;
    out.image_model = image_sel;
    out
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

// ==================== 其它 ====================

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
    pub compressed: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct WebPage {
    pub url: String,
    pub title: String,
    pub html: String,
}