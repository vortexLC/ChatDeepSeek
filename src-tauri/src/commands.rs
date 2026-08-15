use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use reqwest::Client;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::oneshot;

use crate::db::Db;
use crate::llm::{CancelToken, run_agent};
use crate::models::*;

const MAX_WEBPAGE_BYTES: usize = 8 * 1024 * 1024;
const MAX_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;

/// 待保存的附件（前端 FileReader 得到的 data URL）
#[derive(serde::Deserialize)]
pub struct UploadAttachment {
    pub name: String,
    pub mime: String,
    pub data_url: String,
}

/// 允许作为文档附件的扩展名（文本类 + PDF）
const DOC_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "csv", "json", "xml", "yaml", "yml", "log", "ini", "conf", "toml",
    "sql", "py", "js", "ts", "rs", "java", "c", "cpp", "h", "hpp", "go", "rb", "php", "sh",
    "bat", "ps1", "html", "css", "scss", "vue", "tsx", "jsx", "pdf",
];

fn classify_upload(name: &str, mime: &str) -> Result<(String, String), String> {
    if mime.starts_with("image/") {
        // SVG 可内嵌脚本（且旧逻辑会以 jpg 扩展名保存导致损坏），直接拒绝
        if mime == "image/svg+xml" || name.to_lowercase().ends_with(".svg") {
            return Err(format!(
                "不支持该附件类型（{name}）：SVG 图片可能携带脚本，请转换为 PNG/JPEG 后上传"
            ));
        }
        let ext = match mime {
            "image/png" => "png",
            "image/gif" => "gif",
            "image/webp" => "webp",
            "image/bmp" => "bmp",
            _ => "jpg",
        };
        return Ok((ext.to_string(), "image".to_string()));
    }
    let ext = name
        .rsplit('.')
        .next()
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    if DOC_EXTENSIONS.contains(&ext.as_str()) {
        return Ok((ext, "document".to_string()));
    }
    Err(format!(
        "不支持该附件类型（{name}）：请上传图片或文本/PDF 文档"
    ))
}

/// 校验上传附件是否含图片且当前对话模型支持图片输入（落盘前调用，避免孤儿附件文件）
fn check_uploads_model(
    settings: &AppSettings,
    conv: &Conversation,
    uploads: &[UploadAttachment],
) -> Result<(), String> {
    let has_image = uploads.iter().any(|u| u.mime.starts_with("image/"));
    if has_image {
        if let Some((_, m)) = settings.resolve_chat_model(conv) {
            if m.model_type != crate::models::MODEL_TYPE_VISION {
                return Err(format!(
                    "当前对话模型「{}」不支持图片输入，请选择多模态（视觉）模型，或仅上传文档/文本文件",
                    m.name
                ));
            }
        }
    }
    Ok(())
}

/// 保存前端上传的附件到会话 uploads/ 目录
fn save_attachments(
    state: &AppState,
    conv_id: i64,
    atts: &[UploadAttachment],
) -> Result<Vec<crate::models::Attachment>, String> {
    if atts.is_empty() {
        return Ok(Vec::new());
    }
    let dir = state.db.session_uploads_dir(conv_id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    use base64::Engine;
    let mut out = Vec::new();
    for (i, a) in atts.iter().enumerate() {
        let (ext, kind) = classify_upload(&a.name, &a.mime)?;
        let b64 = a.data_url.split(',').last().unwrap_or("");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("附件「{}」解码失败: {e}", a.name))?;
        if bytes.is_empty() {
            return Err(format!("附件「{}」内容为空", a.name));
        }
        if bytes.len() > MAX_ATTACHMENT_BYTES {
            return Err(format!("附件「{}」过大（超过 8MB）", a.name));
        }
        let fname = format!("att_{}_{}.{}", crate::db::now_ms(), i, ext);
        std::fs::write(dir.join(&fname), &bytes)
            .map_err(|e| format!("保存附件失败: {e}"))?;
        out.push(crate::models::Attachment {
            name: a.name.clone(),
            mime: if kind == "image" && !a.mime.starts_with("image/") {
                format!("image/{ext}")
            } else {
                a.mime.clone()
            },
            kind,
            path: format!("uploads/{fname}"),
            size: bytes.len() as i64,
        });
    }
    Ok(out)
}









