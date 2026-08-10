pub mod anthropic;
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

#[derive(Clone, Debug)]
pub enum OutMsg {
    User { content: String },
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

fn build_outgoing(state: &AppState, conv_id: i64, include_tools: bool) -> Vec<OutMsg> {
    let mut out = Vec::new();
    let rows = match state.db.list_messages_full(conv_id) {
        Ok(r) => r,
        Err(_) => return out,
    };
    for row in rows {
        match row.role.as_str() {
            "user" => out.push(OutMsg::User {
                content: row.content,
            }),
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
            persist(&state, &conv, &acc);
            emit(&app, conv.id, "done", None);
        }
        Err(msg) => {
            if !acc.is_empty() {
                persist(&state, &conv, &acc);
            }
            emit(&app, conv.id, "error", Some(&msg));
        }
    }
}

fn persist(state: &AppState, conv: &Conversation, acc: &Accum) {
    let tool_calls_json = serde_json::to_string(&acc.tool_calls).unwrap_or_else(|_| "[]".into());
    let tool_results_json =
        serde_json::to_string(&acc.tool_results).unwrap_or_else(|_| "[]".into());
    let _ = state.db.insert_message(
        conv.id,
        "assistant",
        &acc.content,
        &acc.reasoning,
        &tool_calls_json,
        &tool_results_json,
        &acc.search_items,
        &acc.artifacts,
    );
    state.db.touch(conv.id);
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
    let tools = tools::tools_for_mode(&conv.mode, conv.web_search);
    let mut cur = build_outgoing(state, conv.id, !tools.is_empty());

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

        let mut turn = anthropic::run(app, state, conv, &settings.deepseek.api_key, &cur, &tools, token).await?;

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
