use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use reqwest::Client;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::oneshot;

use crate::db::Db;
use crate::llm::{CancelToken, run_agent};
use crate::models::*;

pub const DEEPSEEK_ANTHROPIC_BASE: &str = "https://api.deepseek.com/anthropic";
const MAX_WEBPAGE_BYTES: usize = 8 * 1024 * 1024;

pub struct AppState {
    pub db: Db,
    pub cancels: Mutex<HashMap<i64, Arc<CancelToken>>>,
    /// 越界访问待用户确认：conv_id -> 应答通道
    pub pending_perms: Mutex<HashMap<i64, oneshot::Sender<bool>>>,
    /// 后台任务（如视频生成轮询）：key -> 取消令牌
    pub bg_tasks: Mutex<HashMap<String, Arc<CancelToken>>>,
    pub client: Client,
}

impl AppState {
    pub fn new(db: Db) -> Result<Self, Box<dyn std::error::Error>> {
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .user_agent("ChatDeepSeek/0.1")
            .build()?;
        Ok(Self {
            db,
            cancels: Mutex::new(HashMap::new()),
            pending_perms: Mutex::new(HashMap::new()),
            bg_tasks: Mutex::new(HashMap::new()),
            client,
        })
    }
}

/// 取消某会话的所有后台任务（删除/编辑会话时调用）
pub fn cancel_session_tasks(state: &AppState, conv_id: i64) {
    let prefix = format!("video-{conv_id}");
    let mut tasks = state.bg_tasks.lock().unwrap();
    let keys: Vec<String> = tasks
        .keys()
        .filter(|k| k.starts_with(&prefix))
        .cloned()
        .collect();
    for k in keys {
        if let Some(t) = tasks.remove(&k) {
            t.cancel();
        }
    }
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
    match tokio::time::timeout(std::time::Duration::from_secs(90), rx).await {
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
    Ok(state.db.create_conversation(&settings))
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
    cancel_session_tasks(&state, id);
    state.db.delete_conversation(id)
}

#[tauri::command]
pub fn clear_all_conversations(state: State<Arc<AppState>>) -> Result<(), String> {
    state.bg_tasks.lock().unwrap().clear();
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
) -> Result<(), String> {
    let conv = state
        .db
        .get_conversation(conversation_id)
        .ok_or_else(|| "对话不存在".to_string())?;

    let settings = state.db.get_settings();

    let key = settings.deepseek.api_key.clone();
    if key.trim().is_empty() {
        return err("未配置 API Key，请在设置面板中填写 DeepSeek API Key");
    }

    let trimmed = content.trim();
    if trimmed.is_empty() {
        return err("消息内容为空");
    }

    let usage = state.db.context_usage(conversation_id);
    if usage.full {
        return err("上下文已满，请新开会话");
    }
    if state.cancels.lock().unwrap().contains_key(&conversation_id) {
        return err("当前对话正在生成中，请先停止或等待完成");
    }

    state
        .db
        .insert_message(conversation_id, "user", trimmed, "", "[]", "[]", &[], &[])
        .map_err(|e| e.to_string())?;
    state.db.set_title_if_default(conversation_id, trimmed);
    state.db.touch(conversation_id);

    spawn_agent(app, state.inner().clone(), conv, settings, conversation_id);

    Ok(())
}

/// 编辑已发送的用户消息：删除该消息及其后所有消息，以新内容重新生成
#[tauri::command]
pub async fn edit_and_resend(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    conversation_id: i64,
    message_id: i64,
    content: String,
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
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return err("消息内容为空");
    }
    if state.cancels.lock().unwrap().contains_key(&conversation_id) {
        return err("当前对话正在生成中，请先停止或等待完成");
    }

    state.db.delete_messages_from(conversation_id, message_id)?;
    cancel_session_tasks(&state, conversation_id);

    let usage = state.db.context_usage(conversation_id);
    if usage.full {
        return err("上下文已满，请新开会话");
    }

    state
        .db
        .insert_message(conversation_id, "user", trimmed, "", "[]", "[]", &[], &[])
        .map_err(|e| e.to_string())?;
    state.db.touch(conversation_id);

    let settings = state.db.get_settings();
    spawn_agent(app, state.inner().clone(), conv, settings, conversation_id);

    Ok(())
}

fn spawn_agent(
    app: AppHandle,
    state: Arc<AppState>,
    conv: Conversation,
    settings: AppSettings,
    conversation_id: i64,
) {
    let token = Arc::new(CancelToken::new());
    state
        .cancels
        .lock()
        .unwrap()
        .insert(conversation_id, token.clone());
    tokio::spawn(async move {
        run_agent(app, state.clone(), conv, settings, token).await;
        state.cancels.lock().unwrap().remove(&conversation_id);
    });
}

#[tauri::command]
pub fn stop_generation(state: State<Arc<AppState>>, conversation_id: i64) -> Result<(), String> {
    if let Some(token) = state.cancels.lock().unwrap().get(&conversation_id) {
        token.cancel();
    }
    Ok(())
}

/// 测试 DeepSeek API 连接（Anthropic Messages 端点），返回成功或具体错误
#[tauri::command]
pub async fn test_deepseek_connection(
    state: State<'_, Arc<AppState>>,
    api_key: String,
) -> Result<String, String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err("未配置 DeepSeek API Key，请先在设置面板中填写".into());
    }
    let url = format!("{DEEPSEEK_ANTHROPIC_BASE}/v1/messages");
    let body = serde_json::json!({
        "model": "deepseek-v4-flash",
        "max_tokens": 8,
        "messages": [{"role": "user", "content": "hi"}],
        "stream": false,
    });
    let resp = state
        .client
        .post(url)
        .header("x-api-key", key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if status.is_success() {
        Ok("连接成功，API Key 有效，可正常使用 deepseek-v4-flash / deepseek-v4-pro".into())
    } else {
        Err(crate::llm::anthropic::api_error(status.as_u16(), &text))
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
    ips.into_iter().any(|ip| match ip {
        std::net::IpAddr::V4(v4) => {
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
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xfe00 == 0xfc00)
                || v6.segments()[0] == 0xfe80
        }
    })
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
    if is_ssrf_target(&url) {
        return err("出于安全考虑，仅允许访问公网地址");
    }
    let resp = state
        .client
        .get(&url)
        .header(
            "user-agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
        )
        .header("accept-language", "zh-CN,zh;q=0.9,en;q=0.8")
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| format!("网页请求失败: {e}"))?;
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
        .map_err(|e| format!("读取网页内容失败: {e}"))?;
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
    out.push_str(&escape_attr(&url));
    out.push_str(
        "\"><style>img{max-width:100%;height:auto}table{border-collapse:collapse;width:100%;margin:8px 0}th,td{border:1px solid #ccc;padding:5px 8px;text-align:left}body{font-family:-apple-system,'Segoe UI','PingFang SC','Microsoft YaHei',sans-serif;margin:0;padding:14px;line-height:1.7;font-size:14px;color:#1c2027;word-break:break-word}a{color:#4d6bfe;text-decoration:none}pre{background:#f5f6f8;padding:10px;border-radius:6px;overflow-x:auto}code{background:#f5f6f8;border-radius:4px;padding:1px 5px}</style></head><body>",
    );
    out.push_str(&body_buf);
    out.push_str("</body></html>");
    if out.len() < 250 {
        return Err("未能提取到网页正文内容".into());
    }
    Ok(WebPage { url, title, html: out })
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