pub struct AppState {
    pub db: Db,
    pub cancels: Mutex<HashMap<i64, Arc<CancelToken>>>,
    /// 越界访问待用户确认：conv_id -> 应答通道
    pub pending_perms: Mutex<HashMap<i64, oneshot::Sender<bool>>>,
    pub client: Client,
}

impl AppState {
    pub fn new(db: Db) -> Result<Self, Box<dyn std::error::Error>> {
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            // 连接保活：空闲连接保留 5 分钟——多轮工具循环 / 连续对话复用
            // 同一服务商连接，省去每次 TLS+TCP 握手，降低首字节延迟
            .pool_idle_timeout(std::time::Duration::from_secs(300))
            .pool_max_idle_per_host(8)
            .user_agent("ChatDeepSeek/0.1")
            .build()?;
        Ok(Self {
            db,
            cancels: Mutex::new(HashMap::new()),
            pending_perms: Mutex::new(HashMap::new()),
            client,
        })
    }
}

/// 取消某会话的所有后台任务（删除/编辑会话时调用）
pub fn cancel_session_tasks(state: &AppState, conv_id: i64) {
    // 进行中的 agent（流式生成）同样取消，避免删除/编辑后继续运行并写库
    if let Some(t) = state.cancels.lock().unwrap().remove(&conv_id) {
        t.cancel();
    }
    // 丢弃等待用户确认的越界访问请求：会话已删除/编辑，确认已无意义。
    // 丢弃 oneshot sender 后等待方立即收到"确认已失效"，agent 任务得以尽快结束
    state.pending_perms.lock().unwrap().remove(&conv_id);
}

/// 退出应用时取消所有进行中的任务，释放网络连接与系统资源
pub fn cancel_all_tasks(state: &AppState) {
    for t in state.cancels.lock().unwrap().values() {
        t.cancel();
    }
    // 清空所有待确认的越界访问请求（应用退出，确认无意义）
    state.pending_perms.lock().unwrap().clear();
}

/// 请求用户确认越界访问（会话目录之外的文件操作），90 秒超时自动拒绝
pub async fn request_permission(
    app: &AppHandle,
    state: &AppState,
    conversation_id: i64,
    tool: &str,
    path: &str,
) -> Result<bool, String> {
    let (tx, rx) = oneshot::channel();
    state
        .pending_perms
        .lock()
        .unwrap()
        .insert(conversation_id, tx);
    let payload = serde_json::json!({
        "kind": "permission_request",
        "conversation_id": conversation_id,
        "tool": tool,
        "path": path,
    });
    let _ = app.emit("chat_event", payload);
    let result = tokio::time::timeout(std::time::Duration::from_secs(90), rx).await;
    // 无论结果如何都清理槽位，避免超时/取消后残留过期条目
    state.pending_perms.lock().unwrap().remove(&conversation_id);
    match result {
        Ok(Ok(true)) => Ok(true),
        Ok(Ok(false)) => Err(format!("用户拒绝了该操作：{tool} {path}")),
        Ok(Err(_)) => Err("权限确认已失效".into()),
        Err(_) => Err("等待用户确认超时，已拒绝该操作".into()),
    }
}

fn err<T>(e: impl std::fmt::Display) -> Result<T, String> {
    Err(format!("{e}"))
}

#[tauri::command]
pub fn get_initial_state(state: State<Arc<AppState>>) -> Result<InitialState, String> {
    let conversations = state.db.list_conversations();
    let settings = state.db.get_settings();
    Ok(InitialState {
        conversations,
        settings,
    })
}

#[tauri::command]
pub fn list_conversations(state: State<Arc<AppState>>) -> Result<Vec<Conversation>, String> {
    Ok(state.db.list_conversations())
}

#[tauri::command]
pub fn create_conversation(state: State<Arc<AppState>>) -> Result<Conversation, String> {
    let settings = state.db.get_settings();
    state.db.create_conversation(&settings)
}

#[tauri::command]
pub fn update_conversation(
    state: State<Arc<AppState>>,
    id: i64,
    patch: ConversationPatch,
) -> Result<(), String> {
    state.db.update_conversation(id, &patch)
}

