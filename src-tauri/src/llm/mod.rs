pub mod anthropic;
pub mod openai;
pub mod search;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Notify;

use tauri::{AppHandle, Emitter};

use crate::agent::tools;
use crate::commands::{AppState, emit};
use crate::models::*;

pub const MAX_TOOL_ROUNDS: usize = 8;

#[derive(Clone)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
        self.notify.notify_one();
    }

    pub async fn wait(&self) {
        while !self.is_cancelled() {
            self.notify.notified().await;
        }
    }
}

/// 图片块（base64），随用户消息发送给多模态模型
#[derive(Clone, Debug)]
pub struct ImageBlock {
    pub media_type: String,
    pub base64: String,
}

#[derive(Clone, Debug)]
pub enum OutMsg {
    User {
        content: String,
        images: Vec<ImageBlock>,
        /// 已提取的文档文本（上传的 txt/pdf 等）
        docs: String,
    },
    Assistant {
        content: String,
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

pub struct TurnResult {
    pub reasoning: String,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    /// 流式中途被用户停止：content/reasoning 为部分生成结果，需保留并持久化
    pub stopped: bool,
    /// 流式中断（网络错误等，非用户操作）：content/reasoning 为已生成的部分内容，
    /// 由上层先持久化再报告错误，避免已流式输出的内容随错误一起丢失
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Accum {
    pub reasoning: String,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_results: Vec<ToolResult>,
    pub search_items: Vec<SearchItem>,
    pub artifacts: Vec<Artifact>,
}

impl Accum {
    fn is_empty(&self) -> bool {
        self.reasoning.is_empty()
            && self.content.is_empty()
            && self.search_items.is_empty()
            && self.tool_calls.is_empty()
            && self.artifacts.is_empty()
    }
}

const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_DOC_CHARS: usize = 20000;
/// 单轮发送给模型的图片上限（超出时只保留最近消息中的图片）
const MAX_IMAGES_PER_TURN: usize = 6;

fn mime_from_name(name: &str) -> String {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "image/jpeg",
    }
    .to_string()
}

/// 提取文档文本：文本类直接读取；PDF 通过 lopdf 提取
fn extract_document_text(path: &std::path::Path) -> Option<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => {
            let doc = lopdf::Document::load(path).ok()?;
            let mut text = String::new();
            for (page_no, _) in doc.get_pages() {
                if let Ok(t) = doc.extract_text(&[page_no]) {
                    text.push_str(&t);
                }
            }
            Some(text)
        }
        _ => std::fs::read_to_string(path).ok(),
    }
}

/// 当前日期（中国时区，YYYY-MM-DD），用于系统提示词
fn today_cn() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = (secs + 8 * 3600) / 86400;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y}-{m:02}-{d:02}")
}

