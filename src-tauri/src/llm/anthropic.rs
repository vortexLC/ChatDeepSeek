use futures_util::StreamExt;
use tauri::AppHandle;

use crate::commands::{AppState, emit};
use crate::llm::{CancelToken, ImageBlock, OutMsg, TurnResult};
use crate::models::*;

const MAX_TOKENS: u32 = 16384;

pub async fn run(
    app: &AppHandle,
    state: &AppState,
    provider: &ProviderConfig,
    model: &ModelConfig,
    conv: &Conversation,
    msgs: &[OutMsg],
    tools: &[serde_json::Value],
    token: &CancelToken,
) -> Result<TurnResult, String> {
    let base = provider.api_base.trim_end_matches('/');
    let url = format!("{base}/v1/messages");

    let body = build_body(model, conv, msgs, tools, true);
    // 初始请求（建立流式连接）加超时：服务端挂起时避免任务永久卡死、
    // 停止按钮失效（此时还未进入可取消的流式循环）
    let mut resp = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        send_request(state, &url, &provider.api_key, &body),
    )
    .await
    .map_err(|_| "请求超时（60 秒），请检查网络或服务商状态".to_string())??;

    if resp.status() == 400 {
        // 极少数兼容服务不接受思考模式参数，收到 400 时去掉思考参数重试一次
        // （先消费响应体再重试，避免连接复用被破坏）
        let _ = resp.text().await;
        let body2 = build_body(model, conv, msgs, tools, false);
        resp = tokio::time::timeout(
            std::time::Duration::from_secs(60),
            send_request(state, &url, &provider.api_key, &body2),
        )
        .await
        .map_err(|_| "请求超时（60 秒），请检查网络或服务商状态".to_string())??;
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
            _ = token.wait() => {
                // 用户停止：返回已生成的部分内容（reasoning/content），由上层持久化
                return Ok(TurnResult {
                    reasoning,
                    content,
                    tool_calls: Vec::new(),
                    stopped: true,
                    error: None,
                });
            }
            // 流式读取停滞超时：网络静默（如连接被无声掐断）时不能无限等待，
            // 120 秒无数据视为中断，保留已生成内容
            chunk = tokio::time::timeout(
                std::time::Duration::from_secs(120),
                stream.next(),
            ) => {
                let bytes = match chunk {
                    Ok(Some(Ok(b))) => b,
                    Ok(Some(Err(e))) => {
                        // 用户已停止时网络中断属正常：保留已生成内容，按"已停止"收尾
                        if token.is_cancelled() {
                            return Ok(TurnResult {
                                reasoning,
                                content,
                                tool_calls: Vec::new(),
                                stopped: true,
                                error: None,
                            });
                        }
                        // 流式中断：已生成的内容随错误一并返回，由上层先持久化
                        // 再报告错误，避免已流式输出的内容随网络错误一起丢失
                        return Ok(TurnResult {
                            reasoning,
                            content,
                            tool_calls: Vec::new(),
                            stopped: false,
                            error: Some(format!("网络错误: {e}")),
                        });
                    }
                    Ok(None) => break,
                    Err(_) => {
                        if token.is_cancelled() {
                            return Ok(TurnResult {
                                reasoning,
                                content,
                                tool_calls: Vec::new(),
                                stopped: true,
                                error: None,
                            });
                        }
                        // 流式停滞：内容随错误一并返回，由上层先持久化再报告错误
                        return Ok(TurnResult {
                            reasoning,
                            content,
                            tool_calls: Vec::new(),
                            stopped: false,
                            error: Some("流式响应停滞超时（120 秒无数据），已中断".into()),
                        });
                    }
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
        stopped: false,
        error: None,
    })
}

fn build_body(
    model: &ModelConfig,
    conv: &Conversation,
    msgs: &[OutMsg],
    tools: &[serde_json::Value],
    thinking: bool,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": model.name,
        "max_tokens": MAX_TOKENS,
        "messages": build_messages_json(msgs),
        "system": super::build_system_prompt(conv),
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
    if !tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tools.to_vec());
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

/// 调用对话模型浓缩早期对话为要点摘要（非流式，供上下文自动压缩使用）
pub async fn summarize(
    state: &AppState,
    provider: &ProviderConfig,
    model: &ModelConfig,
    msgs: &[OutMsg],
) -> Result<String, String> {
    let base = provider.api_base.trim_end_matches('/');
    let body = serde_json::json!({
        "model": model.name,
        "max_tokens": 600,
        "system": "你是对话摘要助手。请将给定的多轮对话内容浓缩为一段简洁的中文要点摘要：保留关键事实、用户需求、已得出的结论与尚未完成的事项；丢弃寒暄与无关细节；控制在 300 字以内。只输出摘要本身，不要任何前缀或解释。",
        "messages": build_messages_json(msgs),
        "stream": false,
    });
    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        send_request(state, &format!("{base}/v1/messages"), &provider.api_key, &body),
    )
    .await
    .map_err(|_| "摘要请求超时（60 秒），本次跳过上下文压缩".to_string())??;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(api_error(status.as_u16(), &text));
    }
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("摘要响应解析失败: {e}"))?;
    Ok(v["content"]
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .into_iter()
                .next()
                .map(|s| s.to_string())
        })
        .unwrap_or_default())
}

fn build_messages_json(msgs: &[OutMsg]) -> Vec<serde_json::Value> {
    let mut result: Vec<serde_json::Value> = Vec::new();
    let mut i = 0;
    while i < msgs.len() {
        match &msgs[i] {
            OutMsg::User {
                content,
                images,
                docs,
            } => {
                let mut blocks: Vec<serde_json::Value> = Vec::new();
                let full = if docs.is_empty() {
                    content.clone()
                } else {
                    format!("{docs}\n\n{content}")
                };
                if !full.is_empty() {
                    blocks.push(serde_json::json!({"type": "text", "text": full}));
                }
                for img in images {
                    blocks.push(build_image_block(img));
                }
                if blocks.is_empty() {
                    blocks.push(serde_json::json!({"type": "text", "text": ""}));
                }
                result.push(serde_json::json!({"role": "user", "content": blocks}));
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

/// 构建 Anthropic 图片块
pub fn build_image_block(img: &ImageBlock) -> serde_json::Value {
    serde_json::json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": img.media_type,
            "data": img.base64,
        }
    })
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
    log::error!("[api] Anthropic 协议请求失败 ({status}): {}", parsed);
    if status == 401 || status == 403 {
        msg.push_str("。请检查服务商 API Key 是否正确（可在 设置 → 服务商 中测试连接）");
    }
    msg
}
