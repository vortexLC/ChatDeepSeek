use futures_util::StreamExt;
use tauri::AppHandle;

use crate::commands::{AppState, emit};
use crate::llm::{CancelToken, ImageBlock, OutMsg, TurnResult};
use crate::models::*;

const MAX_TOKENS: u32 = 16384;

/// OpenAI 兼容协议（chat/completions 流式），支持 function calling 与图片（多模态）输入
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
    let url = format!("{base}/chat/completions");

    let body = build_body(model, conv, msgs, tools);
    // 初始请求（建立流式连接）加超时：服务端挂起时避免任务永久卡死、
    // 停止按钮失效（此时还未进入可取消的流式循环）
    let mut resp = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        send_request(state, &url, &provider.api_key, &body),
    )
    .await
    .map_err(|_| "请求超时（60 秒），请检查网络或服务商状态".to_string())??;

    if resp.status() == 400 {
        // 部分服务不支持工具参数（如纯视觉模型），收到 400 时去掉工具重试一次
        // （先消费响应体再重试，避免连接复用被破坏）
        let _ = resp.text().await;
        let body2 = build_body(model, conv, msgs, &[]);
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
    let mut tool_calls: std::collections::BTreeMap<usize, (String, String, String)> =
        std::collections::BTreeMap::new();
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
                    if line == "data: [DONE]" {
                        finished = true;
                        break;
                    }
                    if !line.starts_with("data:") || line.len() <= 5 {
                        continue;
                    }
                    let data = line.trim_start_matches("data:").trim();
                    let v: serde_json::Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if let Some(err) = v["error"]["message"].as_str() {
                        return Err(err.to_string());
                    }
                    let choices = &v["choices"];
                    let Some(delta) = choices[0]["delta"].as_object().or_else(|| {
                        // 部分服务使用 choices[0].message 非流式返回
                        choices[0]["message"].as_object()
                    }) else {
                        continue;
                    };
                    if let Some(t) = delta.get("content").and_then(|x| x.as_str()) {
                        if !t.is_empty() {
                            if !content_started {
                                content_started = true;
                                emit(app, conv.id, "status", Some("answering"));
                            }
                            emit(app, conv.id, "content_delta", Some(t));
                            content.push_str(t);
                        }
                    }
                    if let Some(t) = delta.get("reasoning_content").and_then(|x| x.as_str()) {
                        if !t.is_empty() {
                            emit(app, conv.id, "reasoning_delta", Some(t));
                            reasoning.push_str(t);
                        }
                    }
                    if let Some(calls) = delta.get("tool_calls").and_then(|x| x.as_array()) {
                        for call in calls {
                            let idx = call["index"].as_u64().unwrap_or(0) as usize;
                            let entry = tool_calls
                                .entry(idx)
                                .or_insert_with(|| (String::new(), String::new(), String::new()));
                            if let Some(id) = call["id"].as_str() {
                                entry.0 = id.to_string();
                            }
                            if let Some(name) = call["function"]["name"].as_str() {
                                entry.1 = name.to_string();
                            }
                            if let Some(args) = call["function"]["arguments"].as_str() {
                                entry.2.push_str(args);
                            }
                        }
                    }
                }
                if finished {
                    break;
                }
            }
        }
    }

    let tools = tool_calls
        .into_iter()
        .filter(|(_, (_, name, _))| !name.is_empty())
        .map(|(_, (id, name, arguments))| ToolCall {
            id: if id.is_empty() {
                format!("call_{}", crate::db::now_ms())
            } else {
                id
            },
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
) -> serde_json::Value {
    let mut messages = build_messages_json(msgs);
    // 注入系统提示词（与 Anthropic 协议一致）：模式说明、工具使用指引、回答规范。
    // 缺失时模型不知道当前模式与可用工具，会以"文本模型无法生成"为由拒绝图片/视频生成
    messages.insert(
        0,
        serde_json::json!({ "role": "system", "content": super::build_system_prompt(conv) }),
    );
    let mut body = serde_json::json!({
        "model": model.name,
        "max_tokens": MAX_TOKENS,
        "messages": messages,
        "stream": true,
    });
    if !tools.is_empty() {
        let openai_tools: Vec<serde_json::Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t["name"],
                        "description": t["description"],
                        "parameters": t["input_schema"],
                    }
                })
            })
            .collect();
        body["tools"] = serde_json::Value::Array(openai_tools);
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
        .bearer_auth(api_key)
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
    let mut msgs_json = build_messages_json(msgs);
    msgs_json.insert(
        0,
        serde_json::json!({
            "role": "system",
            "content": "你是对话摘要助手。请将给定的多轮对话内容浓缩为一段简洁的中文要点摘要：保留关键事实、用户需求、已得出的结论与尚未完成的事项；丢弃寒暄与无关细节；控制在 300 字以内。只输出摘要本身，不要任何前缀或解释。",
        }),
    );
    let body = serde_json::json!({
        "model": model.name,
        "max_tokens": 600,
        "messages": msgs_json,
        "stream": false,
    });
    let resp = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        send_request(state, &format!("{base}/chat/completions"), &provider.api_key, &body),
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
    Ok(v["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string())
}

fn build_messages_json(msgs: &[OutMsg]) -> Vec<serde_json::Value> {
    let mut result: Vec<serde_json::Value> = Vec::new();
    for m in msgs {
        match m {
            OutMsg::User {
                content,
                images,
                docs,
            } => {
                let full = if docs.is_empty() {
                    content.clone()
                } else {
                    format!("{docs}\n\n{content}")
                };
                if images.is_empty() {
                    result.push(serde_json::json!({"role": "user", "content": full}));
                } else {
                    let mut blocks: Vec<serde_json::Value> = Vec::new();
                    if !full.is_empty() {
                        blocks.push(serde_json::json!({"type": "text", "text": full}));
                    }
                    for img in images {
                        blocks.push(build_image_block(img));
                    }
                    result.push(serde_json::json!({"role": "user", "content": blocks}));
                }
            }
            OutMsg::Assistant {
                content,
                tool_calls,
            } => {
                let mut msg = serde_json::json!({
                    "role": "assistant",
                    "content": if content.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(content.clone()) },
                });
                if !tool_calls.is_empty() {
                    let calls: Vec<serde_json::Value> = tool_calls
                        .iter()
                        .map(|tc| {
                            serde_json::json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {"name": tc.name, "arguments": tc.arguments},
                            })
                        })
                        .collect();
                    msg["tool_calls"] = serde_json::Value::Array(calls);
                }
                result.push(msg);
            }
            OutMsg::Tool {
                tool_call_id,
                content,
            } => {
                result.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": tool_call_id,
                    "content": content,
                }));
            }
        }
    }
    result
}

/// 构建 OpenAI 图片块（data URL）
pub fn build_image_block(img: &ImageBlock) -> serde_json::Value {
    serde_json::json!({
        "type": "image_url",
        "image_url": {
            "url": format!("data:{};base64,{}", img.media_type, img.base64)
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
                .or_else(|| v["error"].as_str().map(|s| s.to_string()))
        })
        .unwrap_or_else(|| text.chars().take(300).collect());
    let mut msg = format!("API 错误 ({status}): {parsed}");
    if status == 401 || status == 403 {
        msg.push_str("。请检查服务商 API Key 是否正确（可在 设置 → 服务商 中测试连接）");
    }
    msg
}
