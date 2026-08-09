use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use tauri::AppHandle;

use crate::commands::{AppState, DEEPSEEK_ANTHROPIC_BASE, emit};
use crate::llm::{CancelToken, OutMsg, TurnResult, search_tool_json_anthropic};
use crate::models::*;

const MAX_TOKENS: u32 = 16384;

pub async fn run(
    app: &AppHandle,
    state: &AppState,
    conv: &Conversation,
    api_key: &str,
    msgs: &[OutMsg],
    token: &CancelToken,
) -> Result<TurnResult, String> {
    let url = format!("{DEEPSEEK_ANTHROPIC_BASE}/v1/messages");

    let body = build_body(conv, msgs, true);
    let mut resp = send_request(state, &url, api_key, &body).await?;

    if resp.status() == 400 {
        // 极少数兼容服务不接受思考模式参数，收到 400 时去掉思考参数重试一次
        let body2 = build_body(conv, msgs, false);
        resp = send_request(state, &url, api_key, &body2).await?;
    }

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(api_error(status.as_u16(), &text));
    }

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut reasoning = String::new();
    let mut content = String::new();
    let mut content_started = false;
    let mut tool_blocks: Vec<(usize, String, String, String)> = Vec::new();
    let mut finished = false;

    loop {
        tokio::select! {
            _ = token.wait() => return Err("已停止生成".into()),
            chunk = stream.next() => {
                let bytes = match chunk {
                    Some(Ok(b)) => b,
                    Some(Err(e)) => return Err(format!("网络错误: {e}")),
                    None => break,
                };
                buf.extend_from_slice(&bytes);
                loop {
                    let Some(pos) = buf.iter().position(|&b| b == b'\n') else { break };
                    let line: Vec<u8> = buf.drain(..=pos).collect();
                    let line = String::from_utf8_lossy(&line);
                    let line = line.trim();
                    if !line.starts_with("data:") || line.len() <= 5 {
                        continue;
                    }
                    let data = line.trim_start_matches("data:").trim();
                    let v: serde_json::Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let etype = v["type"].as_str().unwrap_or("");
                    match etype {
                        "error" => {
                            let msg = v["error"]["message"]
                                .as_str()
                                .unwrap_or("Anthropic 流式错误")
                                .to_string();
                            return Err(msg);
                        }
                        "content_block_start" => {
                            let idx = v["index"].as_u64().unwrap_or(0) as usize;
                            let block = &v["content_block"];
                            if block["type"].as_str() == Some("tool_use") {
                                let id = block["id"].as_str().unwrap_or("").to_string();
                                let name = block["name"].as_str().unwrap_or("").to_string();
                                while tool_blocks.len() <= idx {
                                    tool_blocks.push((idx, String::new(), String::new(), String::new()));
                                }
                                tool_blocks[idx].1 = id;
                                tool_blocks[idx].2 = name;
                            }
                        }
                        "content_block_delta" => {
                            let idx = v["index"].as_u64().unwrap_or(0) as usize;
                            let delta = &v["delta"];
                            match delta["type"].as_str() {
                                Some("text_delta") => {
                                    if let Some(t) = delta["text"].as_str() {
                                        if !t.is_empty() {
                                            if !content_started {
                                                content_started = true;
                                                emit(app, conv.id, "status", Some("answering"));
                                            }
                                            emit(app, conv.id, "content_delta", Some(t));
                                            content.push_str(t);
                                        }
                                    }
                                }
                                Some("thinking_delta") => {
                                    if let Some(t) = delta["thinking"].as_str() {
                                        if !t.is_empty() {
                                            emit(app, conv.id, "reasoning_delta", Some(t));
                                            reasoning.push_str(t);
                                        }
                                    }
                                }
                                Some("input_json_delta") => {
                                    if let Some(t) = delta["partial_json"].as_str() {
                                        while tool_blocks.len() <= idx {
                                            tool_blocks.push((idx, String::new(), String::new(), String::new()));
                                        }
                                        tool_blocks[idx].3.push_str(t);
                                    }
                                }
                                _ => {}
                            }
                        }
                        "message_stop" => {
                            finished = true;
                        }
                        _ => {}
                    }
                }
                if finished {
                    break;
                }
            }
        }
    }

    let tools = tool_blocks
        .into_iter()
        .filter(|t| !t.2.is_empty())
        .map(|(_, id, name, arguments)| ToolCall {
            id,
            name,
            arguments,
        })
        .collect::<Vec<_>>();

    if tools.is_empty() && content.is_empty() && !finished {
        return Err("模型未返回内容".into());
    }

    Ok(TurnResult {
        reasoning,
        content,
        tool_calls: tools,
    })
}