#[tauri::command]
pub fn delete_conversation(state: State<Arc<AppState>>, id: i64) -> Result<(), String> {
    log::info!("[chat] 删除会话 {id}");
    cancel_session_tasks(&state, id);
    state.db.delete_conversation(id)
}

#[tauri::command]
pub fn clear_all_conversations(state: State<Arc<AppState>>) -> Result<(), String> {
    // 先取消所有进行中的任务，否则它们会在目录删除后继续写库，
    // 把已清空的会话目录与消息库"复活"成僵尸数据
    log::info!("[chat] 清空所有会话");
    cancel_all_tasks(&state);
    state.db.clear_all()
}

#[tauri::command]
pub fn get_messages(state: State<Arc<AppState>>, id: i64) -> Result<Vec<Message>, String> {
    state.db.list_messages(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_context_status(
    state: State<Arc<AppState>>,
    conversation_id: i64,
) -> Result<ContextUsage, String> {
    Ok(state.db.context_usage(conversation_id))
}

#[tauri::command]
pub fn get_settings(state: State<Arc<AppState>>) -> Result<AppSettings, String> {
    Ok(state.db.get_settings())
}

#[tauri::command]
pub fn save_settings(
    state: State<Arc<AppState>>,
    settings: AppSettings,
) -> Result<(), String> {
    state.db.save_settings(&settings).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    conversation_id: i64,
    content: String,
    attachments: Option<Vec<UploadAttachment>>,
) -> Result<(), String> {
    let conv = state
        .db
        .get_conversation(conversation_id)
        .ok_or_else(|| "对话不存在".to_string())?;

    let settings = state.db.get_settings();

    let trimmed = content.trim().to_string();
    let uploads = attachments.unwrap_or_default();
    log::info!(
        "[chat] 会话 {} 发送消息（{} 字符，附件 {} 个）",
        conversation_id,
        trimmed.chars().count(),
        uploads.len()
    );
    if trimmed.is_empty() && uploads.is_empty() {
        return err("消息内容为空");
    }

    // 先校验再落盘：模型不支持图片 / 上下文已满时直接拒绝，
    // 避免上传文件已写入 uploads/ 却无法发送的孤儿文件
    check_uploads_model(&settings, &conv, &uploads)?;
    let usage = state.db.context_usage(conversation_id);
    if usage.full {
        return err("上下文已满，请新开会话");
    }
    if state.cancels.lock().unwrap().contains_key(&conversation_id) {
        return err("当前对话正在生成中，请先停止或等待完成");
    }

    let saved_atts = save_attachments(&state, conversation_id, &uploads)?;

    state
        .db
        .insert_message_with_attachments(
            conversation_id,
            "user",
            &trimmed,
            "",
            "[]",
            "[]",
            &[],
            &[],
            &[],
            &saved_atts,
        )
        .map_err(|e| e.to_string())?;
    state.db.set_title_if_default(conversation_id, &trimmed);
    state.db.touch(conversation_id);

    spawn_agent(app, state.inner().clone(), conv, settings, conversation_id)
}

/// 编辑已发送的用户消息：删除该消息及其后所有消息，以新内容重新生成
#[tauri::command]
pub async fn edit_and_resend(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    conversation_id: i64,
    message_id: i64,
    content: String,
    attachments: Option<Vec<UploadAttachment>>,
) -> Result<(), String> {
    let conv = state
        .db
        .get_conversation(conversation_id)
        .ok_or_else(|| "对话不存在".to_string())?;
    let row = state
        .db
        .get_message(conversation_id, message_id)
        .ok_or_else(|| "消息不存在".to_string())?;
    if row.role != "user" {
        return err("只能编辑用户发送的消息");
    }
    let trimmed = content.trim().to_string();
    let uploads = attachments.unwrap_or_default();
    log::info!(
        "[chat] 会话 {} 编辑消息 {} 并重新发送（{} 字符，附件 {} 个）",
        conversation_id,
        message_id,
        trimmed.chars().count(),
        uploads.len()
    );
    if trimmed.is_empty() && uploads.is_empty() {
        return err("消息内容为空");
    }
    if state.cancels.lock().unwrap().contains_key(&conversation_id) {
        return err("当前对话正在生成中，请先停止或等待完成");
    }

    // 未重新上传附件时沿用原消息附件
    let old_atts: Vec<crate::models::Attachment> =
        serde_json::from_str(&row.attachments).unwrap_or_default();
    // 先校验再落盘，避免孤儿附件文件
    check_uploads_model(&state.db.get_settings(), &conv, &uploads)?;
    let saved_atts = if uploads.is_empty() {
        old_atts
    } else {
        save_attachments(&state, conversation_id, &uploads)?
    };

    // 清理将被截断消息引用的图片文件：消息删除后这些图片成为孤儿文件，
    // 累积在会话目录导致本地图片数量与当前轮次生成记录不一致
    if let Ok(msgs) = state.db.list_messages(conversation_id) {
        let dir = state.db.session_images_dir(conversation_id);
        let mut removed = 0usize;
        for m in msgs.iter().filter(|m| m.id >= message_id) {
            for a in m.artifacts.iter().filter(|a| a.kind == "image") {
                if std::fs::remove_file(dir.join(&a.name)).is_ok() {
                    removed += 1;
                }
            }
        }
        if removed > 0 {
            log::info!(
                "[chat] 会话 {} 编辑重发：清理 {} 个孤儿图片文件",
                conversation_id,
                removed
            );
        }
    }
    state.db.delete_messages_from(conversation_id, message_id)?;
    cancel_session_tasks(&state, conversation_id);

    // 编辑点已进入摘要范围时压缩状态失效：重置摘要，避免旧摘要与编辑后的
    // 新对话线程语义漂移（编辑内容不会出现在旧摘要中）
    if let Some(c) = state.db.get_conversation(conversation_id) {
        if message_id <= c.summarized_until {
            let _ = state.db.update_conversation_summary(conversation_id, "", 0);
        }
    }

    let usage = state.db.context_usage(conversation_id);
    if usage.full {
        return err("上下文已满，请新开会话");
    }

    state
        .db
        .insert_message_with_attachments(
            conversation_id,
            "user",
            &trimmed,
            "",
            "[]",
            "[]",
            &[],
            &[],
            &[],
            &saved_atts,
        )
        .map_err(|e| e.to_string())?;
    state.db.touch(conversation_id);

    // 以最新会话快照启动 agent（压缩状态可能已被重置）
    let conv = state
        .db
        .get_conversation(conversation_id)
        .ok_or_else(|| "对话不存在".to_string())?;
    let settings = state.db.get_settings();
    spawn_agent(app, state.inner().clone(), conv, settings, conversation_id)
}

/// 启动 agent 后台任务；"检查是否正在生成 + 注册取消令牌"在同一临界区内完成，
/// 防止并发调用同时通过检查导致同一会话双 agent 并行
fn spawn_agent(
    app: AppHandle,
    state: Arc<AppState>,
    conv: Conversation,
    settings: AppSettings,
    conversation_id: i64,
) -> Result<(), String> {
    let token = Arc::new(CancelToken::new());
    {
        let mut cancels = state.cancels.lock().unwrap();
        if cancels.contains_key(&conversation_id) {
            return err("当前对话正在生成中，请先停止或等待完成");
        }
        cancels.insert(conversation_id, token.clone());
    }
    tokio::spawn(async move {
        run_agent(app, state.clone(), conv, settings, token).await;
        state.cancels.lock().unwrap().remove(&conversation_id);
    });
    Ok(())
}

#[tauri::command]
pub fn stop_generation(state: State<Arc<AppState>>, conversation_id: i64) -> Result<(), String> {
    log::info!("[chat] 停止会话 {conversation_id} 生成");
    if let Some(token) = state.cancels.lock().unwrap().get(&conversation_id) {
        token.cancel();
    }
    Ok(())
}

/// 测试服务商下某个模型是否可用（按模型类型分别测试）
#[tauri::command]
pub async fn test_model(
    state: State<'_, Arc<AppState>>,
    provider: crate::models::ProviderConfig,
    model: crate::models::ModelConfig,
) -> Result<String, String> {
    let key = provider.api_key.trim();
    if key.is_empty() {
        return Err("未填写该服务商的 API Key".into());
    }
    let base = provider.api_base.trim_end_matches('/');

    match model.model_type.as_str() {
        crate::models::MODEL_TYPE_TEXT | crate::models::MODEL_TYPE_VISION => {
            let body = serde_json::json!({
                "model": model.name,
                "max_tokens": 4,
                "messages": [{"role": "user", "content": "hi"}],
                "stream": false,
            });
            let resp = state
                .client
                .post(format!("{base}/chat/completions"))
                .bearer_auth(key)
                .timeout(std::time::Duration::from_secs(30))
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("网络请求失败: {e}"))?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if status.is_success() {
                Ok(format!("「{}」连接成功，模型可用", model.name))
            } else {
                Err(format!(
                    "「{}」测试失败 ({status}): {}",
                    model.name,
                    text.chars().take(200).collect::<String>()
                ))
            }
        }
        crate::models::MODEL_TYPE_IMAGE => {
            let body = serde_json::json!({
                "model": model.name,
                "prompt": "a red circle",
                "n": 1,
            });
            let resp = state
                .client
                .post(format!("{base}/images/generations"))
                .bearer_auth(key)
                .timeout(std::time::Duration::from_secs(60))
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("网络请求失败: {e}"))?;
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            if status.is_success() {
                Ok(format!("「{}」图片生成接口可用（已生成测试图片，未保存）", model.name))
            } else {
                Err(format!(
                    "「{}」测试失败 ({status}): {}",
                    model.name,
                    text.chars().take(200).collect::<String>()
                ))
            }
        }
        _ => Err("未知模型类型".into()),
    }
}

