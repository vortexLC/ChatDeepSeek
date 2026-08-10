use serde_json::json;
use tauri::{AppHandle, Emitter};

use crate::commands::{AppState, emit};
use crate::models::{Artifact, GenSettings};

/// 生成图片：调用硅基流动 images/generations，下载保存到会话 images/ 目录
pub async fn generate_image(
    app: &AppHandle,
    state: &AppState,
    gen: &GenSettings,
    session_id: i64,
    arguments: &str,
) -> Result<(String, Vec<Artifact>), String> {
    let sf = &gen.siliconflow;
    if sf.api_key.trim().is_empty() {
        return Err("未配置图像生成服务 API Key（设置 → 图像视频 → 硅基流动）".into());
    }
    let args: serde_json::Value =
        serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let prompt = args["prompt"].as_str().unwrap_or("").trim().to_string();
    if prompt.is_empty() {
        return Err("generate_image 缺少 prompt 参数".into());
    }
    let model = args["model"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(&sf.image_model)
        .to_string();
    let image_size = args["image_size"].as_str().unwrap_or("1024x1024");

    let mut body = json!({
        "model": model,
        "prompt": prompt,
        "image_size": image_size,
        "batch_size": 1,
    });
    if let Some(n) = args["negative_prompt"].as_str() {
        if !n.is_empty() {
            body["negative_prompt"] = json!(n);
        }
    }

    emit(app, session_id, "status", Some("generating"));
    let base = sf.base_url.trim_end_matches('/');
    let resp = state
        .client
        .post(format!("{base}/images/generations"))
        .bearer_auth(&sf.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("图片生成请求失败: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "图片生成 API 错误 ({status}): {}",
            text.chars().take(200).collect::<String>()
        ));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("图片生成响应解析失败: {e}"))?;
    let urls: Vec<String> = v["images"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|img| img["url"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if urls.is_empty() {
        return Err(format!("图片生成未返回结果: {v}"));
    }

    let dir = state.db.session_images_dir(session_id);
    let _ = std::fs::create_dir_all(&dir);
    let mut artifacts = Vec::new();
    let mut notes = Vec::new();
    for (i, url) in urls.iter().enumerate() {
        let img_resp = state
            .client
            .get(url)
            .timeout(std::time::Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| format!("下载图片失败: {e}"))?;
        let bytes = img_resp
            .bytes()
            .await
            .map_err(|e| format!("读取图片失败: {e}"))?;
        let ext = if url.to_lowercase().contains(".png") {
            "png"
        } else if url.to_lowercase().contains(".webp") {
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
    Ok((format!("已生成 {} 张图片并保存到会话目录：{}", artifacts.len(), notes.join("；")), artifacts))
}

/// 生成视频：提交 video/submit 后立即返回，后台轮询 video/status，
/// 完成后自动保存到会话 videos/ 目录并推送视频完成事件（不阻塞会话其它操作）
pub async fn generate_video(
    app: &AppHandle,
    state: &std::sync::Arc<AppState>,
    gen: &GenSettings,
    session_id: i64,
    arguments: &str,
) -> Result<(String, Vec<Artifact>), String> {
    let sf = &gen.siliconflow;
    if sf.api_key.trim().is_empty() {
        return Err("未配置视频生成服务 API Key（设置 → 图像视频 → 硅基流动）".into());
    }
    let args: serde_json::Value =
        serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let prompt = args["prompt"].as_str().unwrap_or("").trim().to_string();
    if prompt.is_empty() {
        return Err("generate_video 缺少 prompt 参数".into());
    }
    // I2V 图生视频需要 image；T2V 文生视频不需要
    let image = args["image"].as_str().unwrap_or("");
    let model = args["model"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(if image.is_empty() { &sf.video_model_t2v } else { &sf.video_model_i2v })
        .to_string();
    let image_size = args["image_size"].as_str().unwrap_or("1280x720");

    let mut body = json!({
        "model": model,
        "prompt": prompt,
        "image_size": image_size,
    });
    if !image.is_empty() {
        body["image"] = json!(image);
    }

    emit(app, session_id, "status", Some("generating"));
    let base = sf.base_url.trim_end_matches('/');
    let resp = state
        .client
        .post(format!("{base}/video/submit"))
        .bearer_auth(&sf.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("视频生成提交失败: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "视频生成 API 错误 ({status}): {}",
            text.chars().take(200).collect::<String>()
        ));
    }
    let v: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("视频提交响应解析失败: {e}"))?;
    let request_id = v["requestId"].as_str().unwrap_or("").to_string();
    if request_id.is_empty() {
        return Err(format!("视频提交未返回 requestId: {v}"));
    }

    // ---------- 后台轮询生成 ----------
    let key = format!("video-{session_id}");
    let token = std::sync::Arc::new(crate::llm::CancelToken::new());
    state.bg_tasks.lock().unwrap().insert(key.clone(), token.clone());
    let st = state.clone();
    let app2 = app.clone();
    let base2 = base.to_string();
    let prompt2 = prompt.clone();
    let model2 = model.clone();
    tokio::spawn(async move {
        let result = poll_video(
            &st,
            session_id,
            &base2,
            &request_id,
            &prompt2,
            &model2,
            &token,
        )
        .await;
        st.bg_tasks.lock().unwrap().remove(&key);
        match result {
            Ok((artifact, msg)) => {
                let _ = st.db.insert_message(
                    session_id,
                    "assistant",
                    &msg,
                    "",
                    "[]",
                    "[]",
                    &[],
                    &[artifact.clone()],
                );
                st.db.touch(session_id);
                let payload = json!({
                    "kind": "video_done",
                    "conversation_id": session_id,
                    "item": artifact,
                    "text": msg,
                });
                let _ = app2.emit("chat_event", payload);
            }
            Err(e) => {
                emit(&app2, session_id, "error", Some(&format!("视频生成失败：{e}")));
            }
        }
    });

    Ok((
        format!(
            "视频已提交（模型 {model}），正在后台生成，预计需要几分钟；生成完成后会自动通知并保存到会话目录"
        ),
        Vec::new(),
    ))
}

/// 后台轮询视频状态并下载保存
async fn poll_video(
    state: &AppState,
    session_id: i64,
    base: &str,
    request_id: &str,
    prompt: &str,
    model: &str,
    token: &crate::llm::CancelToken,
) -> Result<(Artifact, String), String> {
    let sf = &state.db.get_settings().gen.siliconflow;
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
            .bearer_auth(&sf.api_key)
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
                .map_err(|e| format!("读取视频失败: {e}"))?;
            let fname = format!("video_{}.mp4", crate::db::now_ms());
            let dir = state.db.session_videos_dir(session_id);
            let _ = std::fs::create_dir_all(&dir);
            std::fs::write(dir.join(&fname), &bytes).map_err(|e| format!("保存视频失败: {e}"))?;
            let artifact = Artifact {
                kind: "video".into(),
                name: fname.clone(),
                path: format!("videos/{fname}"),
                size: bytes.len() as i64,
                note: prompt.to_string(),
            };
            return Ok((
                artifact,
                format!("视频已生成并保存到会话目录：videos/{fname}（模型 {model}）"),
            ));
        }
        if attempts > 120 {
            return Err("视频生成超时（超过 10 分钟），请稍后重试".into());
        }
    }
}
