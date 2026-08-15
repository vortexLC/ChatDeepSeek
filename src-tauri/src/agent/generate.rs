use serde_json::json;
use tauri::AppHandle;

use crate::commands::{AppState, emit};
use crate::llm::CancelToken;
use crate::models::*;

/// 生成图片：调用所选图片生成模型（OpenAI 兼容 images/generations），
/// 下载保存到会话 images/ 目录
pub async fn generate_image(
    app: &AppHandle,
    state: &AppState,
    settings: &AppSettings,
    session_id: i64,
    arguments: &str,
    token: &CancelToken,
) -> Result<(String, Vec<Artifact>), String> {
    let Some((provider, model)) = settings.resolve_image_model() else {
        return Err(
            "未配置图片生成模型：请在 设置 → 服务商 中添加图片生成类型的模型，并在 设置 → 模型选择 中为「图片生成模型」选择已添加的模型"
                .into(),
        );
    };
    if provider.api_key.trim().is_empty() {
        return Err(format!(
            "服务商「{}」未配置 API Key，请在 设置 → 服务商 中填写",
            provider.name
        ));
    }
    let args: serde_json::Value =
        serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let prompt = args["prompt"].as_str().unwrap_or("").trim().to_string();
    if prompt.is_empty() {
        return Err("generate_image 缺少 prompt 参数".into());
    }
    let image_size = args["image_size"].as_str().unwrap_or("1024x1024");
    let base = provider.api_base.trim_end_matches('/');
    // 阿里云百炼（DashScope，*.aliyuncs.com/compatible-mode/v1）的
    // size 参数使用「宽*高」星号格式（如 1024*1024），与 OpenAI 的 1024x1024
    // 不同，需转换，否则接口报 size 参数非法
    let is_dashscope = base.to_lowercase().contains("aliyuncs.com");
    let size_param = if is_dashscope {
        image_size.replace('x', "*")
    } else {
        image_size.to_string()
    };
    let negative_prompt = args["negative_prompt"]
        .as_str()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());

    emit(app, session_id, "status", Some("generating"));
    log::info!(
        "[gen] 会话 {} 开始生成图片（模型: {}, 尺寸: {}, 服务商: {}）",
        session_id,
        model.name,
        size_param,
        provider.name
    );

    // 并发控制：Agent 并行执行多个 generate_image 时，同一时刻最多 2 个
    // 请求在途，其余排队等待，避免触发服务商限流（429 Too Many Requests）
    static IMAGE_SEMAPHORE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let queued_at = std::time::Instant::now();
    let _permit = IMAGE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| format!("获取图片生成并发许可失败: {e}"))?;
    let waited = queued_at.elapsed();
    if waited.as_secs() >= 1 {
        log::info!(
            "[gen] 会话 {} 排队等待 {:?} 后获得生成槽位",
            session_id,
            waited
        );
    }

    // qwen-image 系列（阿里云百炼）：原生多模态生成接口（同步），
    // OpenAI 兼容端点 /images/generations 对其返回 404
    let is_qwen_image = is_dashscope && model.name.to_lowercase().contains("qwen-image");
    let images: Vec<Vec<u8>> = if is_qwen_image {
        log::info!("[gen] 会话 {} 使用 qwen-image 原生多模态接口", session_id);
        fetch_images_qwen(state, provider, model, &prompt, &size_param, negative_prompt.as_deref(), token).await?
    } else {
        fetch_images_compatible(state, provider, model, &prompt, &size_param, negative_prompt.as_deref(), token).await?
    };

    let dir = state.db.session_images_dir(session_id);
    let _ = std::fs::create_dir_all(&dir);
    // 文件名序号：跨调用全局递增。仅用 now_ms+i 时，并行 generate_image 或
    // 同一毫秒内的多次调用会生成同名文件相互覆盖
    static IMG_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut artifacts = Vec::new();
    let mut notes = Vec::new();
    for (i, bytes) in images.iter().enumerate() {
        let ext = if bytes.len() > 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
            "png"
        } else if bytes.len() > 3 && &bytes[..3] == b"\xff\xd8\xff" {
            "jpg"
        } else if bytes.len() > 4 && &bytes[..4] == b"RIFF" {
            "webp"
        } else {
            "jpg"
        };
        let seq = IMG_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let fname = format!("img_{}_{}_{seq}.{ext}", crate::db::now_ms(), i);
        std::fs::write(dir.join(&fname), bytes)
            .map_err(|e| format!("保存图片失败: {e}"))?;
        artifacts.push(Artifact {
            kind: "image".into(),
            name: fname.clone(),
            path: format!("images/{fname}"),
            size: bytes.len() as i64,
            note: prompt.clone(),
        });
        notes.push(format!("图片 {}: images/{fname}", i + 1));
    }
    log::info!(
        "[gen] 会话 {} 图片生成完成：{} 张（模型: {}）",
        session_id,
        artifacts.len(),
        model.name
    );
    Ok((
        format!("已生成 {} 张图片并保存到会话目录：{}", artifacts.len(), notes.join("；")),
        artifacts,
    ))
}