fn build_body(conv: &Conversation, msgs: &[OutMsg], thinking: bool) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": conv.model,
        "max_tokens": MAX_TOKENS,
        "messages": build_messages_json(msgs),
        "system": build_system_prompt(conv),
        "stream": true,
    });
    if thinking {
        if conv.deep_think {
            // 开启思考模式，并按用户选择设置推理强度
            body["thinking"] = serde_json::json!({"type": "enabled"});
            if conv.effort != "none" {
                body["output_config"] = serde_json::json!({"effort": conv.effort});
            }
        } else {
            // 显式关闭思考模式（DeepSeek 思考模式默认开启，必须显式 disabled）
            body["thinking"] = serde_json::json!({"type": "disabled"});
        }
    }
    if conv.web_search {
        body["tools"] = serde_json::json!([search_tool_json_anthropic()]);
        body["tool_choice"] = serde_json::json!({"type": "auto"});
    }
    body
}

async fn send_request(
    state: &AppState,
    url: &str,
    api_key: &str,
    body: &serde_json::Value,
) -> Result<reqwest::Response, String> {
    state
        .client
        .post(url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))
}

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

fn build_system_prompt(conv: &Conversation) -> String {
    let date = today_cn();
    if conv.web_search {
        format!(
            "你是 ChatDeepSeek 智能助手，基于 DeepSeek 大模型构建。今天是 {date}。\n\
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
            "你是 ChatDeepSeek 智能助手，基于 DeepSeek 大模型构建。今天是 {date}。\n\
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
    }
}

fn build_messages_json(msgs: &[OutMsg]) -> Vec<serde_json::Value> {
    let mut result: Vec<serde_json::Value> = Vec::new();
    let mut i = 0;
    while i < msgs.len() {
        match &msgs[i] {
            OutMsg::User { content } => {
                result.push(serde_json::json!({"role": "user", "content": content}));
                i += 1;
            }
            OutMsg::Assistant {
                content,
                tool_calls,
                ..
            } => {
                let mut blocks: Vec<serde_json::Value> = Vec::new();
                if !content.is_empty() {
                    blocks.push(serde_json::json!({"type": "text", "text": content}));
                }
                for tc in tool_calls {
                    let input: serde_json::Value =
                        serde_json::from_str(&tc.arguments).unwrap_or(serde_json::json!({}));
                    blocks.push(serde_json::json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.name,
                        "input": input,
                    }));
                }
                result.push(serde_json::json!({"role": "assistant", "content": blocks}));
                i += 1;
            }
            OutMsg::Tool {
                tool_call_id,
                content,
            } => {
                // 连续的 tool_result 合并为同一条 user 消息，保证紧跟对应的 tool_use
                let mut blocks = vec![serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": tool_call_id,
                    "content": content,
                })];
                i += 1;
                while i < msgs.len() {
                    if let OutMsg::Tool {
                        tool_call_id: id2,
                        content: c2,
                    } = &msgs[i]
                    {
                        blocks.push(serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": id2,
                            "content": c2,
                        }));
                        i += 1;
                    } else {
                        break;
                    }
                }
                result.push(serde_json::json!({"role": "user", "content": blocks}));
            }
        }
    }
    result
}

pub fn api_error(status: u16, text: &str) -> String {
    let parsed = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| {
            v["error"]["message"]
                .as_str()
                .map(|s| s.to_string())
                .or_else(|| v["message"].as_str().map(|s| s.to_string()))
        })
        .unwrap_or_else(|| text.chars().take(300).collect());
    let mut msg = format!("API 错误 ({status}): {parsed}");
    if status == 401 || status == 403 {
        msg.push_str("。请检查 DeepSeek API Key 是否正确（可在设置面板点击「测试连接」验证）");
    }
    msg
}