/// 构建系统提示词（Anthropic / OpenAI 协议共用）。
/// 按会话模式附加工具使用说明；生成类模式明确要求模型调用生成工具、禁止以
/// "文本模型无法生成"为由拒绝，也不要让用户复制提示词去外部工具生成。
pub(crate) fn build_system_prompt(conv: &Conversation) -> String {
    let date = today_cn();
    let mode_hint = match conv.mode.as_str() {
        MODE_BUILD => "\n\n【当前模式：Build】你可以使用编程工具（read_file / write_file / edit_file / delete_file / list_files / glob / grep / bash）在本会话隔离目录中创建和修改文件、执行命令，完成用户的开发任务。所有文件仅保存在本会话目录内，无法访问会话目录之外的文件。",
        MODE_AGENT => "\n\n【当前模式：Agent】你拥有以下全部工具能力：\n\
1. 编程工具（read_file / write_file / edit_file / delete_file / list_files / glob / grep / bash）：在本会话隔离目录中读写文件、执行命令，完成开发任务；\n\
2. generate_image：生成图片。当用户要求生成、绘制、设计图片/图像/插画/海报/壁纸等任何视觉内容时，必须调用此工具——系统会调用专门的绘图模型完成生成并自动保存；\n\
3. generate_video：生成视频。当用户要求生成视频/动画/短片时，必须调用此工具（耗时约几分钟）。\n\
【重要】\n\
- 你确实具备图片与视频生成能力（由上述工具调用专门模型实现）。严禁回复「我无法生成图片/视频」「作为文本/语言模型我不能…」「你可以把提示词复制到 Midjourney / Stable Diffusion / Runway / 可灵等工具中」之类的话术。\n\
- 需要生成时，立即实际发起对应工具调用。严禁只在回复正文里描述或罗列「调用 generate_image：prompt: …」这类文字而不真正调用工具；也严禁只说「正在为您生成」却不发起调用。调用工具前不要输出冗长的计划、分镜脚本或提示词设计。\n\
- 用户要求「先生成图片、再基于该图生成视频」时：先调用 generate_image，等拿到图片结果（含 images/xxx.png 路径）后，再调用 generate_video（mode=image2video，image 传刚生成的图片路径 images/xxx.png）。",
        MODE_IMAGE => "\n\n【当前模式：Image】你具备图片生成能力：当用户要求生成、绘制、设计任何图片/图像/插画/海报/壁纸等视觉内容时，必须调用 generate_image 工具——系统会调用专门的绘图模型完成生成并自动保存。\n\
【重要】严禁回复「我无法生成图片」「作为文本模型我不能画图」「请去 Midjourney / Stable Diffusion 等外部工具」等话术，也不要只输出提示词。需要生成时立即实际发起 generate_image 工具调用，不要在正文里描述「调用 generate_image：…」而不真正调用。",
        MODE_VIDEO => "\n\n【当前模式：Video】你具备视频生成能力：当用户要求生成视频/动画/短片时，必须调用 generate_video 工具，并按需选择 mode：text2video 文生视频（无需图片）；image2video 图生视频（图片作首帧）；reference2video 参考图生视频（参考图片风格/主体，需 r2v 模型）。image/images 可传图片 URL、base64 或本会话内图片路径（如 images/xxx.png，即 generate_image 的产物）；图生/参考模式下若用户已上传图片也可不传 image，系统自动使用最近上传的图片。生成需几分钟，请告知用户耐心等待。\n\
【重要】严禁回复「我无法生成视频」等话术，也不要只给提示词。需要生成时立即实际发起 generate_video 工具调用，不要在正文里描述「调用 generate_video：…」而不真正调用。",
        _ => "",
    };
    let base = if conv.web_search {
        format!(
            "你是 ChatDeepSeek 智能助手。今天是 {date}。\n\
\n\
请严格遵循以下工作流程处理用户的每个问题：\n\
1. 【思考】深入理解用户问题：拆解意图、提取关键信息、判断需要哪些实时信息或专业领域数据；\n\
2. 【执行】当问题涉及实时信息、专业垂直领域内容，或你无法确定的事实，主动调用 web_search 工具搜索（可针对不同关键词多次搜索；必要时通过 provider 参数指定 tavily 或 anysearch 引擎）。注意：只有真正调用 web_search 工具时，才可向用户说明「正在搜索」；在发出工具调用之前，不要向用户承诺「我去搜索」「让我查一下」之类的话术，直接调用工具即可；\n\
3. 【分析】综合分析搜索结果与用户问题：交叉验证、去伪存真，提炼与问题直接相关的核心事实与数据；\n\
4. 【总结】以「总-分-总」结构回答用户：\n\
   - 总：先给出直接明确的结论或概要，让用户第一时间获得答案；\n\
   - 分：再分点展开，说明依据、推理过程与关键数据，引用搜索结果时附上来源链接 [来源](url)；\n\
   - 总：最后总结要点，并适当补充注意事项或建议。\n\
\n\
回答请使用规范的 Markdown 格式（标题、列表、表格、加粗等），保持简洁、准确、条理清晰，不要使用 emoji 过度装饰。"
        )
    } else {
        format!(
            "你是 ChatDeepSeek 智能助手。今天是 {date}。\n\
\n\
回答用户问题前请先思考：拆解问题意图、整理回答思路，再组织答案。\n\
回答采用「总-分-总」结构：先给出结论概要，再分点展开说明依据与细节，最后总结要点。\n\
请使用规范的 Markdown 格式（标题、列表、表格、加粗等），保持简洁、准确、条理清晰，不要使用 emoji 过度装饰。\n\
\n\
【重要：联网搜索未开启】\n\
当前没有启用联网搜索功能，你无法访问互联网，也无法获取实时信息。\n\
- 绝对不要说自己「正在搜索」「为你搜索」「稍后查询」「等我查一下」等话术，也不要声称自己具备联网能力；\n\
- 如果用户询问实时新闻、最新事件、实时行情、今日热点等需要联网才能获取的信息，请如实告知：「当前未开启联网搜索，我无法获取实时信息」，然后基于自身已有知识给出一般性说明、背景分析或建议，并提醒用户可开启消息框中的「联网搜索」后再询问。"
        )
    };
    format!("{base}{mode_hint}")
}