/// OpenAI 兼容模式图片生成：POST {base}/images/generations，
/// 解析 data[].b64_json / data[].url 并下载，返回图片字节列表。
/// 生成请求与下载均支持取消：用户停止时立即返回"已停止生成"，
/// 避免同步生成（最长 180s）期间停止按钮失效
async fn fetch_images_compatible(
    state: &AppState,
    provider: &ProviderConfig,
    model: &ModelConfig,
    prompt: &str,
    size: &str,
    negative_prompt: Option<&str>,
    token: &CancelToken,
) -> Result<Vec<Vec<u8>>, String> {
    let base = provider.api_base.trim_end_matches('/');
    let mut body = json!({
        "model": model.name,
        "prompt": prompt,
        "size": size,
        "n": 1,
    });
    if let Some(n) = negative_prompt {
        body["negative_prompt"] = json!(n);
    }
    // 瞬时错误自动重试（指数退避 2s / 4s / 8s，最多 3 次）：
    // 网络层超时/连接失败、429 限流、5xx 服务端错误
    let mut attempt: u32 = 0;
    let resp = loop {
        let r = tokio::select! {
            _ = token.wait() => return Err("已停止生成".into()),
            r = state.client
                .post(format!("{base}/images/generations"))
                .bearer_auth(&provider.api_key)
                .timeout(std::time::Duration::from_secs(180))
                .json(&body)
                .send() => r,
        };
        let transient = match &r {
            Err(e) => e.is_timeout() || e.is_connect(),
            Ok(resp) => {
                let s = resp.status().as_u16();
                s == 429 || s >= 500
            }
        };
        if transient && attempt < 3 {
            attempt += 1;
            let delay = 2u64 << (attempt - 1);
            let desc = match &r {
                Err(e) => e.to_string(),
                Ok(resp) => format!("HTTP {}", resp.status()),
            };
            log::warn!(
                "[gen] 图片生成瞬时错误（{desc}），{delay} 秒后进行第 {attempt} 次重试"
            );
            tokio::select! {
                _ = token.wait() => return Err("已停止生成".into()),
                _ = tokio::time::sleep(std::time::Duration::from_secs(delay)) => {}
            }
            continue;
        }
        break r.map_err(|e| format!("图片生成请求失败: {e}"))?;
    };
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "图片生成 API 错误 ({status}): {}",
            text.chars().take(300).collect::<String>()
        ));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("图片生成响应解析失败: {e}"))?;
    let items: Vec<serde_json::Value> = v["data"].as_array().cloned().unwrap_or_default();
    if items.is_empty() {
        return Err(format!("图片生成未返回结果: {v}"));
    }
    let mut out = Vec::new();
    for item in items {
        if let Some(b64) = item["b64_json"].as_str() {
            let bytes = base64_decode(b64).map_err(|e| format!("图片 base64 解码失败: {e}"))?;
            out.push(bytes);
        } else {
            let url = item["url"]
                .as_str()
                .ok_or_else(|| format!("图片生成未返回 url: {item}"))?;
            let img_resp = tokio::select! {
                _ = token.wait() => return Err("已停止生成".into()),
                r = state.client
                    .get(url)
                    .timeout(std::time::Duration::from_secs(60))
                    .send() => r,
            }
            .map_err(|e| format!("下载图片失败: {e}"))?;
            let bytes = img_resp
                .bytes()
                .await
                .map_err(|e| format!("读取图片失败: {e}"))?
                .to_vec();
            out.push(bytes);
        }
    }
    Ok(out)
}