/// 判断目标 URL 是否指向内网/本地/回环地址（SSRF 防护）
fn is_ssrf_target(url: &str) -> bool {
    let parsed = match reqwest::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return true,
    };
    let host = match parsed.host_str() {
        Some(h) => h,
        None => return true,
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let mut ips: Vec<std::net::IpAddr> = Vec::new();
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        ips.push(ip);
    } else if let Ok(addrs) = std::net::ToSocketAddrs::to_socket_addrs(&(host, 0)) {
        for a in addrs {
            ips.push(a.ip());
        }
    }
    if ips.is_empty() {
        // 域名无法解析，视为不可信目标
        return true;
    }
    ips.into_iter().any(is_blocked_ip)
}

fn is_blocked_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => is_blocked_v4(v4),
        std::net::IpAddr::V6(v6) => {
            // IPv4-mapped IPv6（如 ::ffff:127.0.0.1）还原为 IPv4 后再检查，
            // 否则可绕过回环/内网拦截
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_blocked_v4(v4);
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xfe00 == 0xfc00)
                || v6.segments()[0] == 0xfe80
        }
    }
}

fn is_blocked_v4(v4: std::net::Ipv4Addr) -> bool {
    let o = v4.octets();
    o[0] == 10
        || o[0] == 127
        || o[0] == 0
        || (o[0] == 172 && o[1] >= 16 && o[1] <= 31)
        || (o[0] == 192 && o[1] == 168)
        || (o[0] == 169 && o[1] == 254)
        || (o[0] == 100 && (64..=127).contains(&o[1]))
        || o[0] >= 224
}