fn build_outgoing(
    state: &AppState,
    conv: &Conversation,
    include_tools: bool,
) -> Vec<OutMsg> {
    let mut out = Vec::new();
    // 已自动压缩的早期对话：以摘要代替原始消息
    if !conv.summary.is_empty() {
        out.push(OutMsg::User {
            content: format!("【以下为本次对话早期内容的摘要，请将其作为背景信息】\n{}", conv.summary),
            images: Vec::new(),
            docs: String::new(),
        });
    }
    let rows = match state.db.list_messages_full(conv.id) {
        Ok(r) => r,
        Err(_) => return out,
    };
    // 图片数量上限：超出时只保留最近消息中的图片
    // （先收集全部图片附件，再截取末尾 MAX_IMAGES_PER_TURN 个，保证最新图片优先）
    let mut img_keep: Vec<(i64, String)> = Vec::new();
    for row in &rows {
        if row.role != "user" || row.id <= conv.summarized_until {
            continue;
        }
        let attachments: Vec<Attachment> =
            serde_json::from_str(&row.attachments).unwrap_or_default();
        for att in attachments.iter().filter(|a| a.kind == "image") {
            img_keep.push((row.id, att.path.clone()));
        }
    }
    if img_keep.len() > MAX_IMAGES_PER_TURN {
        img_keep = img_keep.split_off(img_keep.len() - MAX_IMAGES_PER_TURN);
    }
    let img_keep: std::collections::HashSet<(i64, String)> = img_keep.into_iter().collect();

    for row in rows {
        if row.id <= conv.summarized_until {
            continue;
        }
        match row.role.as_str() {
            "user" => {
                let attachments: Vec<Attachment> =
                    serde_json::from_str(&row.attachments).unwrap_or_default();
                let mut images: Vec<ImageBlock> = Vec::new();
                let mut docs = String::new();
                for att in attachments {
                    if att.kind == "image" {
                        // 超出配额（非最近）的图片直接跳过，不再读取文件
                        if !img_keep.contains(&(row.id, att.path.clone())) {
                            continue;
                        }
                    }
                    let Some(p) = state.db.session_abs_path(conv.id, &att.path) else {
                        continue;
                    };
                    if att.kind == "image" {
                        let Ok(bytes) = std::fs::read(&p) else { continue };
                        if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
                            continue;
                        }
                        use base64::Engine;
                        let media_type = if att.mime.starts_with("image/") {
                            att.mime.clone()
                        } else {
                            mime_from_name(&att.name)
                        };
                        images.push(ImageBlock {
                            media_type,
                            base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
                        });
                    } else if let Some(text) = extract_document_text(&p) {
                        if !text.trim().is_empty() {
                            let trimmed: String = text.chars().take(MAX_DOC_CHARS).collect();
                            docs.push_str(&format!(
                                "【用户上传文件：{}】\n{}\n",
                                att.name, trimmed
                            ));
                        }
                    }
                }
                out.push(OutMsg::User {
                    content: row.content.clone(),
                    images,
                    docs,
                });
            }
            "assistant" => {
                let tool_calls_parsed: Vec<ToolCall> =
                    serde_json::from_str(&row.tool_calls).unwrap_or_default();
                let tool_results: Vec<ToolResult> =
                    serde_json::from_str(&row.tool_results).unwrap_or_default();
                // 关闭联网搜索时，剥离历史中的工具调用痕迹，防止模型误以为仍可搜索
                let tool_calls = if include_tools {
                    tool_calls_parsed.clone()
                } else {
                    Vec::new()
                };
                // 空内容且无工具调用的 assistant 消息（如旧版本停止生成时残留的
                // 部分思考记录）：重放给 OpenAI 兼容接口会因缺少 content 字段报 400，
                // 直接跳过（不产生孤儿 tool_result——本分支其余代码一并跳过）
                if row.content.trim().is_empty() && tool_calls.is_empty() {
                    continue;
                }
                out.push(OutMsg::Assistant {
                    content: row.content.clone(),
                    tool_calls,
                });
                if include_tools {
                    // 仅回传与 tool_use 配对的结果，防止历史数据中出现孤儿 tool_result 导致 API 400
                    let ids: std::collections::HashSet<&str> =
                        tool_calls_parsed.iter().map(|c| c.id.as_str()).collect();
                    for tr in tool_results {
                        if ids.contains(tr.tool_call_id.as_str()) {
                            out.push(OutMsg::Tool {
                                tool_call_id: tr.tool_call_id,
                                content: tr.content,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

pub async fn run_agent(
    app: AppHandle,
    state: Arc<AppState>,
    conv: Conversation,
    settings: AppSettings,
    token: Arc<CancelToken>,
) {
    emit(&app, conv.id, "status", Some("thinking"));
    let mut acc = Accum::default();
    let result = run_agent_inner(&app, &state, &state, &conv, &settings, &token, &mut acc).await;
    match result {
        Ok(()) => {
            if let Err(e) = persist(&state, &conv, &acc) {
                // 回复已生成但保存失败：明确告知用户，避免"内容消失"而不知原因
                eprintln!("[persist] 会话 {} 回复保存失败: {e}", conv.id);
                emit(&app, conv.id, "error", Some(&e));
                return;
            }
            emit(&app, conv.id, "done", None);
        }
        Err(msg) => {
            if !acc.is_empty() {
                if let Err(e) = persist(&state, &conv, &acc) {
                    eprintln!("[persist] 会话 {} 部分回复保存失败: {e}", conv.id);
                    emit(&app, conv.id, "error", Some(&e));
                    return;
                }
            }
            // 用户主动停止不是错误：以独立事件静默收尾，避免弹出红色错误横幅
            if msg == "已停止生成" {
                emit(&app, conv.id, "stopped", None);
            } else {
                emit(&app, conv.id, "error", Some(&msg));
            }
        }
    }
}

fn persist(state: &AppState, conv: &Conversation, acc: &Accum) -> Result<(), String> {
    // 会话已被删除时不落库：防止后台任务把已删除会话的目录/消息库"复活"
    if state.db.get_conversation(conv.id).is_none() {
        return Ok(());
    }
    // 停止生成时即使只有思考内容（正文为空）也照常落库：
    // 用户停止后界面依赖该消息展示已流式输出的思考过程，不落库会导致内容"消失"。
    // 空 content 消息不会发往 API——build_outgoing / try_compress_history 已将其过滤
    let tool_calls_json = serde_json::to_string(&acc.tool_calls).unwrap_or_else(|_| "[]".into());
    let tool_results_json =
        serde_json::to_string(&acc.tool_results).unwrap_or_else(|_| "[]".into());
    state.db.insert_message(
        conv.id,
        "assistant",
        &acc.content,
        &acc.reasoning,
        &tool_calls_json,
        &tool_results_json,
        &acc.search_items,
        &acc.artifacts,
    )?;
    state.db.touch(conv.id);
    Ok(())
}

async fn run_agent_inner(
    app: &AppHandle,
    state: &AppState,
    state_arc: &Arc<AppState>,
    conv: &Conversation,
    settings: &AppSettings,
    token: &CancelToken,
    acc: &mut Accum,
) -> Result<(), String> {
    let Some((provider, model)) = settings.resolve_chat_model(conv) else {
        return Err(
            "未配置对话模型：请在 设置 → 服务商 中添加模型，并在 设置 → 模型选择 中为「对话模型」选择已添加的模型"
                .into(),
        );
    };
    if provider.api_key.trim().is_empty() {
        return Err(format!(
            "服务商「{}」未配置 API Key，请在 设置 → 服务商 中填写",
            provider.name
        ));
    }

    let tools = tools::tools_for_mode(&conv.mode, conv.web_search);

    // 上下文自动压缩：用量达到阈值时，将早期对话摘要化（LLM 摘要 + 持久化）。
    // 若本轮发生了压缩，用压缩后的 summary/summarized_until 组装请求，
    // 保证压缩当轮即生效（否则该轮仍发送全量历史，可能超模型上限）
    let mut send_conv = conv.clone();
    if let Some((summary, until)) = try_compress_history(state, conv, provider, model).await {
        send_conv.summary = summary;
        send_conv.summarized_until = until;
    }

    let mut cur = build_outgoing(state, &send_conv, !tools.is_empty());

    for _round in 0..MAX_TOOL_ROUNDS {
        if token.is_cancelled() {
            return Err("已停止生成".into());
        }

        let analyzing = cur.iter().any(|m| matches!(m, OutMsg::Tool { .. }));
        emit(
            app,
            conv.id,
            "status",
            Some(if analyzing { "analyzing" } else { "thinking" }),
        );

        let mut turn = match provider.protocol.as_str() {
            PROTOCOL_OPENAI => {
                openai::run(app, state, provider, model, conv, &cur, &tools, token).await?
            }
            _ => {
                anthropic::run(app, state, provider, model, conv, &cur, &tools, token).await?
            }
        };

        // 流式中途被用户停止：已生成的部分内容先收进 acc，
        // 再以"已停止"结束，由外层 persist 保存（否则已输出的内容会全部丢失）
        if turn.stopped {
            acc.reasoning.push_str(&turn.reasoning);
            acc.content.push_str(&turn.content);
            return Err("已停止生成".into());
        }

        // 防御：未提供联网搜索工具时，忽略模型误发的 web_search 调用
        if !conv.web_search {
            turn.tool_calls.retain(|tc| tc.name != tools::TOOL_WEB_SEARCH);
        }

        acc.reasoning.push_str(&turn.reasoning);
        acc.content.push_str(&turn.content);

        // 流式中断（网络错误等）：内容已收进 acc，立即以错误收尾，
        // 由外层 persist 保存部分内容后再报告错误（避免"内容消失"）
        if let Some(err) = turn.error {
            return Err(err);
        }

        if turn.tool_calls.is_empty() {
            break;
        }

        let mut results: Vec<ToolResult> = Vec::new();
        for tc in &turn.tool_calls {
            if token.is_cancelled() {
                return Err("已停止生成".into());
            }
            let outcome = tools::execute_tool(app, state_arc, conv.id, &tc.name, &tc.arguments, settings, token).await?;
            // 产物实时推送到前端
            for art in &outcome.artifacts {
                emit_artifact(app, conv.id, art);
            }
            acc.artifacts.extend(outcome.artifacts.clone());
            let tr = ToolResult {
                tool_call_id: tc.id.clone(),
                content: outcome.content,
            };
            // 工具调用与其结果成对保存，保证中断/出错时持久化的数据也能正确回放
            acc.tool_calls.push(tc.clone());
            acc.tool_results.push(tr.clone());
            results.push(tr);
        }

        // 按 API 要求顺序拼接上下文：assistant(tool_use) 在前，user(tool_result) 紧跟其后
        cur.push(OutMsg::Assistant {
            content: turn.content,
            tool_calls: turn.tool_calls,
        });
        for tr in results {
            cur.push(OutMsg::Tool {
                tool_call_id: tr.tool_call_id,
                content: tr.content,
            });
        }
    }

    Ok(())
}

fn emit_artifact(app: &AppHandle, conv_id: i64, artifact: &Artifact) {
    let payload = serde_json::json!({
        "kind": "artifact",
        "conversation_id": conv_id,
        "item": artifact,
    });
    let _ = app.emit("chat_event", payload);
}

/// 上下文自动压缩：
/// 当估算用量达到阈值（默认 60%）且存在尚未摘要的早期消息时，
/// 调用对话模型将「已有摘要 + 早期消息」浓缩为一段新摘要并持久化，
/// 之后该部分消息不再直接发送，只发送摘要与最近的若干条消息。
/// 返回压缩后的 (summary, summarized_until)；未发生压缩返回 None。
async fn try_compress_history(
    state: &AppState,
    conv: &Conversation,
    provider: &ProviderConfig,
    model: &ModelConfig,
) -> Option<(String, i64)> {
    use crate::models::{CONTEXT_COMPRESS_THRESHOLD, CONTEXT_KEEP_LAST_MSGS};
    // 用量未达阈值或模型不可用时跳过
    let usage = state.db.context_usage(conv.id);
    if usage.percent < CONTEXT_COMPRESS_THRESHOLD {
        return None;
    }
    let rows = match state.db.list_messages_full(conv.id) {
        Ok(r) => r,
        Err(_) => return None,
    };
    // 仅考虑尚未摘要的消息；新增数量不足（如刚压缩过）则跳过，避免每轮都调用摘要
    let unsummarized: Vec<DbMessageRow> = rows
        .into_iter()
        .filter(|r| r.id > conv.summarized_until)
        .collect();
    if unsummarized.len() <= CONTEXT_KEEP_LAST_MSGS + 1 {
        return None;
    }
    // 仅摘要保留窗口之前的消息
    let cutoff_id = unsummarized[unsummarized.len() - CONTEXT_KEEP_LAST_MSGS].id;
    let new_rows: Vec<DbMessageRow> = unsummarized
        .into_iter()
        .filter(|r| r.id < cutoff_id)
        .collect();
    if new_rows.is_empty() {
        return None;
    }
    // 构造摘要输入：已有摘要 + 早期消息（不发送图片，只含文本）
    let mut msgs: Vec<OutMsg> = Vec::new();
    if !conv.summary.is_empty() {
        msgs.push(OutMsg::User {
            content: format!("【早期对话摘要】\n{}", conv.summary),
            images: Vec::new(),
            docs: String::new(),
        });
    }
    let mut images_left = 2;
    for row in new_rows {
        match row.role.as_str() {
            "user" => {
                let attachments: Vec<Attachment> =
                    serde_json::from_str(&row.attachments).unwrap_or_default();
                let mut images = Vec::new();
                for att in attachments {
                    if att.kind != "image" || images_left == 0 {
                        continue;
                    }
                    if let Some(p) = state.db.session_abs_path(conv.id, &att.path) {
                        if let Ok(bytes) = std::fs::read(&p) {
                            if !bytes.is_empty() && bytes.len() <= MAX_IMAGE_BYTES {
                                use base64::Engine;
                                images.push(ImageBlock {
                                    media_type: if att.mime.starts_with("image/") {
                                        att.mime.clone()
                                    } else {
                                        mime_from_name(&att.name)
                                    },
                                    base64: base64::engine::general_purpose::STANDARD
                                        .encode(&bytes),
                                });
                                images_left -= 1;
                            }
                        }
                    }
                }
                msgs.push(OutMsg::User {
                    content: row.content.clone(),
                    images,
                    docs: String::new(),
                });
            }
            "assistant" => {
                // 摘要输入不携带工具调用：历史 tool_use 块没有配对的
                // tool_result 会导致 Anthropic/OpenAI 摘要接口返回 400
                if row.content.trim().is_empty() {
                    // 空内容消息（如停止生成时的部分思考记录）同样跳过，
                    // 避免摘要接口因缺少 content 字段报 400
                    continue;
                }
                msgs.push(OutMsg::Assistant {
                    content: row.content.clone(),
                    tool_calls: Vec::new(),
                });
            }
            _ => {}
        }
    }
    let summary = match provider.protocol.as_str() {
        PROTOCOL_OPENAI => openai::summarize(state, provider, model, &msgs).await,
        _ => anthropic::summarize(state, provider, model, &msgs).await,
    };
    let Ok(summary) = summary else {
        // 摘要失败不应静默：记录日志便于定位（压缩是长会话免于"上下文已满"的关键）
        eprintln!("[compress] 会话 {} 摘要生成失败", conv.id);
        return None;
    };
    let _ = state
        .db
        .update_conversation_summary(conv.id, &summary, cutoff_id - 1);
    Some((summary, cutoff_id - 1))
}
