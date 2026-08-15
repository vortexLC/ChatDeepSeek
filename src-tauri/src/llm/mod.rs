pub mod openai;
pub mod search;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter};

use crate::agent::tools;
use crate::commands::{AppState, emit};
use crate::models::*;

pub const MAX_TOOL_ROUNDS: usize = 16;

#[derive(Clone)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
    /// watch 通道：广播取消事件给所有等待者。
    /// 此前用 Notify，notify_one 只能唤醒一个等待者——流式循环与工具执行
    /// 同时等待时，停止操作可能丢失唤醒（表现为停止按钮失灵）
    notify: Arc<tokio::sync::watch::Sender<bool>>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(tokio::sync::watch::Sender::new(false)),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
        let _ = self.notify.send(true);
    }

    /// 阻塞直到取消。watch 通道自带"最后值"语义，无丢失唤醒问题
    pub async fn wait(&self) {
        let mut rx = self.notify.subscribe();
        while !self.is_cancelled() {
            if rx.changed().await.is_err() {
                return;
            }
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
    /// 执行时间线：思考与各次工具调用按发生顺序记录
    pub steps: Vec<ToolStep>,
}

impl Accum {
    fn is_empty(&self) -> bool {
        self.reasoning.is_empty()
            && self.content.is_empty()
            && self.search_items.is_empty()
            && self.tool_calls.is_empty()
            && self.artifacts.is_empty()
            && self.steps.is_empty()
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

/// 构建系统提示词（OpenAI 协议）。
/// 按会话模式附加工具使用说明；生成类模式明确要求模型调用生成工具、禁止以
/// "文本模型无法生成"为由拒绝，也不要让用户复制提示词去外部工具生成。
pub(crate) fn build_system_prompt(conv: &Conversation) -> String {
    let date = today_cn();
    let mode_hint = match conv.mode.as_str() {
        MODE_AGENT => "\n\n【当前模式：Agent】你拥有以下全部工具能力：\n\
1. 编程工具（read_file / write_file / edit_file / delete_file / list_files / glob / grep / bash）：在本会话隔离目录中读写文件、执行命令，完成开发任务；\n\
2. generate_image：生成图片。当用户要求生成、绘制、设计图片/图像/插画/海报/壁纸等任何视觉内容时，必须调用此工具——系统会调用专门的绘图模型完成生成并自动保存。\n\
【工具调用效率】需要同时获取多项独立信息（如多次搜索、读取多个文件、列目录+搜索）时，请在同一轮中并行发起多个工具调用，系统会并发执行只读工具，显著缩短等待时间；有先后依赖的操作（先读后写、先建目录再写文件）必须分轮进行。\n\
【重要】\n\
- 你确实具备图片生成能力（由上述工具调用专门模型实现）。严禁回复「我无法生成图片」「作为文本/语言模型我不能…」「你可以把提示词复制到 Midjourney / Stable Diffusion 等工具中」之类的话术。\n\
- 需要生成时，立即实际发起对应工具调用。工具参数不要写在回复正文中——严禁输出 JSON 提示词，或描述「调用 generate_image：prompt: …」这类文字而不真正调用；也严禁只说「正在为您生成」却不发起调用。调用工具前不要输出冗长的计划或提示词设计。\n\
- 若当前环境确实无法发起原生函数调用，可直接在回复正文中输出 JSON 格式的工具参数（形如 {\"prompt\": \"...\"}，可用代码块包裹），系统会自动识别并执行。无论哪种方式，都不要在回复中讨论工具调用格式，也不要输出 call、函数名等调用标记，直接输出参数即可。",
        // chat 模式（含图片生成）
        _ => "\n\n【当前模式：Chat】你具备图片生成能力：当用户要求生成、绘制、设计任何图片/图像/插画/海报/壁纸等视觉内容时，必须调用 generate_image 工具——系统会调用专门的绘图模型完成生成并自动保存。\n\
【重要】严禁回复「我无法生成图片」「作为文本模型我不能画图」「请去 Midjourney / Stable Diffusion 等外部工具」等话术，也不要只输出提示词。必须通过函数调用机制调用 generate_image 工具，工具参数不要写在回复正文中（不要输出 JSON 提示词）。若当前环境确实无法发起原生函数调用，可直接在回复正文中输出 JSON 格式的工具参数（形如 {\"prompt\": \"...\"}，可用代码块包裹），系统会自动识别并执行。无论哪种方式，都不要在回复中讨论工具调用格式，也不要输出 call、函数名等调用标记。",
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
    // 硬截断保护：估算总量超过模型上下文容量时，从最旧的消息组开始丢弃
    // （压缩失败 / 单条超大消息等场景的兜底，避免直接发送导致 API 400）
    truncate_outgoing(state, conv, out)
}

/// 按消息组硬截断 outgoing 消息，确保估算 token 不超过模型上下文容量。
/// 消息组：User 消息为一组；Assistant（含 tool_calls）+ 其紧随的 Tool
/// 结果为一组——工具调用与结果必须同组，拆开会产生孤儿 tool_result
/// 导致 API 400。首条摘要消息与最后一组始终保留。
fn truncate_outgoing(
    state: &AppState,
    conv: &Conversation,
    out: Vec<OutMsg>,
) -> Vec<OutMsg> {
    let total = state.db.get_settings().chat_context_total(conv);
    let overhead = if conv.mode == "agent" {
        crate::models::CONTEXT_OVERHEAD_AGENT
    } else {
        crate::models::CONTEXT_OVERHEAD_CHAT
    };
    let outmsg_tokens = |m: &OutMsg| -> u64 {
        match m {
            OutMsg::User { content, images, docs } => {
                crate::db::estimate_tokens(content)
                    + crate::db::estimate_tokens(docs)
                    + (images.len() as u64) * crate::models::CONTEXT_IMAGE_TOKENS
            }
            OutMsg::Assistant { content, tool_calls } => {
                crate::db::estimate_tokens(content)
                    + tool_calls
                        .iter()
                        .map(|tc| crate::db::estimate_tokens(&tc.arguments))
                        .sum::<u64>()
            }
            OutMsg::Tool { content, .. } => crate::db::estimate_tokens(content),
        }
    };
    let sum: u64 = out.iter().map(&outmsg_tokens).sum();
    if sum + overhead <= total {
        return out;
    }
    // 划分消息组：(组内起始下标, 组 token)
    let mut groups: Vec<(usize, u64)> = Vec::new();
    for (i, m) in out.iter().enumerate() {
        let starts = matches!(m, OutMsg::User { .. } | OutMsg::Assistant { .. });
        if starts || groups.is_empty() {
            groups.push((i, outmsg_tokens(m)));
        } else {
            groups.last_mut().unwrap().1 += outmsg_tokens(m);
        }
    }
    // 摘要消息（首条 User 且存在摘要）不参与丢弃
    let has_summary = !conv.summary.is_empty() && matches!(out.first(), Some(OutMsg::User { .. }));
    let first_droppable = if has_summary { 1 } else { 0 };
    if groups.len() <= first_droppable + 1 {
        return out; // 无可丢弃组（摘要 + 最后一组）
    }
    let target = total.saturating_sub(overhead);
    let mut acc = sum;
    // 从最旧可丢组开始丢弃，直到达标；cut = 第一个保留组的起始消息下标
    let mut cut: Option<usize> = None;
    for gi in first_droppable..groups.len() - 1 {
        if acc <= target {
            cut = Some(groups[gi].0);
            break;
        }
        acc = acc.saturating_sub(groups[gi].1);
    }
    let Some(cut) = cut else {
        return out; // 丢光可丢组仍超限（最后一组本身超大）：原样返回交由 API 报错
    };
    let dropped_groups = groups[first_droppable..]
        .iter()
        .filter(|(s, _)| *s < cut)
        .count();
    log::warn!(
        "[context] 会话 {} 估算 {} token 超容量 {total}，硬截断丢弃最旧 {} 组消息（保留摘要与近期消息）",
        conv.id,
        sum + overhead,
        dropped_groups
    );
    let mut kept: Vec<OutMsg> = out.iter().take(first_droppable).cloned().collect();
    kept.extend(out.into_iter().skip(cut));
    kept
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
        &acc.steps,
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

    for round in 0..MAX_TOOL_ROUNDS {
        if token.is_cancelled() {
            return Err("已停止生成".into());
        }

        // 最后一轮不再携带工具：强制模型基于已有工具结果给出最终回答，
        // 避免 Agent 长任务到达轮次上限时被静默截断（最后的工具结果得不到回应）
        let round_tools: &[serde_json::Value] = if round + 1 == MAX_TOOL_ROUNDS {
            &[]
        } else {
            &tools
        };

        let analyzing = cur.iter().any(|m| matches!(m, OutMsg::Tool { .. }));
        emit(
            app,
            conv.id,
            "status",
            Some(if analyzing { "analyzing" } else { "thinking" }),
        );

        let round_started = std::time::Instant::now();
        let mut turn =
            openai::run(app, state, provider, model, conv, &cur, round_tools, token).await?;

        // 思考步骤进入时间线（发生在工具调用之前）：
        // 耗时以本轮模型调用总时长计，纯对话场景同样形成"深度思考"步骤
        if !turn.reasoning.is_empty() {
            let step = ToolStep {
                kind: "reasoning".into(),
                label: "深度思考".into(),
                tool: String::new(),
                duration_ms: round_started.elapsed().as_millis() as u64,
                items: Vec::new(),
            };
            emit(
                app,
                conv.id,
                "tool_step",
                serde_json::to_string(&step).ok().as_deref(),
            );
            acc.steps.push(step);
        }

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
            // 避免"说而不做"直接结束。若本轮已成功调用过生成工具（图片已生成），
            // 模型说"正在生成中"是真实陈述，不再回喂提示——
            // 否则会形成"回喂提示 → 模型纠结工具格式 → 再输出 JSON → 重复提交"的循环
            let tool_already_called = acc
                .tool_calls
                .iter()
                .any(|tc| tc.name == tools::TOOL_GENERATE_IMAGE);
            if let Some(hint) = promise_reminder_hint(&turn.content, &conv.mode, tool_already_called) {
                cur.push(OutMsg::User {
                    content: hint,
                    images: Vec::new(),
                    docs: String::new(),
                });
                continue;
            }
            break;
        }

        let results = execute_tool_calls(app, state_arc, conv.id, &turn.tool_calls, settings, token, acc).await?;

        // 按 API 要求顺序拼接上下文：assistant(tool_calls) 在前，tool 结果紧跟其后。
        // 有原生工具调用时不做文本兜底（textual 必为 None），display_content 即 turn.content
        cur.push(OutMsg::Assistant {
            content: display_content,
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

/// 可并行执行的工具集合：只读/纯网络、不写文件、不触发权限确认弹窗。
/// 含写操作或可能请求权限确认（pending_perms 以 conv_id 为 key，并发确认会互相
/// 覆盖）的工具必须串行执行
const PARALLEL_SAFE_TOOLS: [&str; 5] = [
    tools::TOOL_WEB_SEARCH,
    tools::TOOL_LIST_FILES,
    tools::TOOL_GLOB,
    tools::TOOL_GREP,
    tools::TOOL_GENERATE_IMAGE,
];

/// 执行单个工具调用：取消为致命错误上抛，其余（含权限拒绝、工具自身
/// 失败如 API 限流/网络超时）转为结果回喂模型，由模型决定重试或调整
/// 策略，避免单个工具失败中止整个任务
async fn execute_one_tool(
    app: &AppHandle,
    state_arc: &Arc<AppState>,
    conv_id: i64,
    tc: &ToolCall,
    settings: &AppSettings,
    token: &CancelToken,
) -> Result<tools::ToolOutcome, String> {
    match tools::execute_tool(app, state_arc, conv_id, &tc.name, &tc.arguments, settings, token)
        .await
    {
        Ok(o) => Ok(o),
        Err(e) if e == "已停止生成" => Err(e),
        Err(e) => {
            log::error!("[tool] 会话 {} 调用 {} 失败: {}", conv_id, tc.name, e);
            Ok(tools::ToolOutcome {
                content: format!("工具执行失败: {e}"),
                artifacts: Vec::new(),
                search_items: Vec::new(),
            })
        }
    }
}

/// 执行一组工具调用：推送产物、累计到 acc，返回按调用顺序的 ToolResult。
///
/// 并行优化：当本批调用全部属于可并行集合（只读/纯网络工具）且数量 ≥ 2 时，
/// 用 join_all 并发执行——模型一轮发起多个 tool_calls（并行调用）时，
/// 总耗时从「各工具之和」降为「最慢一个」。写文件 / bash / 可能触发
/// 权限确认的批次保持串行，避免文件竞争与权限弹窗互相覆盖。
async fn execute_tool_calls(
    app: &AppHandle,
    state_arc: &Arc<AppState>,
    conv_id: i64,
    tool_calls: &[ToolCall],
    settings: &AppSettings,
    token: &CancelToken,
    acc: &mut Accum,
) -> Result<Vec<ToolResult>, String> {
    let parallel = tool_calls.len() >= 2
        && tool_calls
            .iter()
            .all(|tc| PARALLEL_SAFE_TOOLS.contains(&tc.name.as_str()));

    let outcomes: Vec<(std::time::Duration, tools::ToolOutcome)> = if parallel {
        log::info!(
            "[tool] 会话 {} 并行执行 {} 个工具调用: {}",
            conv_id,
            tool_calls.len(),
            tool_calls.iter().map(|tc| tc.name.as_str()).collect::<Vec<_>>().join(", ")
        );
        let started = std::time::Instant::now();
        let futs = tool_calls
            .iter()
            .map(|tc| execute_one_tool(app, state_arc, conv_id, tc, settings, token));
        let all = futures_util::future::join_all(futs).await;
        let elapsed = started.elapsed();
        // 按原顺序检查结果：任一致命错误即上抛（与串行语义一致）
        let mut oks = Vec::with_capacity(all.len());
        for r in all {
            oks.push(r?);
        }
        oks.into_iter().map(|o| (elapsed, o)).collect()
    } else {
        let mut outs = Vec::with_capacity(tool_calls.len());
        for tc in tool_calls {
            if token.is_cancelled() {
                return Err("已停止生成".into());
            }
            let started = std::time::Instant::now();
            outs.push((started.elapsed(), execute_one_tool(app, state_arc, conv_id, tc, settings, token).await?));
        }
        outs
    };

    let mut results: Vec<ToolResult> = Vec::with_capacity(tool_calls.len());
    for (tc, (elapsed, outcome)) in tool_calls.iter().zip(outcomes) {
        log::info!(
            "[tool] 会话 {} 调用 {} 完成，耗时 {:?}，产物 {} 个",
            conv_id,
            tc.name,
            elapsed,
            outcome.artifacts.len()
        );
        // 产物实时推送到前端
        for art in &outcome.artifacts {
            emit_artifact(app, conv_id, art);
        }
        acc.artifacts.extend(outcome.artifacts.clone());
        // 搜索结果并入累计：持久化到消息的 search_results，任务结束后仍可展示来源卡片
        acc.search_items.extend(outcome.search_items.clone());
        // 工具步骤进入执行时间线：按发生顺序持久化并实时推送到前端
        let step = tool_step(tc, elapsed.as_millis() as u64, &outcome.search_items);
        emit(
            app,
            conv_id,
            "tool_step",
            serde_json::to_string(&step).ok().as_deref(),
        );
        acc.steps.push(step);
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

/// 由工具调用构造时间线步骤：解析参数生成人类可读摘要
fn tool_step(tc: &ToolCall, duration_ms: u64, search_items: &[SearchItem]) -> ToolStep {
    let args: serde_json::Value = serde_json::from_str(&tc.arguments).unwrap_or_default();
    let s = |k: &str| args[k].as_str().unwrap_or("").trim().to_string();
    let trunc = |mut t: String, n: usize| {
        if t.chars().count() > n {
            t = t.chars().take(n).collect::<String>() + "…";
        }
        t
    };
    let base = std::path::Path::new(&s("path"))
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("")
        .to_string();

    let (kind, label) = match tc.name.as_str() {
        tools::TOOL_WEB_SEARCH => {
            let q = trunc(s("query"), 40);
            (
                "search",
                format!(
                    "联网搜索 “{q}”{}",
                    if search_items.is_empty() {
                        String::new()
                    } else {
                        format!(" · {} 条结果", search_items.len())
                    }
                ),
            )
        }
        tools::TOOL_GENERATE_IMAGE => ("image", format!("生成图片 “{}”", trunc(s("prompt"), 40))),
        tools::TOOL_READ_FILE => ("tool", format!("读取文件 {base}")),
        tools::TOOL_WRITE_FILE => ("tool", format!("写入文件 {base}")),
        tools::TOOL_EDIT_FILE => ("tool", format!("编辑文件 {base}")),
        tools::TOOL_DELETE_FILE => ("tool", format!("删除文件 {base}")),
        tools::TOOL_LIST_FILES => ("tool", format!("列出目录 {}", trunc(s("path"), 40))),
        tools::TOOL_GLOB => ("tool", format!("匹配文件 {}", trunc(s("pattern"), 40))),
        tools::TOOL_GREP => ("tool", format!("搜索内容 “{}”", trunc(s("pattern"), 40))),
        tools::TOOL_BASH => ("tool", format!("执行命令 {}", trunc(s("cmd"), 60))),
        _ => ("tool", tc.name.clone()),
    };
    ToolStep {
        kind: kind.into(),
        label,
        tool: tc.name.clone(),
        duration_ms,
        items: if tc.name == tools::TOOL_WEB_SEARCH {
            search_items.to_vec()
        } else {
            Vec::new()
        },
    }
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
/// 约束：仅生成模式、内容含承诺词、内容较短（长回答不打断）、
/// 且本轮尚未成功调用过生成工具（tool_already_called 为 false）——工具已调用时
/// "正在生成中"是真实陈述，不打断。
fn promise_reminder_hint(content: &str, mode: &str, tool_already_called: bool) -> Option<String> {
    let c = content.trim();
    if c.chars().count() > 200 {
        return None;
    }
    if tool_already_called {
        return None;
    }
    const PROMISE_WORDS: [&str; 6] = ["正在生成", "请稍等", "请稍候", "马上", "为您生成", "开始生成"];
    if !PROMISE_WORDS.iter().any(|w| c.contains(w)) {
        return None;
    }
    let tool_hint = match mode {
        MODE_CHAT => format!(
            "请立即调用 {} 工具并提供参数（图片描述 prompt）",
            tools::TOOL_GENERATE_IMAGE
        ),
        MODE_AGENT => format!(
            "请立即调用相应工具并提供参数（如 {} 的 prompt、或文件操作参数）",
            tools::TOOL_GENERATE_IMAGE
        ),
        _ => return None,
    };
    Some(format!(
        "你尚未真正调用工具，只说了一句承诺话术。{tool_hint}，不要只说「正在生成」之类的文字。"
    ))
}

/// 已知工具名集合（文本兜底识别的合法范围）
const KNOWN_TOOL_NAMES: [&str; 10] = [
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
            // chat 与 agent 模式均具备图片生成能力
            if mode == MODE_CHAT || mode == MODE_AGENT {
                return Some((tools::TOOL_GENERATE_IMAGE.to_string(), v.clone()));
            }
        }
    }
    // ---- 文件工具（Agent）----
    if mode == MODE_AGENT {
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

/// 单条消息的 token 估算（与 db::context_usage 口径一致）：
/// 正文 + 思考 + 工具调用/结果 + 附件（图片固定值，文档按字节）
fn message_row_tokens(r: &DbMessageRow) -> u64 {
    let mut t = crate::db::estimate_tokens(&r.content)
        + crate::db::estimate_tokens(&r.reasoning)
        + crate::db::estimate_tokens(&r.tool_calls)
        + crate::db::estimate_tokens(&r.tool_results);
    if let Ok(atts) = serde_json::from_str::<Vec<crate::models::Attachment>>(&r.attachments) {
        for a in atts {
            if a.kind == "image" {
                t += crate::models::CONTEXT_IMAGE_TOKENS;
            } else {
                t += ((a.size.max(0) as u64) / 4).min(20_000);
            }
        }
    }
    t
}

/// 上下文自动压缩：
/// 当估算用量达到阈值（默认 60%）且存在尚未摘要的早期消息时，
/// 调用对话模型将「已有摘要 + 早期消息」浓缩为一段新摘要并持久化，
/// 之后该部分消息不再直接发送，只发送摘要与保留窗口内的近期消息
/// （窗口按 token 预算从最新往回划定）。
/// 返回压缩后的 (summary, summarized_until)；未发生压缩返回 None。
async fn try_compress_history(
    state: &AppState,
    conv: &Conversation,
    provider: &ProviderConfig,
    model: &ModelConfig,
    token: &CancelToken,
) -> Option<(String, i64)> {
    use crate::models::{CONTEXT_COMPRESS_THRESHOLD, CONTEXT_KEEP_BUDGET_RATIO, CONTEXT_KEEP_LAST_MSGS};
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
    // 仅考虑尚未摘要的消息
    let unsummarized: Vec<DbMessageRow> = rows
        .into_iter()
        .filter(|r| r.id > conv.summarized_until)
        .collect();
    if unsummarized.len() <= CONTEXT_KEEP_LAST_MSGS {
        return None;
    }
    // 保留窗口按 token 预算而非固定条数：从最新消息往回累计，达到预算
    // （总量 × CONTEXT_KEEP_BUDGET_RATIO）后停止，更早的消息进入摘要。
    // Agent 单条消息可含大量工具结果（上万 token），固定条数保留会导致
    // "已压缩但仍超限"；至少保留最近 CONTEXT_KEEP_LAST_MSGS 条保证
    // 最近的上下文连贯
    let total = state
        .db
        .get_settings()
        .chat_context_total(conv);
    let budget = (total as f64 * CONTEXT_KEEP_BUDGET_RATIO) as u64;
    let mut used_budget: u64 = 0;
    let mut keep_from = unsummarized.len(); // 保留窗口起点（含）
    for (i, r) in unsummarized.iter().enumerate().rev() {
        used_budget += message_row_tokens(r);
        keep_from = i;
        if used_budget >= budget {
            break;
        }
    }
    // 至少保留最近 CONTEXT_KEEP_LAST_MSGS 条
    keep_from = keep_from.min(unsummarized.len() - CONTEXT_KEEP_LAST_MSGS);
    if keep_from == 0 {
        return None; // 全部消息都在保留窗口内，无需压缩
    }
    let cutoff_id = unsummarized[keep_from].id;
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
                // tool_result 会导致 OpenAI 摘要接口返回 400
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
    let summary = openai::summarize(state, provider, model, &msgs).await;
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

    /// 文本形式工具调用解析：JSON/代码块/模式约束
    #[test]
    fn textual_tool_call_parsing() {
        // 纯 JSON → generate_image（Chat 模式，含图片生成）
        let r = parse_textual_tool_call(r#"{"prompt": "一只猫"}"#, MODE_CHAT);
        assert!(r.is_some());
        let (name, args) = r.unwrap();
        assert_eq!(name, tools::TOOL_GENERATE_IMAGE);
        assert_eq!(args["prompt"], "一只猫");

        // 代码块包裹同样识别
        let r = parse_textual_tool_call("```json\n{\"prompt\": \"狗\"}\n```", MODE_CHAT);
        assert!(r.is_some());

        // 文字 + 代码块 JSON（模型把工具参数写进正文的典型场景）
        let reply = "### 总\n已为您构思并生成一张图片…\n系统正在根据以下参数为您绘制图像：\n```json\n{\"prompt\": \"一只猫\"}\n```";
        let r = parse_textual_tool_call(reply, MODE_CHAT);
        assert!(r.is_some());
        let (name, args) = r.unwrap();
        assert_eq!(name, tools::TOOL_GENERATE_IMAGE);
        assert_eq!(args["prompt"], "一只猫");

        // 文字中直接内嵌 JSON（无代码块）同样识别
        let reply = "好的，正在为您生成：{\"prompt\": \"一只狗\", \"image_size\": \"1024x1024\"}";
        let r = parse_textual_tool_call(reply, MODE_CHAT);
        assert!(r.is_some());
        assert_eq!(r.unwrap().1["image_size"], "1024x1024");

        // 正文包含多个 JSON 片段时优先取代码块
        let reply = "参考 {\"a\": 1}，生成：```json\n{\"prompt\": \"猫\"}\n```";
        let r = parse_textual_tool_call(reply, MODE_CHAT);
        assert!(r.is_some());
        assert_eq!(r.unwrap().1["prompt"], "猫");

        // 位置信息：剥离展示内容中的 JSON 参数用
        let reply = "为您生成了一张图片。{\"prompt\": \"猫\"}";
        let r = find_textual_tool_call(reply, MODE_CHAT);
        assert!(r.is_some());
        let (_, _, start, end) = r.unwrap();
        assert_eq!(&reply[start..end], "{\"prompt\": \"猫\"}");
        // 代码块整体剥离（含 ``` 标记）
        let reply = "为您生成：\n```json\n{\"prompt\": \"狗\"}\n```";
        let r = find_textual_tool_call(reply, MODE_CHAT);
        assert!(r.is_some());
        let (_, _, start, end) = r.unwrap();
        assert_eq!(&reply[start..end], "```json\n{\"prompt\": \"狗\"}\n```");

        // 包装格式（模型模拟 tool_call 结构：name + arguments）
        let r = parse_textual_tool_call(
            r#"{"name": "generate_image", "arguments": {"prompt": "一只猫"}}"#,
            MODE_CHAT,
        );
        assert!(r.is_some());
        let (name, args) = r.unwrap();
        assert_eq!(name, tools::TOOL_GENERATE_IMAGE);
        assert_eq!(args["prompt"], "一只猫");

        // arguments 为 JSON 字符串
        let r = parse_textual_tool_call(
            r#"{"name": "generate_image", "arguments": "{\"prompt\": \"狗\"}"}"#,
            MODE_CHAT,
        );
        assert!(r.is_some());
        assert_eq!(r.unwrap().1["prompt"], "狗");

        // 未知 name → 不识别
        assert!(
            parse_textual_tool_call(
                r#"{"name": "hack", "arguments": {"prompt": "x"}}"#,
                MODE_CHAT
            )
            .is_none()
        );

        // 普通文本 / 无 prompt → 不识别
        assert!(parse_textual_tool_call("你好，有什么可以帮你", MODE_CHAT).is_none());
        assert!(parse_textual_tool_call(r#"{"a": 1}"#, MODE_CHAT).is_none());

        // Agent 模式同样识别 generate_image
        let r = parse_textual_tool_call(r#"{"prompt": "一只猫"}"#, MODE_AGENT);
        assert!(r.is_some());
        assert_eq!(r.unwrap().0, tools::TOOL_GENERATE_IMAGE);

        // 文件工具（Agent 模式）
        let r = parse_textual_tool_call(r#"{"path": "a.txt"}"#, MODE_AGENT);
        assert_eq!(r.unwrap().0, tools::TOOL_READ_FILE);
        let r = parse_textual_tool_call(r#"{"path": "a.txt", "content": "hi"}"#, MODE_AGENT);
        assert_eq!(r.unwrap().0, tools::TOOL_WRITE_FILE);
        let r = parse_textual_tool_call(r#"{"path": "a.txt", "old_string": "x", "new_string": "y"}"#, MODE_AGENT);
        assert_eq!(r.unwrap().0, tools::TOOL_EDIT_FILE);
        let r = parse_textual_tool_call(r#"{"command": "dir"}"#, MODE_AGENT);
        assert_eq!(r.unwrap().0, tools::TOOL_BASH);
        let r = parse_textual_tool_call(r#"{"pattern": "hello"}"#, MODE_AGENT);
        assert_eq!(r.unwrap().0, tools::TOOL_GREP);
        let r = parse_textual_tool_call(r#"{"dir": "src"}"#, MODE_AGENT);
        assert_eq!(r.unwrap().0, tools::TOOL_LIST_FILES);

        // 文件工具 JSON 在 Chat 模式不识别
        assert!(parse_textual_tool_call(r#"{"path": "a.txt"}"#, MODE_CHAT).is_none());
        assert!(parse_textual_tool_call(r#"{"command": "dir"}"#, MODE_CHAT).is_none());

        // 承诺话术纠正
        let h = promise_reminder_hint("正在为您生成中式古风庭院图片，请稍等。", MODE_CHAT, false);
        assert!(h.is_some());
        assert!(h.unwrap().contains("generate_image"));
        // 无承诺词 / 长回答 → 不打断
        assert!(promise_reminder_hint("好的，我来分析一下这个问题。", MODE_CHAT, false).is_none());
        assert!(promise_reminder_hint("正在生成".repeat(120).as_str(), MODE_CHAT, false).is_none());
        // 已成功调用过生成工具（图片已生成）→ 不再回喂"未调用工具"提示，
        // 否则会形成"回喂 → 模型纠结格式 → 再输出 JSON → 重复提交任务"的循环
        assert!(promise_reminder_hint("图片正在生成，请稍等。", MODE_CHAT, true).is_none());
        assert!(promise_reminder_hint("正在为您生成图片，请稍等。", MODE_AGENT, true).is_none());
    }
}