/// 拼接错误链：reqwest 对 body 读取失败统一包装为 "error decoding response body"，
/// 真实原因（超时/连接中断/TLS 等）在 source 链中，拼出来便于定位问题
fn err_source_chain(e: &reqwest::Error) -> String {
    use std::error::Error as _;
    let mut out = String::new();
    let mut src = e.source();
    while let Some(s) = src {
        out.push_str(" -> ");
        out.push_str(&s.to_string());
        src = s.source();
    }
    out
}

/// 抓取网页，返回可安全渲染的页面 HTML（保留结构/样式/图片，剔除脚本等危险内容），供右侧预览面板展示
#[tauri::command]
pub async fn fetch_webpage(
    state: State<'_, Arc<AppState>>,
    url: String,
) -> Result<WebPage, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return err("仅支持 http/https 链接");
    }
    // 手动跟随重定向（reqwest 默认跟随策略无法逐跳复检）：
    // 每一跳都重新校验目标地址，防止通过重定向跳转到内网/回环地址（SSRF 绕过）
    let mut current = url;
    let mut redirects = 0usize;
    let resp = loop {
        if is_ssrf_target(&current) {
            return err("出于安全考虑，仅允许访问公网地址");
        }
        let resp = state
            .client
            .get(&current)
            .header(
                "user-agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
            )
            .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
            // 显式请求未压缩内容：部分服务器/代理返回损坏的 gzip 流会导致解码失败
            .header("accept-encoding", "identity")
            // 总超时（连接+下载）放宽到 60s：大页面/慢网络不再因 20s 上限误报 body 解码错误
            .timeout(std::time::Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| format!("网页请求失败: {e}"))?;
        if resp.status().is_redirection() {
            redirects += 1;
            if redirects > 5 {
                return err("重定向次数过多，已停止");
            }
            let Some(loc) = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
            else {
                return err("重定向响应缺少 Location 头");
            };
            current = resp
                .url()
                .join(loc)
                .map(|u| u.to_string())
                .map_err(|e| format!("重定向地址无效: {e}"))?;
            // 消费响应体后继续，避免连接复用被破坏
            let _ = resp.text().await;
            continue;
        }
        break resp;
    };
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("网页返回错误状态码: {status}"));
    }
    if let Some(len) = resp
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
    {
        if len > MAX_WEBPAGE_BYTES {
            return Err(format!("网页过大（超过 {}MB），无法预览", MAX_WEBPAGE_BYTES / 1024 / 1024));
        }
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取网页内容失败: {}{}", e, err_source_chain(&e)))?;
    if bytes.len() > MAX_WEBPAGE_BYTES {
        return Err(format!("网页过大（超过 {}MB），无法预览", MAX_WEBPAGE_BYTES / 1024 / 1024));
    }
    let html = String::from_utf8_lossy(&bytes);
    let doc = scraper::Html::parse_document(&html);
    let title = doc
        .select(&scraper::Selector::parse("title").unwrap())
        .next()
        .map(|e| e.text().collect::<String>())
        .unwrap_or_default()
        .trim()
        .to_string();
    let mut body_buf = String::new();
    if let Some(body) = doc.select(&scraper::Selector::parse("body").unwrap()).next() {
        sanitize_html(&body, &mut body_buf);
    }
    let mut out = String::from(
        "<!doctype html><html><head><meta charset=\"utf-8\"><base href=\"",
    );
    out.push_str(&escape_attr(&current));
    out.push_str(
        "\"><style>img{max-width:100%;height:auto}table{border-collapse:collapse;width:100%;margin:8px 0}th,td{border:1px solid #ccc;padding:5px 8px;text-align:left}body{font-family:-apple-system,'Segoe UI','PingFang SC','Microsoft YaHei',sans-serif;margin:0;padding:14px;line-height:1.7;font-size:14px;color:#1c2027;word-break:break-word}a{color:#4d6bfe;text-decoration:none}pre{background:#f5f6f8;padding:10px;border-radius:6px;overflow-x:auto}code{background:#f5f6f8;border-radius:4px;padding:1px 5px}</style></head><body>",
    );
    out.push_str(&body_buf);
    out.push_str("</body></html>");
    if out.len() < 250 {
        return Err("未能提取到网页正文内容".into());
    }
    Ok(WebPage { url: current, title, html: out })
}