/// qwen-image 系列（阿里云百炼）原生多模态生成接口：
/// 优先使用 DashScope 异步任务模式——提交任务（X-DashScope-Async 头）后
/// 轮询任务状态，每次都是短请求。同步接口会握住连接直到整张图生成完
/// （实测 55~125 秒，随服务端负载波动），客户端超时与生成耗时赛跑，
/// 并行批次的队尾调用极易超时，且客户端放弃时服务端仍在消耗配额生成。
/// 异步模式从根上消除该问题。若服务端不支持异步（提交被拒 4xx 或
/// 响应无 task_id），回退同步长超时模式。
async fn fetch_images_qwen(
    state: &AppState,
    provider: &ProviderConfig,
    model: &ModelConfig,
    prompt: &str,
    size: &str,
    negative_prompt: Option<&str>,
    token: &CancelToken,
) -> Result<Vec<Vec<u8>>, String> {
    let native_base = native_dashscope_base(&provider.api_base);
    let url = format!("{native_base}/api/v1/services/aigc/multimodal-generation/generation");
    let mut parameters = json!({
        "size": size,
        "n": 1,
        "watermark": false,
    });
    if let Some(n) = negative_prompt {
        parameters["negative_prompt"] = json!(n);
    }
    let body = json!({
        "model": model.name,
        "input": {
            "messages": [{
                "role": "user",
                "content": [{"text": prompt}]
            }]
        },
        "parameters": parameters,
    });

    // 1. 提交生成任务（短请求）：瞬时错误（超时/连接失败/429/5xx）
    //    指数退避 2s / 4s / 8s 重试，最多 3 次
    let mut attempt: u32 = 0;
    let submitted = loop {
        let r = tokio::select! {
            _ = token.wait() => return Err("已停止生成".into()),
            r = state.client
                .post(&url)
                .header("X-DashScope-Async", "enable")
                .bearer_auth(&provider.api_key)
                .timeout(std::time::Duration::from_secs(30))
                .json(&body)
                .send() => r,
        };
        let transient = match &r {
            Err(e) => e.is_timeout() || e.is_connect(),
            Ok(resp) => {
                let s = resp.status().as_u16();
                s == 429 || s >= 500
            }
        };
        if transient && attempt < 3 {
            attempt += 1;
            let delay = 2u64 << (attempt - 1);
            log::warn!("[gen] qwen-image 任务提交瞬时错误，{delay} 秒后重试（第 {attempt} 次）");
            tokio::select! {
                _ = token.wait() => return Err("已停止生成".into()),
                _ = tokio::time::sleep(std::time::Duration::from_secs(delay)) => {}
            }
            continue;
        }
        break r.map_err(|e| format!("qwen-image 任务提交失败: {e}"))?;
    };

    if !submitted.status().is_success() {
        let status = submitted.status();
        let text = submitted.text().await.unwrap_or_default();
        // 4xx（除 429）重试无意义；但可能是服务不支持异步头，回退同步模式
        if status.as_u16() != 429 && status.as_u16() < 500 {
            log::warn!("[gen] qwen-image 异步提交被拒（{status}），回退同步模式");
            return qwen_sync_request(state, provider, &url, &body, token).await;
        }
        return Err(format!(
            "qwen-image API 错误 ({status}): {}",
            text.chars().take(300).collect::<String>()
        ));
    }
    let v: serde_json::Value = submitted
        .json()
        .await
        .map_err(|e| format!("qwen-image 响应解析失败: {e}"))?;

    // 服务端忽略异步头、直接返回生成结果 → 按同步结构解析
    let Some(task_id) = v["output"]["task_id"].as_str() else {
        return qwen_download_images(state, &v, token).await;
    };
    log::info!("[gen] qwen-image 异步任务已创建（task: {task_id}）");

    // 2. 轮询任务状态：2.5s 间隔，整体 5 分钟截止。轮询为短请求，
    //    连续 5 次瞬时失败才放弃（容忍网络抖动 / 查询限流）
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
    let mut poll_failures: u32 = 0;
    loop {
        tokio::select! {
            _ = token.wait() => return Err("已停止生成".into()),
            _ = tokio::time::sleep(std::time::Duration::from_millis(2500)) => {}
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("qwen-image 生成超时（任务 {task_id} 超过 5 分钟未完成）"));
        }
        let poll = tokio::select! {
            _ = token.wait() => return Err("已停止生成".into()),
            r = state.client
                .get(format!("{native_base}/api/v1/tasks/{task_id}"))
                .bearer_auth(&provider.api_key)
                .timeout(std::time::Duration::from_secs(20))
                .send() => r,
        };
        let resp = match poll {
            Ok(r) => r,
            Err(e) => {
                poll_failures += 1;
                if poll_failures >= 5 {
                    return Err(format!("qwen-image 任务状态查询失败: {e}"));
                }
                log::warn!("[gen] qwen-image 任务查询瞬时失败（{e}），继续轮询");
                continue;
            }
        };
        if !resp.status().is_success() {
            let s = resp.status();
            if s.as_u16() == 429 || s.as_u16() >= 500 {
                poll_failures += 1;
                if poll_failures >= 5 {
                    return Err(format!("qwen-image 任务状态查询持续失败（{s}）"));
                }
                continue;
            }
            let text = resp.text().await.unwrap_or_default();
            return Err(format!(
                "qwen-image 任务查询错误 ({s}): {}",
                text.chars().take(200).collect::<String>()
            ));
        }
        poll_failures = 0;
        let t: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("qwen-image 任务响应解析失败: {e}"))?;
        match t["output"]["task_status"].as_str().unwrap_or("") {
            "SUCCEEDED" => {
                log::info!("[gen] qwen-image 异步任务完成（task: {task_id}）");
                return qwen_download_images(state, &t, token).await;
            }
            "FAILED" | "CANCELED" | "UNKNOWN" => {
                let code = t["output"]["code"].as_str().unwrap_or("");
                let msg = t["output"]["message"].as_str().unwrap_or("无错误详情");
                return Err(format!("qwen-image 生成失败（{code}）: {msg}"));
            }
            _ => {} // PENDING / RUNNING → 继续等待
        }
    }
}

