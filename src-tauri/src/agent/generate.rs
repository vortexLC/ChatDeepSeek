use serde_json::json;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

use crate::commands::{AppState, emit};
use crate::models::*;

/// 生成图片：调用所选图片生成模型（OpenAI 兼容 images/generations），
/// 下载保存到会话 images/ 目录
pub async fn generate_image(
    app: &AppHandle,
    state: &AppState,
    settings: &AppSettings,
    session_id: i64,
    arguments: &str,
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
    // 阿里云百炼（DashScope，*.aliyuncs.com/compatible-mode/v1）兼容模式的
    // size 参数使用「宽*高」星号格式（如 1024*1024），与 OpenAI 的 1024x1024
    // 不同，需转换，否则接口报 size 参数非法
    let is_dashscope = base.to_lowercase().contains("aliyuncs.com");
    let size_param = if is_dashscope {
        image_size.replace('x', "*")
    } else {
        image_size.to_string()
    };

    let mut body = json!({
        "model": model.name,
        "prompt": prompt,
        "size": size_param,
        "n": 1,
    });
    if let Some(n) = args["negative_prompt"].as_str() {
        if !n.is_empty() {
            body["negative_prompt"] = json!(n);
        }
    }

    emit(app, session_id, "status", Some("generating"));
    let resp = state
        .client
        .post(format!("{base}/images/generations"))
        .bearer_auth(&provider.api_key)
        .timeout(std::time::Duration::from_secs(180))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("图片生成请求失败: {e}"))?;
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

    let dir = state.db.session_images_dir(session_id);
    let _ = std::fs::create_dir_all(&dir);
    let mut artifacts = Vec::new();
    let mut notes = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let bytes: Vec<u8> = if let Some(b64) = item["b64_json"].as_str() {
            base64_decode(b64).map_err(|e| format!("图片 base64 解码失败: {e}"))?
        } else {
            let url = item["url"]
                .as_str()
                .ok_or_else(|| format!("图片生成未返回 url: {item}"))?;
            let img_resp = state
                .client
                .get(url)
                .timeout(std::time::Duration::from_secs(60))
                .send()
                .await
                .map_err(|e| format!("下载图片失败: {e}"))?;
            img_resp
                .bytes()
                .await
                .map_err(|e| format!("读取图片失败: {e}"))?
                .to_vec()
        };
        let ext = if bytes.len() > 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
            "png"
        } else if bytes.len() > 3 && &bytes[..3] == b"\xff\xd8\xff" {
            "jpg"
        } else if bytes.len() > 4 && &bytes[..4] == b"RIFF" {
            "webp"
        } else {
            "jpg"
        };
        let fname = format!("img_{}_{}.{ext}", crate::db::now_ms(), i);
        std::fs::write(dir.join(&fname), &bytes)
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
    Ok((
        format!("已生成 {} 张图片并保存到会话目录：{}", artifacts.len(), notes.join("；")),
        artifacts,
    ))
}