/// 白名单序列化：只保留安全标签与属性，转义文本，剔除脚本/表单等危险内容
fn sanitize_html(el: &scraper::ElementRef, buf: &mut String) {
    use scraper::Node;
    let name = el.value().name();
    if matches!(
        name,
        "script"
            | "style"
            | "noscript"
            | "iframe"
            | "svg"
            | "template"
            | "form"
            | "object"
            | "embed"
            | "video"
            | "audio"
            | "canvas"
            | "link"
            | "meta"
            | "input"
            | "button"
            | "select"
            | "textarea"
            | "head"
    ) {
        return;
    }
    let void = matches!(name, "br" | "img" | "hr" | "wbr" | "source" | "track");
    buf.push('<');
    buf.push_str(name);
    for (attr, val) in el.value().attrs() {
        if matches!(attr, "href" | "src" | "alt" | "title" | "width" | "height") {
            // 过滤 javascript: / data:text/html / vbscript: 等危险协议，防止 XSS
            let safe = if matches!(attr, "href" | "src") {
                sanitize_protocol(val)
            } else {
                val.to_string()
            };
            buf.push(' ');
            buf.push_str(attr);
            buf.push_str("=\"");
            buf.push_str(&escape_attr(&safe));
            buf.push('"');
        }
    }
    if void {
        buf.push('>');
        return;
    }
    buf.push('>');
    for child in el.children() {
        match child.value() {
            Node::Element(_) => {
                if let Some(ce) = scraper::ElementRef::wrap(child) {
                    sanitize_html(&ce, buf);
                }
            }
            Node::Text(t) => buf.push_str(&escape_text(&t.text)),
            _ => {}
        }
    }
    buf.push_str("</");
    buf.push_str(name);
    buf.push('>');
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(s: &str) -> String {
    escape_text(s).replace('"', "&quot;")
}

/// 过滤 URL 属性中的危险协议，若协议不在 https/http/相对/data图片 白名单内则置空
fn sanitize_protocol(v: &str) -> String {
    let t = v.trim();
    let lower = t.to_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("//")
        || lower.starts_with('/')
        || lower.starts_with('#')
        || lower.starts_with("data:image/")
        || t.is_empty()
    {
        t.to_string()
    } else {
        String::new()
    }
}

pub fn emit(app: &AppHandle, conversation_id: i64, kind: &str, text: Option<&str>) {
    let mut payload = serde_json::json!({
        "kind": kind,
        "conversation_id": conversation_id,
    });
    if let Some(t) = text {
        payload["text"] = serde_json::Value::String(t.to_string());
    }
    let _ = app.emit("chat_event", payload);
}

/// 读取会话目录内的文件内容（文本），供右侧面板文件预览/Diff 使用
#[tauri::command]
pub async fn fetch_file_content(
    state: State<'_, Arc<AppState>>,
    conversation_id: i64,
    path: String,
) -> Result<crate::models::WebPage, String> {
    let p = state
        .db
        .session_abs_path(conversation_id, &path)
        .ok_or_else(|| "路径越界：只能访问本会话目录内的文件".to_string())?;
    if !p.is_file() {
        return Err(format!("文件不存在: {path}"));
    }
    let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    if size > 2 * 1024 * 1024 {
        return Err("文件过大（超过 2MB），无法预览".into());
    }
    let content = std::fs::read_to_string(&p).map_err(|e| format!("读取文件失败: {e}"))?;
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.clone());
    let escaped = escape_html(&content);
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><style>body{{font-family:Consolas,monospace;font-size:13px;line-height:1.6;margin:0;padding:14px;background:#fafafa;color:#1c2027;white-space:pre-wrap;word-break:break-word}}a{{color:#4d6bfe}}@media (prefers-color-scheme: dark){{body{{background:#16181d;color:#e2e4e9}}a{{color:#7db1ff}}}}</style></head><body>{escaped}</body></html>"
    );
    Ok(crate::models::WebPage {
        url: format!("file:///{}", p.display()),
        title: format!("{name}（{size} 字节）"),
        html,
    })
}

/// 返回会话产物的本地绝对路径（前端通过 asset 协议展示图片/视频）
#[tauri::command]
pub fn get_artifact_abs_path(
    state: State<Arc<AppState>>,
    conversation_id: i64,
    path: String,
) -> Result<String, String> {
    let p = state
        .db
        .session_abs_path(conversation_id, &path)
        .ok_or_else(|| "路径越界".to_string())?;
    if !p.exists() {
        return Err(format!("文件不存在: {path}"));
    }
    Ok(p.to_string_lossy().to_string())
}

/// 用户对越界访问确认请求的应答
#[tauri::command]
pub fn respond_permission(
    state: State<Arc<AppState>>,
    conversation_id: i64,
    approve: bool,
) -> Result<(), String> {
    if let Some(tx) = state
        .pending_perms
        .lock()
        .unwrap()
        .remove(&conversation_id)
    {
        let _ = tx.send(approve);
    }
    Ok(())
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