/// 同步模式请求（异步提交被拒时的回退路径）：单次长超时请求 + 解析下载
async fn qwen_sync_request(
    state: &AppState,
    provider: &ProviderConfig,
    url: &str,
    body: &serde_json::Value,
    token: &CancelToken,
) -> Result<Vec<Vec<u8>>, String> {
    let resp = tokio::select! {
        _ = token.wait() => return Err("已停止生成".into()),
        r = state.client
            .post(url)
            .bearer_auth(&provider.api_key)
            .timeout(std::time::Duration::from_secs(300))
            .json(body)
            .send() => r,
    }
    .map_err(|e| format!("qwen-image 生成请求失败: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "qwen-image API 错误 ({status}): {}",
            text.chars().take(300).collect::<String>()
        ));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("qwen-image 响应解析失败: {e}"))?;
    qwen_download_images(state, &v, token).await
}

/// 从 qwen-image 响应解析图片 URL 并下载。兼容两种结构：
/// - 同步接口 / 多模态任务成功：output.choices[].message.content[].image
/// - 部分异步任务（image-synthesis 风格）：output.results[].url
async fn qwen_download_images(
    state: &AppState,
    v: &serde_json::Value,
    token: &CancelToken,
) -> Result<Vec<Vec<u8>>, String> {
    let mut urls: Vec<String> = Vec::new();
    if let Some(choices) = v["output"]["choices"].as_array() {
        for c in choices {
            if let Some(content) = c["message"]["content"].as_array() {
                for block in content {
                    if let Some(u) = block["image"].as_str() {
                        urls.push(u.to_string());
                    }
                }
            }
        }
    }
    if urls.is_empty() {
        if let Some(results) = v["output"]["results"].as_array() {
            for r in results {
                if let Some(u) = r["url"].as_str() {
                    urls.push(u.to_string());
                }
            }
        }
    }
    if urls.is_empty() {
        return Err(format!(
            "qwen-image 未返回图片: {}",
            serde_json::to_string(&v["output"]).unwrap_or_default()
        ));
    }
    let mut out = Vec::new();
    for u in urls {
        let img_resp = tokio::select! {
            _ = token.wait() => return Err("已停止生成".into()),
            r = state.client
                .get(&u)
                .timeout(std::time::Duration::from_secs(60))
                .send() => r,
        }
        .map_err(|e| format!("下载图片失败: {e}"))?;
        let bytes = img_resp
            .bytes()
            .await
            .map_err(|e| format!("读取图片失败: {e}"))?
            .to_vec();
        out.push(bytes);
    }
    Ok(out)
}

/// 由兼容模式地址推导原生 DashScope 根地址
fn native_dashscope_base(compatible_base: &str) -> String {
    let trimmed = compatible_base.trim().trim_end_matches('/');
    if let Some(idx) = trimmed.find("/compatible-mode") {
        trimmed[..idx].to_string()
    } else {
        "https://dashscope.aliyuncs.com".into()
    }
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|e| e.to_string())
}