/// 生成视频：提交视频任务后立即返回，后台轮询状态，
/// 完成后自动保存到会话 videos/ 目录并推送视频完成事件（不阻塞会话其它操作）
pub async fn generate_video(
    app: &AppHandle,
    state: &std::sync::Arc<AppState>,
    settings: &AppSettings,
    session_id: i64,
    arguments: &str,
) -> Result<(String, Vec<Artifact>), String> {
    let args: serde_json::Value =
        serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let prompt = args["prompt"].as_str().unwrap_or("").trim().to_string();
    if prompt.is_empty() {
        return Err("generate_video 缺少 prompt 参数".into());
    }
    // 图生视频 / 参考生视频需要图片；文生视频不需要
    let mut image = args["image"].as_str().unwrap_or("").trim().to_string();
    // 支持使用本会话内已生成/已保存的图片（如 generate_image 产物 images/xxx.png）：
    // 本地相对路径读取并转 base64，使「先生图、再基于该图生视频」的衔接可用
    if !image.is_empty() {
        image = resolve_image_input(state, session_id, &image)?;
    }
    let mut extra_images: Vec<String> = args["images"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    for img in extra_images.iter_mut() {
        *img = resolve_image_input(state, session_id, img)?;
    }
    // 模式：优先显式 mode 参数；缺省时按是否传图推断（兼容旧调用）
    let mode = match args["mode"].as_str().unwrap_or("").trim().to_lowercase().as_str() {
        "image2video" | "i2v" => VIDEO_MODE_I2V,
        "reference2video" | "r2v" => VIDEO_MODE_R2V,
        "text2video" | "t2v" => VIDEO_MODE_T2V,
        _ => {
            if image.is_empty() && !extra_images.is_empty() {
                VIDEO_MODE_R2V
            } else if image.is_empty() {
                VIDEO_MODE_T2V
            } else {
                VIDEO_MODE_I2V
            }
        }
    };
    if mode != VIDEO_MODE_T2V && image.is_empty() {
        // 用户已在对话中上传图片时自动使用最近上传的图片附件
        image = latest_uploaded_image(state, session_id)?.ok_or(
            "图生视频/参考视频生成需要图片：请先在对话中上传一张图片，或在 image 参数中提供图片 URL/base64",
        )?;
    }
    let mode_cn = match mode {
        VIDEO_MODE_I2V => "图生",
        VIDEO_MODE_R2V => "参考生",
        _ => "文生",
    };
    let Some((provider, model)) = settings.resolve_video_model(mode) else {
        return Err(format!(
            "未配置{mode_cn}视频模型：请在 设置 → 服务商 中添加视频生成类型的模型，并在 设置 → 模型选择 中选择{mode_cn}视频模型{}",
            if mode == VIDEO_MODE_R2V { "（参考生视频需 r2v 模型，如阿里云百炼 wan2.7-r2v）" } else { "" }
        ));
    };
    if provider.api_key.trim().is_empty() {
        return Err(format!(
            "服务商「{}」未配置 API Key，请在 设置 → 服务商 中填写",
            provider.name
        ));
    }
    let image_size = normalize_image_size(args["image_size"].as_str());
    let video_api = resolve_video_api(provider, model);
    if mode == VIDEO_MODE_R2V && video_api != VIDEO_API_DASHSCOPE {
        return Err(format!(
            "当前视频模型「{}」使用硅基流动接口，不支持参考视频生成；请在 设置 → 服务商 中为阿里云百炼添加 r2v 模型（如 wan2.7-r2v），并在 设置 → 模型选择 中将其指定为参考生视频模型",
            model.name
        ));
    }

    match video_api {
        VIDEO_API_DASHSCOPE => {
            generate_video_dashscope(
                app, state, provider, model, session_id, &prompt, mode, &image, &extra_images, image_size,
            )
            .await
        }
        _ => {
            generate_video_siliconflow(app, state, provider, model, session_id, &prompt, &image, image_size).await
        }
    }
}

/// 从对话中取最近一次用户上传的图片附件，读为 data URL（供图生视频/参考生视频使用）
fn latest_uploaded_image(state: &AppState, session_id: i64) -> Result<Option<String>, String> {
    let messages = state.db.list_messages(session_id)?;
    for msg in messages.iter().rev() {
        if msg.role != "user" {
            continue;
        }
        for att in msg.attachments.iter().rev() {
            if att.kind != "image" {
                continue;
            }
            let path = state
                .db
                .session_abs_path(session_id, &att.path)
                .ok_or_else(|| format!("附件路径无效: {}", att.path))?;
            let bytes =
                std::fs::read(&path).map_err(|e| format!("读取上传图片失败（{}）: {e}", att.path))?;
            let mime = match att.mime.as_str() {
                "image/png" => "image/png",
                "image/gif" => "image/gif",
                "image/webp" => "image/webp",
                "image/bmp" => "image/bmp",
                _ => "image/jpeg",
            };
            use base64::Engine;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            return Ok(Some(format!("data:{mime};base64,{b64}")));
        }
    }
    Ok(None)
}

/// 解析图片输入为可直接发送给视频 API 的形式：
/// - http(s) URL / data URL 原样返回；
/// - 会话内本地相对路径（如 generate_image 产物 images/xxx.png）读取并转 base64 data URL。
fn resolve_image_input(state: &AppState, session_id: i64, input: &str) -> Result<String, String> {
    let t = input.trim();
    if t.is_empty() {
        return Ok(String::new());
    }
    if t.starts_with("http://") || t.starts_with("https://") || t.starts_with("data:") {
        return Ok(t.to_string());
    }
    // 本地相对路径：限制在会话目录内，读取并转 base64
    let p = state
        .db
        .session_abs_path(session_id, t)
        .ok_or_else(|| format!("图片路径无效（越界）: {t}"))?;
    if !p.is_file() {
        return Err(format!("图片文件不存在: {t}"));
    }
    let bytes = std::fs::read(&p).map_err(|e| format!("读取图片失败（{t}）: {e}"))?;
    if bytes.is_empty() {
        return Err(format!("图片文件为空: {t}"));
    }
    let mime = match p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => "image/jpeg",
    };
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{mime};base64,{b64}"))
}

