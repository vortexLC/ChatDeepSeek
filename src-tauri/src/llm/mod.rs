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
        MODE_BUILD => "\n\n【当前模式：Build】你可以使用编程工具（read_file / write_file / edit_file / delete_file / list_files / glob / grep / bash）在本会话隔离目录中创建和修改文件、执行命令，完成用户的开发任务。所有文件仅保存在本会话目录内，无法访问会话目录之外的文件（访问越界需用户确认）。必须通过函数调用机制调用工具，工具参数不要写在回复正文中（不要输出 JSON 参数）。",
        MODE_AGENT => "\n\n【当前模式：Agent】你拥有以下全部工具能力：\n\
1. 编程工具（read_file / write_file / edit_file / delete_file / list_files / glob / grep / bash）：在本会话隔离目录中读写文件、执行命令，完成开发任务；\n\
2. generate_image：生成图片。当用户要求生成、绘制、设计图片/图像/插画/海报/壁纸等任何视觉内容时，必须调用此工具——系统会调用专门的绘图模型完成生成并自动保存；\n\
3. generate_video：生成视频。当用户要求生成视频/动画/短片时，必须调用此工具（耗时约几分钟）。\n\
【重要】\n\
- 你确实具备图片与视频生成能力（由上述工具调用专门模型实现）。严禁回复「我无法生成图片/视频」「作为文本/语言模型我不能…」「你可以把提示词复制到 Midjourney / Stable Diffusion / Runway / 可灵等工具中」之类的话术。\n\
- 需要生成时，立即实际发起对应工具调用。工具参数不要写在回复正文中——严禁输出 JSON 提示词，或描述「调用 generate_image：prompt: …」这类文字而不真正调用；也严禁只说「正在为您生成」却不发起调用。调用工具前不要输出冗长的计划、分镜脚本或提示词设计。\n\
- 用户要求「先生成图片、再基于该图生成视频」时：先调用 generate_image，等拿到图片结果（含 images/xxx.png 路径）后，再调用 generate_video（mode=image2video，image 传刚生成的图片路径 images/xxx.png）。",
        MODE_IMAGE => "\n\n【当前模式：Image】你具备图片生成能力：当用户要求生成、绘制、设计任何图片/图像/插画/海报/壁纸等视觉内容时，必须调用 generate_image 工具——系统会调用专门的绘图模型完成生成并自动保存。\n\
【重要】严禁回复「我无法生成图片」「作为文本模型我不能画图」「请去 Midjourney / Stable Diffusion 等外部工具」等话术，也不要只输出提示词。必须通过函数调用机制调用 generate_image 工具，工具参数不要写在回复正文中（不要输出 JSON 提示词）。",
        MODE_VIDEO => "\n\n【当前模式：Video】你具备视频生成能力：当用户要求生成视频/动画/短片时，必须调用 generate_video 工具，并按需选择 mode：text2video 文生视频（无需图片）；image2video 图生视频（图片作首帧）；reference2video 参考图生视频（参考图片风格/主体，需 r2v 模型）。image/images 可传图片 URL、base64 或本会话内图片路径（如 images/xxx.png，即 generate_image 的产物）；图生/参考模式下若用户已上传图片也可不传 image，系统自动使用最近上传的图片。生成需几分钟，请告知用户耐心等待。\n\
【重要】严禁回复「我无法生成视频」等话术，也不要只给提示词。必须通过函数调用机制调用 generate_video 工具，工具参数不要写在回复正文中（不要输出 JSON 提示词）。",
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
4. 【总结】组织回答时遵循「总-分-总」的叙述逻辑：先给出直接明确的结论或概要，让用户第一时间获得答案；再分点展开，说明依据、推理过程与关键数据（引用搜索结果时附上来源链接 [来源](url)）；最后总结要点并适当补充注意事项。注意：不要输出「总」「分」「总结」之类的结构标题，直接以内容呈现。\n\
\n\
回答请使用规范的 Markdown 格式（列表、表格、加粗等；除用户要求外一般不要使用「# 标题」），保持简洁、准确、条理清晰，不要使用 emoji 过度装饰。"
        )
    } else {
        format!(
            "你是 ChatDeepSeek 智能助手。今天是 {date}。\n\
\n\
回答用户问题前请先思考：拆解问题意图、整理回答思路，再组织答案。\n\
组织回答时遵循「总-分-总」的叙述逻辑：先给出结论概要，再分点展开说明依据与细节，最后总结要点。注意：不要输出「总」「分」「总结」之类的结构标题，直接以内容呈现。\n\
请使用规范的 Markdown 格式（列表、表格、加粗等；除用户要求外一般不要使用「# 标题」），保持简洁、准确、条理清晰，不要使用 emoji 过度装饰。\n\
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
    let started = std::time::Instant::now();
    log::info!(
        "[agent] 会话 {} 开始生成（模式: {}, 模型: {}, 联网搜索: {}, 深度思考: {}）",
        conv.id,
        conv.mode,
        conv.model,
        conv.web_search,
        conv.deep_think
    );
    emit(&app, conv.id, "status", Some("thinking"));
    let mut acc = Accum::default();
    let result = run_agent_inner(&app, &state, &state, &conv, &settings, &token, &mut acc).await;
    match result {
        Ok(()) => {
            log::info!(
                "[agent] 会话 {} 生成完成，耗时 {:?}，输出 {} 字符",
                conv.id,
                started.elapsed(),
                acc.content.chars().count()
            );
            if let Err(e) = persist(&state, &conv, &acc) {
                // 回复已生成但保存失败：明确告知用户，避免"内容消失"而不知原因
                log::error!("[persist] 会话 {} 回复保存失败: {e}", conv.id);
                emit(&app, conv.id, "error", Some(&e));
                return;
            }
            emit(&app, conv.id, "done", None);
        }
        Err(msg) => {
            if !acc.is_empty() {
                if let Err(e) = persist(&state, &conv, &acc) {
                    log::error!("[persist] 会话 {} 部分回复保存失败: {e}", conv.id);
                    emit(&app, conv.id, "error", Some(&e));
                    return;
                }
            }
            // 用户主动停止不是错误：以独立事件静默收尾，避免弹出红色错误横幅
            if msg == "已停止生成" {
                log::info!("[agent] 会话 {} 已停止，耗时 {:?}", conv.id, started.elapsed());
                emit(&app, conv.id, "stopped", None);
            } else {
                log::warn!("[agent] 会话 {} 出错，耗时 {:?}: {}", conv.id, started.elapsed(), msg);
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
    // 用户已点击停止时跳过压缩，直接以"已停止"收尾
    if token.is_cancelled() {
        return Err("已停止生成".into());
    }
    let mut send_conv = conv.clone();
    if let Some((summary, until)) =
        try_compress_history(state, conv, provider, model, token).await
    {
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

        // 流式中断（网络错误等）：内容已收进 acc，立即以错误收尾，
        // 由外层 persist 保存部分内容后再报告错误（避免"内容消失"）
        if let Some(err) = turn.error {
            acc.content.push_str(&turn.content);
            return Err(err);
        }

        // 无工具调用时尝试识别文本形式的工具调用（JSON 兜底）；
        // 命中时展示内容剥离 JSON 参数，避免把 prompt/size 等参数展示给用户
        let textual = if turn.tool_calls.is_empty() {
            find_textual_tool_call(&turn.content, &conv.mode)
        } else {
            None
        };
        let display_content = match &textual {
            Some((_, _, start, end)) => {
                let mut c = String::with_capacity(turn.content.len());
                c.push_str(&turn.content[..*start]);
                c.push_str(&turn.content[*end..]);
                c.trim().to_string()
            }
            None => turn.content.clone(),
        };
        acc.content.push_str(&display_content);

        if turn.tool_calls.is_empty() {
            // 文本 JSON 兜底：识别并代为执行，避免"只给提示词不生成"
            if let Some((tool_name, args, _, _)) = textual {
                if tools.iter().any(|t| t["name"].as_str() == Some(tool_name.as_str())) {
                    let id = format!("text_call_{}", crate::db::now_ms());
                    let calls = vec![ToolCall {
                        id: id.clone(),
                        name: tool_name,
                        arguments: args.to_string(),
                    }];
                    let results =
                        execute_tool_calls(app, state_arc, conv.id, &calls, settings, token, acc)
                            .await?;
                    // 按 API 要求顺序拼接上下文：assistant(tool_use) 在前，user(tool_result) 紧跟
                    cur.push(OutMsg::Assistant {
                        content: display_content,
                        tool_calls: calls,
                    });
                    for tr in results {
                        cur.push(OutMsg::Tool {
                            tool_call_id: tr.tool_call_id,
                            content: tr.content,
                        });
                    }
                    continue;
                }
            }
            // 承诺话术纠正：模型只说「正在为您生成…请稍等」而未实际调用工具时，
            // 不结束回合，回喂提示要求提供工具参数（模型随后输出 JSON 由上面兜底执行），
            // 避免"说而不做"直接结束
            if let Some(hint) = promise_reminder_hint(&turn.content, &conv.mode) {
                cur.push(OutMsg::User {
                    content: hint,
                    images: Vec::new(),
                    docs: String::new(),
                });
                continue;
            }
            break;
        }

        // 防御：未提供联网搜索工具时，忽略模型误发的 web_search 调用
        if !conv.web_search {
            turn.tool_calls.retain(|tc| tc.name != tools::TOOL_WEB_SEARCH);
        }
        if turn.tool_calls.is_empty() {
            break;
        }

        let results = execute_tool_calls(app, state_arc, conv.id, &turn.tool_calls, settings, token, acc).await?;

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

/// 执行一组工具调用：逐个执行、推送产物、累计到 acc，返回按顺序的 ToolResult。
/// 权限确认类错误（用户拒绝/超时）不终止任务：转为工具结果回喂模型，
/// 让其调整策略（如改用会话内路径）继续，而非整个生成直接失败
async fn execute_tool_calls(
    app: &AppHandle,
    state_arc: &Arc<AppState>,
    conv_id: i64,
    tool_calls: &[ToolCall],
    settings: &AppSettings,
    token: &CancelToken,
    acc: &mut Accum,
) -> Result<Vec<ToolResult>, String> {
    let mut results: Vec<ToolResult> = Vec::new();
    for tc in tool_calls {
        if token.is_cancelled() {
            return Err("已停止生成".into());
        }
        let started = std::time::Instant::now();
        let outcome = match tools::execute_tool(
            app, state_arc, conv_id, &tc.name, &tc.arguments, settings, token,
        )
        .await
        {
            Ok(o) => o,
            Err(e) if is_permission_error(&e) => {
                // 权限拒绝不是致命错误：作为工具结果返回给模型继续
                log::info!("[tool] 会话 {} 调用 {} 被用户拒绝", conv_id, tc.name);
                tools::ToolOutcome {
                    content: e,
                    artifacts: Vec::new(),
                }
            }
            Err(e) => {
                log::error!("[tool] 会话 {} 调用 {} 失败: {}", conv_id, tc.name, e);
                return Err(e);
            }
        };
        log::info!(
            "[tool] 会话 {} 调用 {} 完成，耗时 {:?}，产物 {} 个",
            conv_id,
            tc.name,
            started.elapsed(),
            outcome.artifacts.len()
        );
        // 产物实时推送到前端
        for art in &outcome.artifacts {
            emit_artifact(app, conv_id, art);
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
    Ok(results)
}

/// 权限确认类错误（用户拒绝/确认超时/确认失效）：非致命，转为工具结果回喂模型
fn is_permission_error(e: &str) -> bool {
    e.starts_with("用户拒绝了该操作")
        || e.starts_with("等待用户确认超时")
        || e.starts_with("权限确认已失效")
}

/// 从模型文本回复中识别"文本形式的工具调用"（兼容不支持 function calling、
/// 或把工具参数写进正文的模型）。识别优先级：
/// 1. 整体即 JSON 对象；
/// 2. 正文中的 ```json / ``` 代码块；
/// 3. 正文中第一个 { 到最后一个 } 的 JSON 片段。
/// 返回 (工具名, 参数 JSON, JSON 在 content 中的起止位置 [start, end))，
/// 位置用于把 JSON 参数从展示内容中剥离。
fn find_textual_tool_call(
    content: &str,
    mode: &str,
) -> Option<(String, serde_json::Value, usize, usize)> {
    // 候选 1：整体即 JSON
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(content.trim()) {
        if let Some(r) = classify_textual_tool_call(&v, mode) {
            return Some((r.0, r.1, 0, content.len()));
        }
    }
    // 候选 2：```json / ``` 代码块（位置覆盖整个代码块，剥离时一并删除）
    if let Some(start) = content.find("```") {
        let after = &content[start + 3..];
        if let Some(rel_end) = after.find("```") {
            let end = start + 3 + rel_end + 3;
            let mut block = content[start + 3..end - 3].trim();
            // 去掉代码块语言标记（```json 后的 "json"）
            if let Some(rest) = block
                .strip_prefix("json")
                .or_else(|| block.strip_prefix("JSON"))
            {
                block = rest.trim();
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(block) {
                if let Some(r) = classify_textual_tool_call(&v, mode) {
                    return Some((r.0, r.1, start, end));
                }
            }
        }
    }
    // 候选 3：正文中第一个 { 到最后一个 } 的片段
    if let (Some(open), Some(close)) = (content.find('{'), content.rfind('}')) {
        if close > open {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content[open..=close]) {
                if let Some(r) = classify_textual_tool_call(&v, mode) {
                    return Some((r.0, r.1, open, close + 1));
                }
            }
        }
    }
    None
}

/// 文本工具调用识别（丢弃位置信息，供测试使用）
#[cfg(test)]
fn parse_textual_tool_call(content: &str, mode: &str) -> Option<(String, serde_json::Value)> {
    find_textual_tool_call(content, mode).map(|(n, a, _, _)| (n, a))
}

/// 检测模型是否只输出了"承诺话术"（如「正在为您生成…请稍等」）而未实际调用工具。
/// 命中时返回回喂提示，要求模型提供工具参数（随后模型输出 JSON 由文本兜底执行）。
/// 约束：仅生成模式、内容含承诺词、且内容较短（长回答不打断）。
fn promise_reminder_hint(content: &str, mode: &str) -> Option<String> {
    let c = content.trim();
    if c.chars().count() > 200 {
        return None;
    }
    const PROMISE_WORDS: [&str; 6] = ["正在生成", "请稍等", "请稍候", "马上", "为您生成", "开始生成"];
    if !PROMISE_WORDS.iter().any(|w| c.contains(w)) {
        return None;
    }
    let tool_hint = match mode {
        MODE_IMAGE => format!(
            "请立即调用 {} 工具并提供参数（图片描述 prompt）",
            tools::TOOL_GENERATE_IMAGE
        ),
        MODE_VIDEO => format!(
            "请立即调用 {} 工具并提供参数（mode 与 prompt）",
            tools::TOOL_GENERATE_VIDEO
        ),
        MODE_AGENT => format!(
            "请立即调用相应工具并提供参数（如 {} 的 prompt、{} 的 mode/prompt、或文件操作参数）",
            tools::TOOL_GENERATE_IMAGE,
            tools::TOOL_GENERATE_VIDEO
        ),
        _ => return None,
    };
    Some(format!(
        "你尚未真正调用工具，只说了一句承诺话术。{tool_hint}，不要只说「正在生成」之类的文字。"
    ))
}

/// 已知工具名集合（文本兜底识别的合法范围）
const KNOWN_TOOL_NAMES: [&str; 11] = [
    tools::TOOL_WEB_SEARCH,
    tools::TOOL_READ_FILE,
    tools::TOOL_WRITE_FILE,
    tools::TOOL_EDIT_FILE,
    tools::TOOL_DELETE_FILE,
    tools::TOOL_LIST_FILES,
    tools::TOOL_GLOB,
    tools::TOOL_GREP,
    tools::TOOL_BASH,
    tools::TOOL_GENERATE_IMAGE,
    tools::TOOL_GENERATE_VIDEO,
];

/// 按工具特征字段分类 JSON 对象 → (工具名, 参数 JSON)。
/// 支持两种形态：
/// 1. 包装格式 {"name": "generate_image", "arguments": {"prompt": "..."}}
///    （模型模拟 OpenAI tool_call 结构，arguments 可为对象或 JSON 字符串）；
/// 2. 平铺字段（{"prompt": ...}、{"path": ...}、{"command": ...} 等）。
fn classify_textual_tool_call(
    v: &serde_json::Value,
    mode: &str,
) -> Option<(String, serde_json::Value)> {
    let obj = v.as_object()?;

    // ---- 包装格式 ----
    if let (Some(name), Some(args_raw)) = (
        obj.get("name").and_then(|n| n.as_str()),
        obj.get("arguments"),
    ) {
        let args = match args_raw {
            serde_json::Value::Object(_) => args_raw.clone(),
            serde_json::Value::String(s) => serde_json::from_str::<serde_json::Value>(s).ok()?,
            _ => return None,
        };
        let non_empty = args.as_object().map(|o| !o.is_empty()).unwrap_or(false);
        if non_empty && KNOWN_TOOL_NAMES.contains(&name) {
            return Some((name.to_string(), args));
        }
        // 包装格式但 name 未知/参数为空：不继续按平铺判断（结构已明确是包装格式）
        return None;
    }

    // ---- 生成工具（平铺字段）----
    if let Some(prompt) = obj.get("prompt").and_then(|p| p.as_str()) {
        if !prompt.trim().is_empty() {
            let video_hint = obj.contains_key("mode")
                || obj.contains_key("image")
                || obj.contains_key("images")
                || mode == MODE_VIDEO;
            if video_hint {
                return Some((tools::TOOL_GENERATE_VIDEO.to_string(), v.clone()));
            }
            if mode == MODE_IMAGE || mode == MODE_AGENT {
                return Some((tools::TOOL_GENERATE_IMAGE.to_string(), v.clone()));
            }
        }
    }
    // ---- 文件工具（Build / Agent）----
    if mode == MODE_BUILD || mode == MODE_AGENT {
        if obj.contains_key("command") {
            return Some((tools::TOOL_BASH.to_string(), v.clone()));
        }
        if let Some(path) = obj.get("path").and_then(|p| p.as_str()) {
            if !path.trim().is_empty() {
                if obj.contains_key("old_string") {
                    return Some((tools::TOOL_EDIT_FILE.to_string(), v.clone()));
                }
                if obj.contains_key("content") {
                    return Some((tools::TOOL_WRITE_FILE.to_string(), v.clone()));
                }
                // 仅 path：只读安全，按 read_file 处理
                return Some((tools::TOOL_READ_FILE.to_string(), v.clone()));
            }
        }
        if obj.contains_key("pattern") {
            return Some((tools::TOOL_GREP.to_string(), v.clone()));
        }
        if obj.contains_key("dir") {
            return Some((tools::TOOL_LIST_FILES.to_string(), v.clone()));
        }
    }
    None
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
    token: &CancelToken,
) -> Option<(String, i64)> {
    use crate::models::{CONTEXT_COMPRESS_THRESHOLD, CONTEXT_KEEP_LAST_MSGS};
    // 用户已点击停止：跳过压缩，避免无谓的摘要请求
    if token.is_cancelled() {
        return None;
    }
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
    // 摘要请求前再次检查停止状态（摘要本身也有 60 秒超时兜底）
    if token.is_cancelled() {
        return None;
    }
    let summary = match provider.protocol.as_str() {
        PROTOCOL_OPENAI => openai::summarize(state, provider, model, &msgs).await,
        _ => anthropic::summarize(state, provider, model, &msgs).await,
    };
    let Ok(summary) = summary else {
        // 摘要失败不应静默：记录日志便于定位（压缩是长会话免于"上下文已满"的关键）
        log::error!("[compress] 会话 {} 摘要生成失败", conv.id);
        return None;
    };
    log::info!(
        "[compress] 会话 {} 自动压缩完成：摘要 {} 字符，压缩至消息 {}",
        conv.id,
        summary.chars().count(),
        cutoff_id - 1
    );
    let _ = state
        .db
        .update_conversation_summary(conv.id, &summary, cutoff_id - 1);
    Some((summary, cutoff_id - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 文本形式工具调用解析：JSON/代码块/视频字段/模式约束
    #[test]
    fn textual_tool_call_parsing() {
        // 纯 JSON → generate_image（Image 模式）
        let r = parse_textual_tool_call(r#"{"prompt": "一只猫"}"#, MODE_IMAGE);
        assert!(r.is_some());
        let (name, args) = r.unwrap();
        assert_eq!(name, tools::TOOL_GENERATE_IMAGE);
        assert_eq!(args["prompt"], "一只猫");

        // 代码块包裹同样识别
        let r = parse_textual_tool_call("```json\n{\"prompt\": \"狗\"}\n```", MODE_IMAGE);
        assert!(r.is_some());

        // 文字 + 代码块 JSON（模型把工具参数写进正文的典型场景）
        let reply = "### 总\n已为您构思并生成一张图片…\n系统正在根据以下参数为您绘制图像：\n```json\n{\"prompt\": \"一只猫\"}\n```";
        let r = parse_textual_tool_call(reply, MODE_IMAGE);
        assert!(r.is_some());
        let (name, args) = r.unwrap();
        assert_eq!(name, tools::TOOL_GENERATE_IMAGE);
        assert_eq!(args["prompt"], "一只猫");

        // 文字中直接内嵌 JSON（无代码块）同样识别
        let reply = "好的，正在为您生成：{\"prompt\": \"一只狗\", \"image_size\": \"1024x1024\"}";
        let r = parse_textual_tool_call(reply, MODE_IMAGE);
        assert!(r.is_some());
        assert_eq!(r.unwrap().1["image_size"], "1024x1024");

        // 正文包含多个 JSON 片段时优先取代码块
        let reply = "参考 {\"a\": 1}，生成：```json\n{\"prompt\": \"猫\"}\n```";
        let r = parse_textual_tool_call(reply, MODE_IMAGE);
        assert!(r.is_some());
        assert_eq!(r.unwrap().1["prompt"], "猫");

        // 位置信息：剥离展示内容中的 JSON 参数用
        let reply = "为您生成了一张图片。{\"prompt\": \"猫\"}";
        let r = find_textual_tool_call(reply, MODE_IMAGE);
        assert!(r.is_some());
        let (_, _, start, end) = r.unwrap();
        assert_eq!(&reply[start..end], "{\"prompt\": \"猫\"}");
        // 代码块整体剥离（含 ``` 标记）
        let reply = "为您生成：\n```json\n{\"prompt\": \"狗\"}\n```";
        let r = find_textual_tool_call(reply, MODE_IMAGE);
        assert!(r.is_some());
        let (_, _, start, end) = r.unwrap();
        assert_eq!(&reply[start..end], "```json\n{\"prompt\": \"狗\"}\n```");

        // 包装格式（模型模拟 tool_call 结构：name + arguments）
        let r = parse_textual_tool_call(
            r#"{"name": "generate_image", "arguments": {"prompt": "一只猫"}}"#,
            MODE_IMAGE,
        );
        assert!(r.is_some());
        let (name, args) = r.unwrap();
        assert_eq!(name, tools::TOOL_GENERATE_IMAGE);
        assert_eq!(args["prompt"], "一只猫");

        // arguments 为 JSON 字符串
        let r = parse_textual_tool_call(
            r#"{"name": "generate_image", "arguments": "{\"prompt\": \"狗\"}"}"#,
            MODE_IMAGE,
        );
        assert!(r.is_some());
        assert_eq!(r.unwrap().1["prompt"], "狗");

        // 未知 name → 不识别
        assert!(
            parse_textual_tool_call(
                r#"{"name": "hack", "arguments": {"prompt": "x"}}"#,
                MODE_IMAGE
            )
            .is_none()
        );

        // 含 mode 字段 → generate_video
        let r = parse_textual_tool_call(r#"{"mode": "text2video", "prompt": "视频"}"#, MODE_IMAGE);
        assert_eq!(r.unwrap().0, tools::TOOL_GENERATE_VIDEO);

        // Video 模式缺省 → generate_video
        let r = parse_textual_tool_call(r#"{"prompt": "视频"}"#, MODE_VIDEO);
        assert_eq!(r.unwrap().0, tools::TOOL_GENERATE_VIDEO);

        // 普通文本 / 无 prompt / Chat 模式 → 不识别
        assert!(parse_textual_tool_call("你好，有什么可以帮你", MODE_IMAGE).is_none());
        assert!(parse_textual_tool_call(r#"{"a": 1}"#, MODE_IMAGE).is_none());
        assert!(parse_textual_tool_call(r#"{"prompt": "x"}"#, MODE_CHAT).is_none());

        // 文件工具（Build / Agent 模式）
        let r = parse_textual_tool_call(r#"{"path": "a.txt"}"#, MODE_BUILD);
        assert_eq!(r.unwrap().0, tools::TOOL_READ_FILE);
        let r = parse_textual_tool_call(r#"{"path": "a.txt", "content": "hi"}"#, MODE_BUILD);
        assert_eq!(r.unwrap().0, tools::TOOL_WRITE_FILE);
        let r = parse_textual_tool_call(r#"{"path": "a.txt", "old_string": "x", "new_string": "y"}"#, MODE_AGENT);
        assert_eq!(r.unwrap().0, tools::TOOL_EDIT_FILE);
        let r = parse_textual_tool_call(r#"{"command": "dir"}"#, MODE_BUILD);
        assert_eq!(r.unwrap().0, tools::TOOL_BASH);
        let r = parse_textual_tool_call(r#"{"pattern": "hello"}"#, MODE_BUILD);
        assert_eq!(r.unwrap().0, tools::TOOL_GREP);
        let r = parse_textual_tool_call(r#"{"dir": "src"}"#, MODE_AGENT);
        assert_eq!(r.unwrap().0, tools::TOOL_LIST_FILES);

        // 文件工具 JSON 在 Image / Chat 模式不识别
        assert!(parse_textual_tool_call(r#"{"path": "a.txt"}"#, MODE_IMAGE).is_none());
        assert!(parse_textual_tool_call(r#"{"command": "dir"}"#, MODE_CHAT).is_none());

        // 权限拒绝错误识别
        assert!(is_permission_error("用户拒绝了该操作：bash cd /d C:\\"));
        assert!(is_permission_error("等待用户确认超时，已拒绝该操作"));
        assert!(is_permission_error("权限确认已失效"));
        assert!(!is_permission_error("图片生成 API 错误 (400)"));
        assert!(!is_permission_error("已停止生成"));

        // 承诺话术纠正
        let h = promise_reminder_hint("正在为您生成中式古风庭院图片，请稍等。", MODE_IMAGE);
        assert!(h.is_some());
        assert!(h.unwrap().contains("generate_image"));
        let h = promise_reminder_hint("马上为您生成视频，请稍候。", MODE_VIDEO);
        assert!(h.is_some());
        assert!(h.unwrap().contains("generate_video"));
        // 无承诺词 / 长回答 / 非生成模式 → 不打断
        assert!(promise_reminder_hint("好的，我来分析一下这个问题。", MODE_IMAGE).is_none());
        assert!(promise_reminder_hint("正在生成".repeat(120).as_str(), MODE_IMAGE).is_none());
        assert!(promise_reminder_hint("正在生成", MODE_CHAT).is_none());
    }
}
