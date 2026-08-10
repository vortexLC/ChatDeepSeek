pub mod anthropic;
pub mod openai;
pub mod search;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
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