/// 归一化视频画幅：仅 1280x720 / 720x1280 / 960x960 有效（SiliconFlow 枚举值，
/// DashScope 侧再映射为分辨率/宽高比）；其它 WxH 按宽高比就近取其一
fn normalize_image_size(raw: Option<&str>) -> &'static str {
    let s = raw.unwrap_or("").trim().to_lowercase();
    if let Some((w, h)) = s.split_once('x') {
        if let (Ok(w), Ok(h)) = (w.trim().parse::<f64>(), h.trim().parse::<f64>()) {
            if w > 0.0 && h > 0.0 {
                let ratio = w / h;
                if ratio > 1.3 {
                    return "1280x720";
                }
                if ratio < 0.77 {
                    return "720x1280";
                }
                return "960x960";
            }
        }
    }
    "1280x720"
}

/// 确定视频接口风格：auto 时按服务商地址 / 模型名推断
fn resolve_video_api<'a>(provider: &'a ProviderConfig, model: &'a ModelConfig) -> &'a str {
    if model.video_api != VIDEO_API_AUTO {
        return &model.video_api;
    }
    let base = provider.api_base.to_lowercase();
    let name = model.name.to_lowercase();
    if base.contains("dashscope") || name.contains("wanx") || name.contains("qwen-video") {
        VIDEO_API_DASHSCOPE
    } else {
        VIDEO_API_SILICONFLOW
    }
}

fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|e| e.to_string())
}

// ==================== 视频：硅基流动风格（/video/submit + /video/status） ====================

async fn generate_video_siliconflow(
    app: &AppHandle,
    state: &Arc<AppState>,
    provider: &ProviderConfig,
    model: &ModelConfig,
    session_id: i64,
    prompt: &str,
    image: &str,
    image_size: &str,
) -> Result<(String, Vec<Artifact>), String> {
    let base = provider.api_base.trim_end_matches('/');
    let api_key = provider.api_key.trim().to_string();
    let mut used_model = model.name.clone();
    let mut switched = false;

    emit(app, session_id, "status", Some("generating"));
    let v = match submit_video(&state.client, &base, &api_key, &used_model, prompt, image, image_size).await {
        Ok(v) => v,
        Err(e) => {
            if !is_model_not_exist(&e) {
                return Err(e);
            }
            // 模型不存在（可能已下架或未开通）：自动查询账户可用视频模型并重试一次
            match find_available_video_model(&state.client, &base, &api_key, &used_model, !image.is_empty()).await {
                Ok(Some(fallback)) if fallback != used_model => {
                    used_model = fallback;
                    match submit_video(&state.client, &base, &api_key, &used_model, prompt, image, image_size).await {
                        Ok(v) => {
                            switched = true;
                            v
                        }
                        Err(e2) => return Err(format!("{e}；自动切换模型 {used_model} 后仍失败: {e2}")),
                    }
                }
                _ => return Err(format!(
                    "{e}。请先在服务商平台开通可用的视频模型，或在 设置 → 服务商 中修改/添加视频模型"
                )),
            }
        }
    };
    let request_id = v["requestId"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_default();
    if request_id.is_empty() {
        return Err(format!("视频提交未返回 requestId: {v}"));
    }

    // ---------- 记录任务并后台轮询生成 ----------
    let job_id = state
        .db
        .create_job(session_id, "video", VIDEO_API_SILICONFLOW, &used_model, &request_id)?;
    if let Some(job) = state.db.get_job(session_id, job_id) {
        let payload = json!({
            "kind": "video_submitted",
            "conversation_id": session_id,
            "job": job,
        });
        let _ = app.emit("chat_event", payload);
    }
    spawn_video_poller(
        state,
        app,
        session_id,
        job_id,
        VIDEO_API_SILICONFLOW,
        &base,
        &api_key,
        &used_model,
        &request_id,
    );

    Ok((
        format!(
            "视频任务已提交（模型 {used_model}）{}，正在后台生成，预计需要几分钟；生成完成后会弹出提示，并将视频保存到会话目录",
            if switched { "（原模型不可用，已自动切换）" } else { "" }
        ),
        Vec::new(),
    ))
}

/// 提交视频生成请求，返回响应 JSON
async fn submit_video(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
    model: &str,
    prompt: &str,
    image: &str,
    image_size: &str,
) -> Result<serde_json::Value, String> {
    let mut body = json!({
        "model": model,
        "prompt": prompt,
        "image_size": image_size,
    });
    if !image.is_empty() {
        body["image"] = json!(image);
    }
    let resp = client
        .post(format!("{base}/video/submit"))
        .bearer_auth(api_key)
        .timeout(std::time::Duration::from_secs(30))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("视频生成提交失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "视频生成 API 错误 ({status}): {}",
            text.chars().take(200).collect::<String>()
        ));
    }
    serde_json::from_str(&text).map_err(|e| format!("视频提交响应解析失败: {e}"))
}

/// 判断错误是否为“模型不存在”（硅基流动 code 20012）
fn is_model_not_exist(err: &str) -> bool {
    err.contains("20012") || err.to_lowercase().contains("model does not exist")
}

/// 获取账户当前可用的视频模型 ID 列表
async fn fetch_sf_video_models(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
) -> Result<Vec<String>, String> {
    let resp = client
        .get(format!("{base}/models?type=video"))
        .bearer_auth(api_key)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("获取视频模型列表失败: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "获取视频模型列表失败 ({status}): {}",
            text.chars().take(200).collect::<String>()
        ));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("模型列表解析失败: {e}"))?;
    Ok(v["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["id"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default())
}

/// 查找替代模型：优先 Wan 系列，其次按 I2V/T2V 用途匹配
fn pick_fallback_model(models: &[String], is_i2v: bool) -> Option<String> {
    let kind = if is_i2v { "I2V" } else { "T2V" };
    let other = if is_i2v { "T2V" } else { "I2V" };
    let wan: Vec<&String> = models.iter().filter(|m| m.contains("Wan")).collect();
    let pool: Vec<&String> = if wan.is_empty() {
        models.iter().collect()
    } else {
        wan
    };
    pool.iter()
        .find(|m| m.contains(kind))
        .or_else(|| pool.iter().find(|m| !m.contains(other)))
        .or_else(|| pool.first())
        .map(|m| m.to_string())
}

/// 查询可用视频模型并选出替代模型（当前模型本身可用时返回当前模型）
async fn find_available_video_model(
    client: &reqwest::Client,
    base: &str,
    api_key: &str,
    current: &str,
    is_i2v: bool,
) -> Result<Option<String>, String> {
    let models = fetch_sf_video_models(client, base, api_key).await?;
    Ok(models
        .iter()
        .find(|m| m.as_str() == current)
        .map(|m| m.to_string())
        .or_else(|| pick_fallback_model(&models, is_i2v)))
}

/// 后台轮询视频状态并下载保存
async fn poll_video(
    state: &AppState,
    session_id: i64,
    base: &str,
    request_id: &str,
    _model: &str,
    api_key: &str,
    token: &crate::llm::CancelToken,
) -> Result<Artifact, String> {
    let status_body = json!({ "requestId": request_id });
    let mut attempts = 0u32;
    loop {
        tokio::select! {
            _ = token.wait() => return Err("任务已取消".into()),
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
        }
        attempts += 1;
        let sresp = state
            .client
            .post(format!("{base}/video/status"))
            .bearer_auth(&api_key)
            .timeout(std::time::Duration::from_secs(30))
            .json(&status_body)
            .send()
            .await
            .map_err(|e| format!("视频状态查询失败: {e}"))?;
        let st = sresp.status();
        if !st.is_success() {
            let text = sresp.text().await.unwrap_or_default();
            return Err(format!(
                "视频状态查询错误 ({st}): {}",
                text.chars().take(200).collect::<String>()
            ));
        }
        let sv: serde_json::Value = sresp
            .json()
            .await
            .map_err(|e| format!("视频状态解析失败: {e}"))?;
        let state_str = sv["status"].as_str().unwrap_or("").to_lowercase();
        if state_str.contains("fail") || state_str.contains("error") {
            return Err(format!("{}", sv["message"].as_str().unwrap_or("未知错误")));
        }
        if state_str.contains("succeed") || state_str.contains("success") {
            let url = sv["videos"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|x| x["url"].as_str())
                .unwrap_or("");
            if url.is_empty() {
                return Err(format!("视频生成完成但未返回下载地址: {sv}"));
            }
            let vresp = state
                .client
                .get(url)
                .timeout(std::time::Duration::from_secs(180))
                .send()
                .await
                .map_err(|e| format!("下载视频失败: {e}"))?;
            let bytes = vresp
                .bytes()
                .await
                .map_err(|e| format!("读取视频失败: {e}"))?
                .to_vec();
            let fname = format!("video_{}.mp4", crate::db::now_ms());
            let dir = state.db.session_videos_dir(session_id);
            let _ = std::fs::create_dir_all(&dir);
            std::fs::write(dir.join(&fname), &bytes).map_err(|e| format!("保存视频失败: {e}"))?;
            let artifact = Artifact {
                kind: "video".into(),
                name: fname.clone(),
                path: format!("videos/{fname}"),
                size: bytes.len() as i64,
                note: String::new(),
            };
            return Ok(artifact);
        }
        if attempts > 120 {
            return Err("视频生成超时（超过 10 分钟），请稍后重试".into());
        }
    }
}

// ==================== 视频：阿里云百炼（DashScope 原生异步接口） ====================

/// 由兼容模式地址推导原生 DashScope 根地址
fn native_dashscope_base(compatible_base: &str) -> String {
    let trimmed = compatible_base.trim().trim_end_matches('/');
    if let Some(idx) = trimmed.find("/compatible-mode") {
        trimmed[..idx].to_string()
    } else {
        "https://dashscope.aliyuncs.com".into()
    }
}

/// 提交 DashScope 视频任务；当请求含 media 字段且报 400（如 media 形状不被接受）时，
/// 使用备选 media 形状重试一次
async fn submit_dashscope_video(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    body: &serde_json::Value,
    alt_body: Option<&serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let send = |b: &serde_json::Value| {
        client
            .post(url)
            .bearer_auth(api_key)
            .header("X-DashScope-Async", "enable")
            .json(b)
    };
    let resp = send(body)
        .send()
        .await
        .map_err(|e| format!("阿里云视频生成提交失败: {e}"))?;
    let mut status = resp.status();
    let mut text = resp.text().await.unwrap_or_default();
    if !status.is_success()
        && status.as_u16() == 400
        && text.to_lowercase().contains("media")
        && alt_body.is_some()
    {
        let resp2 = send(alt_body.unwrap())
            .send()
            .await
            .map_err(|e| format!("阿里云视频生成提交失败: {e}"))?;
        status = resp2.status();
        text = resp2.text().await.unwrap_or_default();
    }
    if !status.is_success() {
        return Err(format!(
            "阿里云视频生成 API 错误 ({status}): {}",
            text.chars().take(300).collect::<String>()
        ));
    }
    serde_json::from_str(&text).map_err(|e| format!("阿里云视频提交响应解析失败: {e}"))
}

async fn generate_video_dashscope(
    app: &AppHandle,
    state: &Arc<AppState>,
    provider: &ProviderConfig,
    model: &ModelConfig,
    session_id: i64,
    prompt: &str,
    mode: &str,
    image: &str,
    extra_images: &[String],
    image_size: &str,
) -> Result<(String, Vec<Artifact>), String> {
    let native_base = native_dashscope_base(&provider.api_base);
    let api_key = provider.api_key.trim().to_string();

    // 按模式 + 模型家族构造请求体（用户直接填模型名即可使用）：
    // - wanx2.1 系列（旧 API）：t2v 用 size 数值格式；i2v 用 img_url
    // - wan2.2/2.5/2.6/2.7 系列（新 API）：i2v/r2v 用 media 数组
    // - qwen-video 系列：media 数组（{type:image, image_url}）
    // - wan2.6-r2v：reference_urls（仅公网 URL）+ size 数值格式
    let lower = model.name.to_lowercase();
    let is_wanx21 = lower.contains("wanx2.1");
    let is_wanx = lower.contains("wanx");
    let is_wan27 = lower.contains("wan2.7");
    let is_r2v_model = lower.contains("r2v");
    let is_qwen_video = lower.contains("qwen-video");
    let resolution = "720P";
    let ratio = match image_size {
        "720x1280" => "9:16",
        "960x960" => "1:1",
        _ => "16:9",
    };

    // 收集图片列表：image 优先，其次 extra_images（参考生视频可多张）
    let mut images: Vec<String> = Vec::new();
    if !image.is_empty() {
        images.push(image.to_string());
    }
    images.extend(extra_images.iter().cloned());

    // media 元素形状：qwen-video 用 image_url 风格，其它用 url 风格
    let media_item = |u: &str| -> serde_json::Value {
        if is_qwen_video {
            json!({ "type": "image", "image_url": u })
        } else {
            json!({ "type": "first_frame", "url": u })
        }
    };

    let build = |input: &mut serde_json::Value, items: &[serde_json::Value]| -> serde_json::Value {
        if !items.is_empty() {
            input["media"] = json!(items);
        }
        let mut parameters: serde_json::Value = match mode {
            VIDEO_MODE_I2V => json!({ "resolution": resolution }),
            VIDEO_MODE_R2V => json!({}),
            _ => {
                // 文生视频
                if is_wanx21 {
                    json!({ "size": image_size.replace('x', "*"), "watermark": false })
                } else if is_wan27 {
                    json!({ "resolution": resolution, "ratio": ratio })
                } else {
                    json!({ "resolution": resolution })
                }
            }
        };
        if mode == VIDEO_MODE_R2V && is_wan27 {
            parameters["ratio"] = json!(ratio);
            parameters["resolution"] = json!(resolution);
        }
        let mut body = json!({ "model": model.name, "input": input.clone() });
        if parameters.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
            body["parameters"] = parameters;
        }
        body
    };

    // 校验
    if mode == VIDEO_MODE_R2V && !is_r2v_model {
        return Err(format!(
            "模型「{}」不是参考生视频（r2v）模型；请在 设置 → 服务商 中为阿里云百炼添加 r2v 模型（如 wan2.7-r2v 或 wan2.6-r2v），并在 设置 → 模型选择 中将其指定为参考生视频模型",
            model.name
        ));
    }
    if mode == VIDEO_MODE_R2V && !is_wan27 && is_wanx {
        // wan2.6-r2v 等：reference_urls 仅支持公网 URL，不支持 base64
        if images.iter().any(|u| u.starts_with("data:")) {
            return Err(
                "该 r2v 模型的参考图仅支持公网可访问的 URL（不支持本地上传的图片）；请改用 wan2.7-r2v 模型，或在 image/images 参数中提供公网图片 URL"
                    .into(),
            );
        }
    }
    if mode != VIDEO_MODE_T2V && images.is_empty() {
        return Err("图生/参考生视频需要至少一张图片".into());
    }

    // 组装请求体（含备选 media 形状用于重试）
    let mut input = json!({ "prompt": prompt });
    let media_items: Vec<serde_json::Value> = match mode {
        VIDEO_MODE_I2V => {
            if is_wanx21 {
                // 旧接口：img_url 单图
                input["img_url"] = json!(images[0].clone());
                Vec::new()
            } else {
                vec![media_item(&images[0])]
            }
        }
        VIDEO_MODE_R2V => {
            if is_wan27 {
                images
                    .iter()
                    .map(|u| json!({ "type": "reference_image", "url": u }))
                    .collect()
            } else {
                // wan2.6-r2v：reference_urls 数组
                input["reference_urls"] = json!(images);
                Vec::new()
            }
        }
        _ => Vec::new(),
    };
    let body = build(&mut input, &media_items);

    // 备选形状：i2v 且非旧接口时，media 元素换另一种风格再试一次
    let mut alt_body: Option<serde_json::Value> = None;
    if mode == VIDEO_MODE_I2V && !is_wanx21 {
        let mut alt_input = json!({ "prompt": prompt });
        let alt_items = if is_qwen_video {
            vec![json!({ "type": "first_frame", "url": images[0].clone() })]
        } else {
            vec![json!({ "type": "image", "image_url": images[0].clone() })]
        };
        alt_body = Some(build(&mut alt_input, &alt_items));
    }

    emit(app, session_id, "status", Some("generating"));
    let submit_url = format!("{native_base}/api/v1/services/aigc/video-generation/video-synthesis");
    let v = submit_dashscope_video(&state.client, &submit_url, &api_key, &body, alt_body.as_ref()).await?;
    let task_id = v["output"]["task_id"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_default();
    if task_id.is_empty() {
        return Err(format!("阿里云视频提交未返回 task_id: {v}"));
    }

    // ---------- 记录任务并后台轮询生成 ----------
    let job_id = state
        .db
        .create_job(session_id, "video", VIDEO_API_DASHSCOPE, &model.name, &task_id)?;
    if let Some(job) = state.db.get_job(session_id, job_id) {
        let payload = json!({
            "kind": "video_submitted",
            "conversation_id": session_id,
            "job": job,
        });
        let _ = app.emit("chat_event", payload);
    }
    spawn_video_poller(
        state,
        app,
        session_id,
        job_id,
        VIDEO_API_DASHSCOPE,
        &native_base,
        &api_key,
        &model.name,
        &task_id,
    );

    Ok((
        format!(
            "视频任务已提交（模型 {}），正在后台生成，预计需要几分钟；生成完成后会弹出提示，并将视频保存到会话目录",
            model.name
        ),
        Vec::new(),
    ))
}

/// 启动视频后台轮询任务（视频提交后、以及应用重启后恢复任务时共用）。
/// 任务 key 为 `video-{session_id}-{job_id}`，同一会话可并发多个任务互不覆盖；
/// 若该任务已有存活轮询则直接跳过（幂等）。
pub(crate) fn spawn_video_poller(
    state: &Arc<AppState>,
    app: &AppHandle,
    session_id: i64,
    job_id: i64,
    api: &str,
    base: &str,
    api_key: &str,
    model: &str,
    request_id: &str,
) {
    let key = format!("video-{session_id}-{job_id}");
    let token = std::sync::Arc::new(crate::llm::CancelToken::new());
    if state.bg_tasks.lock().unwrap().insert(key.clone(), token.clone()).is_some() {
        return; // 已有存活轮询
    }
    let st = state.clone();
    let app2 = app.clone();
    let base2 = base.to_string();
    let api_key2 = api_key.to_string();
    let model2 = model.to_string();
    let request_id2 = request_id.to_string();
    let api2 = api.to_string();
    tokio::spawn(async move {
        // 任务退出（含 panic）时自动清理注册表，避免条目残留导致任务永久 pending
        let _guard = BgTaskGuard { state: st.clone(), key: key.clone() };
        let result = if api2 == VIDEO_API_DASHSCOPE {
            poll_video_dashscope(&st, session_id, &base2, &request_id2, &model2, &api_key2, &token).await
        } else {
            poll_video(&st, session_id, &base2, &request_id2, &model2, &api_key2, &token).await
        };
        match result {
            Ok(artifact) => {
                // 取消恰好落在下载/请求期间：不再写库/推送（防止状态回退与僵尸目录）
                if token.is_cancelled() || st.db.get_conversation(session_id).is_none() {
                    return;
                }
                let _ = st.db.finish_job(session_id, job_id, &artifact);
                let job = st.db.get_job(session_id, job_id);
                let submitted_at = job.as_ref().map(|j| j.submitted_at).unwrap_or_else(crate::db::now_ms);
                let finished_at = job.as_ref().map(|j| j.finished_at).unwrap_or_else(crate::db::now_ms);
                let msg = build_video_done_msg(submitted_at, finished_at, &artifact, &model2);
                if let Err(e) = st.db.insert_message_with_job(
                    session_id,
                    "assistant",
                    &msg,
                    "",
                    "[]",
                    "[]",
                    &[],
                    &[artifact.clone()],
                    &[],
                    Some(job_id),
                ) {
                    // 完成消息保存失败（磁盘/权限问题）：记录日志，避免视频"生成成功却无声无息丢失"
                    eprintln!("[video] 会话 {session_id} 完成消息保存失败: {e}");
                    return;
                }
                st.db.touch(session_id);
                let payload = json!({
                    "kind": "video_done",
                    "conversation_id": session_id,
                    "item": artifact,
                    "text": msg,
                    "job": job,
                });
                let _ = app2.emit("chat_event", payload);
            }
            Err(e) => {
                // 主动取消（删除/编辑会话）时任务已在 DB 中标记为 canceled，
                // 这里不再覆盖状态，仅推送"已取消"事件
                let canceled = e == "任务已取消";
                if !canceled {
                    let _ = st.db.fail_job(session_id, job_id, &e);
                }
                let job = st.db.get_job(session_id, job_id);
                let payload = json!({
                    "kind": if canceled { "job_canceled" } else { "video_failed" },
                    "conversation_id": session_id,
                    "text": if canceled { "视频任务已取消".to_string() } else { format!("视频生成失败：{e}") },
                    "job": job,
                });
                let _ = app2.emit("chat_event", payload);
            }
        }
    });
}

/// 后台任务注册表守卫：async 任务退出（含 panic）时自动移除条目，
/// 避免条目残留导致任务永久 pending 且无法恢复轮询
struct BgTaskGuard {
    state: Arc<AppState>,
    key: String,
}

impl Drop for BgTaskGuard {
    fn drop(&mut self) {
        self.state.bg_tasks.lock().unwrap().remove(&self.key);
    }
}

/// 恢复某会话中未完成的视频任务（应用重启后首次打开/列出该会话时调用）。
/// 任务参数从 jobs 表读取，服务商地址与密钥优先从用户配置的 providers 中解析，
/// 缺失时回退到旧版 gen.* 兼容字段；凭据为空时保持 pending，等待用户配置后再恢复。
pub(crate) fn resume_jobs(state: &Arc<AppState>, app: &AppHandle, session_id: i64) {
    let Ok(jobs) = state.db.list_jobs(session_id) else {
        return;
    };
    let settings = state.db.get_settings();
    for job in jobs.iter().filter(|j| j.status == "pending") {
        let Some((api, base, api_key)) = resolve_job_credentials(&settings, job) else {
            continue;
        };
        if api_key.trim().is_empty() {
            continue;
        }
        spawn_video_poller(
            state, app, session_id, job.id, api, &base, &api_key, &job.model, &job.request_id,
        );
    }
}

/// 按任务记录的服务商与模型名解析轮询所需地址与密钥
fn resolve_job_credentials(
    settings: &AppSettings,
    job: &Job,
) -> Option<(&'static str, String, String)> {
    // 1) 优先在用户配置的 providers 中按模型名匹配（新 UI 的凭据保存在这里）
    for p in &settings.providers {
        let is_dashscope_base = p.api_base.to_lowercase().contains("dashscope");
        for m in &p.models {
            if m.model_type != crate::models::MODEL_TYPE_VIDEO || m.name != job.model {
                continue;
            }
            let api = match m.video_api.as_str() {
                VIDEO_API_DASHSCOPE => VIDEO_API_DASHSCOPE,
                VIDEO_API_SILICONFLOW => VIDEO_API_SILICONFLOW,
                _ => {
                    if is_dashscope_base {
                        VIDEO_API_DASHSCOPE
                    } else {
                        VIDEO_API_SILICONFLOW
                    }
                }
            };
            if api != job.api {
                continue;
            }
            let base = if api == VIDEO_API_DASHSCOPE {
                native_dashscope_base(&p.api_base)
            } else {
                p.api_base.trim_end_matches('/').to_string()
            };
            return Some((api, base, p.api_key.clone()));
        }
    }
    // 2) 回退到旧版 gen.* 兼容字段
    if job.api == VIDEO_API_DASHSCOPE {
        let s = &settings.gen.alibaba;
        Some((
            VIDEO_API_DASHSCOPE,
            native_dashscope_base(&s.base_url),
            s.api_key.clone(),
        ))
    } else {
        let s = &settings.gen.siliconflow;
        Some((VIDEO_API_SILICONFLOW, s.base_url.clone(), s.api_key.clone()))
    }
}

/// 组装视频完成总结文案（含耗时）
fn build_video_done_msg(submitted_at: i64, finished_at: i64, artifact: &Artifact, model: &str) -> String {
    let secs = ((finished_at - submitted_at) / 1000).max(1);
    let dur = if secs >= 60 {
        format!("{} 分 {} 秒", secs / 60, secs % 60)
    } else {
        format!("{} 秒", secs)
    };
    format!(
        "✅ 视频生成完成\n\n- 视频文件：videos/{}\n- 模型：{}\n- 耗时：{}\n\n点击下方卡片即可查看/播放。",
        artifact.name, model, dur
    )
}

/// 后台轮询阿里云视频任务状态并下载保存
async fn poll_video_dashscope(
    state: &AppState,
    session_id: i64,
    native_base: &str,
    task_id: &str,
    _model: &str,
    api_key: &str,
    token: &crate::llm::CancelToken,
) -> Result<Artifact, String> {
    let mut attempts = 0u32;
    loop {
        tokio::select! {
            _ = token.wait() => return Err("任务已取消".into()),
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
        }
        attempts += 1;
        let sresp = state
            .client
            .get(format!("{native_base}/api/v1/tasks/{task_id}"))
            .bearer_auth(&api_key)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("阿里云视频状态查询失败: {e}"))?;
        let st = sresp.status();
        if !st.is_success() {
            let text = sresp.text().await.unwrap_or_default();
            return Err(format!(
                "阿里云视频状态查询错误 ({st}): {}",
                text.chars().take(300).collect::<String>()
            ));
        }
        let sv: serde_json::Value = sresp
            .json()
            .await
            .map_err(|e| format!("阿里云视频状态解析失败: {e}"))?;
        let task_status = sv["output"]["task_status"].as_str().unwrap_or("").to_uppercase();
        if task_status == "FAILED" || task_status == "CANCELED" {
            let msg = sv["output"]["message"]
                .as_str()
                .unwrap_or(sv["message"].as_str().unwrap_or("未知错误"));
            return Err(format!("阿里云视频生成失败: {msg}"));
        }
        if task_status == "SUCCEEDED" {
            let url = sv["output"]["video_url"].as_str().unwrap_or("");
            if url.is_empty() {
                return Err(format!("阿里云视频生成完成但未返回下载地址: {sv}"));
            }
            let vresp = state
                .client
                .get(url)
                .timeout(std::time::Duration::from_secs(180))
                .send()
                .await
                .map_err(|e| format!("下载视频失败: {e}"))?;
            let bytes = vresp
                .bytes()
                .await
                .map_err(|e| format!("读取视频失败: {e}"))?
                .to_vec();
            let fname = format!("video_{}.mp4", crate::db::now_ms());
            let dir = state.db.session_videos_dir(session_id);
            let _ = std::fs::create_dir_all(&dir);
            std::fs::write(dir.join(&fname), &bytes).map_err(|e| format!("保存视频失败: {e}"))?;
            let artifact = Artifact {
                kind: "video".into(),
                name: fname.clone(),
                path: format!("videos/{fname}"),
                size: bytes.len() as i64,
                note: String::new(),
            };
            return Ok(artifact);
        }
        if attempts > 180 {
            return Err("阿里云视频生成超时（超过 15 分钟），请稍后重试".into());
        }
    }
}
