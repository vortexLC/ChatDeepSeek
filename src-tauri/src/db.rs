use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use crate::models::*;

pub const CONTEXT_NEAR_FULL_RATIO: f64 = 0.9;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id INTEGER NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL DEFAULT '',
    reasoning TEXT NOT NULL DEFAULT '',
    tool_calls TEXT NOT NULL DEFAULT '[]',
    tool_results TEXT NOT NULL DEFAULT '[]',
    search_results TEXT NOT NULL DEFAULT '[]',
    artifacts TEXT NOT NULL DEFAULT '[]',
    attachments TEXT NOT NULL DEFAULT '[]',
    steps TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conversation_id, id);
";

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 估算文本的 token 数（中文等宽字符约 1 token，英文约 3 字符 1 token）
pub fn estimate_tokens(text: &str) -> u64 {
    let mut tokens: u64 = 0;
    let mut word_len: u64 = 0;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            word_len += 1;
        } else {
            if word_len > 0 {
                tokens += word_len.div_ceil(3);
                word_len = 0;
            }
            tokens += 1;
        }
    }
    if word_len > 0 {
        tokens += word_len.div_ceil(3);
    }
    tokens.max(1)
}

/// 数据根目录结构：
///   data/
///   ├── json/        —— API Key、应用设置 (settings.json)
///   └── sessions/    —— 每个会话一个项目目录：
///       └── <会话ID>/
///           ├── session.json   —— 会话元数据（标题/模型/模式/开关等）
///           ├── messages.db    —— 会话内容（消息/思考/工具调用/搜索结果/产物索引）
///           ├── files/         —— 文件产物（Build/Agent 模式生成）
///           └── images/        —— 图片产物
pub struct Db {
    session_lock: Mutex<()>,
    /// 按会话缓存 SQLite 连接，避免每次消息操作重复打开数据库
    conns: Mutex<HashMap<i64, Arc<Mutex<Connection>>>>,
    json_dir: PathBuf,
    sessions_dir: PathBuf,
}

impl Db {
    pub fn open(root: &Path) -> rusqlite::Result<Self> {
        let json_dir = root.join("json");
        let sessions_dir = root.join("sessions");
        let _ = fs::create_dir_all(&json_dir);
        let _ = fs::create_dir_all(&sessions_dir);
        Ok(Db {
            session_lock: Mutex::new(()),
            conns: Mutex::new(HashMap::new()),
            json_dir,
            sessions_dir,
        })
    }

    fn session_dir(&self, id: i64) -> PathBuf {
        self.sessions_dir.join(id.to_string())
    }

    fn session_json_path(&self, id: i64) -> PathBuf {
        self.session_dir(id).join("session.json")
    }

    fn legacy_session_path(&self, id: i64) -> PathBuf {
        self.sessions_dir.join(format!("{id}.json"))
    }

    fn session_db_path(&self, id: i64) -> PathBuf {
        self.session_dir(id).join("messages.db")
    }

    fn ensure_session_dirs(&self, id: i64) {
        let dir = self.session_dir(id);
        let _ = fs::create_dir_all(dir.join("files"));
        let _ = fs::create_dir_all(dir.join("images"));
        let _ = fs::create_dir_all(dir.join("uploads"));
    }

    /// 旧版单文件 <id>.json 自动迁移到会话目录
    fn migrate_legacy(&self, id: i64) {
        let legacy = self.legacy_session_path(id);
        if legacy.exists() && !self.session_json_path(id).exists() {
            if let Ok(content) = fs::read_to_string(&legacy) {
                if let Ok(mut conv) = serde_json::from_str::<Conversation>(&content) {
                    if conv.mode.is_empty() {
                        conv.mode = MODE_CHAT.to_string();
                    }
                    self.ensure_session_dirs(id);
                    if let Err(e) = self.write_session(&conv) {
                        log::error!("[db] 迁移旧会话 {id} 失败: {e}");
                        return;
                    }
                    let _ = fs::remove_file(&legacy);
                }
            }
        }
    }

    fn open_session_conn(&self, id: i64) -> Result<Arc<Mutex<Connection>>, String> {
        // 会话不存在（已删除/从未创建）时不重建目录：
        // 防止后台任务/残留命令把已删除会话的目录与消息库"复活"成僵尸数据
        if !self.session_json_path(id).exists() {
            return Err(format!("会话 {id} 不存在"));
        }
        if let Some(c) = self.conns.lock().unwrap().get(&id) {
            return Ok(c.clone());
        }
        self.ensure_session_dirs(id);
        let conn = Connection::open(self.session_db_path(id)).map_err(|e| e.to_string())?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
        let has_artifacts: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name='artifacts'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if !has_artifacts {
            let _ = conn.execute(
                "ALTER TABLE messages ADD COLUMN artifacts TEXT NOT NULL DEFAULT '[]'",
                [],
            );
        }
        let has_attachments: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name='attachments'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if !has_attachments {
            let _ = conn.execute(
                "ALTER TABLE messages ADD COLUMN attachments TEXT NOT NULL DEFAULT '[]'",
                [],
            );
        }
        let has_steps: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('messages') WHERE name='steps'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false);
        if !has_steps {
            let _ = conn.execute(
                "ALTER TABLE messages ADD COLUMN steps TEXT NOT NULL DEFAULT '[]'",
                [],
            );
        }
        let arc = Arc::new(Mutex::new(conn));
        self.conns.lock().unwrap().insert(id, arc.clone());
        Ok(arc)
    }

    // ==================== 会话（sessions/<id>/session.json） ====================

    fn read_session(&self, id: i64) -> Option<Conversation> {
        fs::read_to_string(self.session_json_path(id))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    }

    /// 原子写入会话元数据：先写临时文件再重命名，
    /// 避免进程崩溃时写出一半损坏的 session.json（否则整个会话不可读）
    fn write_session(&self, conv: &Conversation) -> Result<(), String> {
        let json = serde_json::to_string_pretty(conv).map_err(|e| e.to_string())?;
        let path = self.session_json_path(conv.id);
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json).map_err(|e| format!("写入会话数据失败: {e}"))?;
        fs::rename(&tmp, &path).map_err(|e| format!("保存会话数据失败: {e}"))?;
        Ok(())
    }

    pub fn list_conversations(&self) -> Vec<Conversation> {
        let _lock = self.session_lock.lock().unwrap();
        let mut list = Vec::new();
        if let Ok(rd) = fs::read_dir(&self.sessions_dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(id) = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .and_then(|s| s.parse::<i64>().ok())
                    {
                        if let Some(c) = self.read_session(id) {
                            list.push(c);
                        }
                    }
                } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
                    // 旧单文件格式：迁移后读取
                    if let Some(id) = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .and_then(|s| s.parse::<i64>().ok())
                    {
                        self.migrate_legacy(id);
                        if let Some(c) = self.read_session(id) {
                            list.push(c);
                        }
                    }
                }
            }
        }
        list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        list
    }

    pub fn get_conversation(&self, id: i64) -> Option<Conversation> {
        let _lock = self.session_lock.lock().unwrap();
        self.migrate_legacy(id);
        self.read_session(id)
    }

    pub fn create_conversation(&self, settings: &AppSettings) -> Result<Conversation, String> {
        let _lock = self.session_lock.lock().unwrap();
        let ts = now_ms();
        let mut id = ts;
        while self.session_dir(id).exists() || self.legacy_session_path(id).exists() {
            id += 1;
        }
        let conv = Conversation {
            id,
            title: "新对话".into(),
            provider: settings
                .chat_model
                .as_ref()
                .map(|s| s.provider_id.clone())
                .unwrap_or_else(|| "openai".into()),
            model: settings
                .chat_model
                .as_ref()
                .and_then(|s| settings.find_model(s))
                .map(|(_, m)| m.id.clone())
                .unwrap_or_else(|| settings.default_model.clone()),
            web_search: settings.default_web_search,
            deep_think: settings.default_deep_think,
            effort: settings.default_effort.clone(),
            mode: if settings.default_mode.is_empty() {
                MODE_CHAT.to_string()
            } else {
                settings.default_mode.clone()
            },
            created_at: ts,
            updated_at: ts,
            summary: String::new(),
            summarized_until: 0,
        };
        self.ensure_session_dirs(id);
        self.write_session(&conv)?;
        Ok(conv)
    }

    pub fn update_conversation(
        &self,
        id: i64,
        patch: &ConversationPatch,
    ) -> Result<(), String> {
        let _lock = self.session_lock.lock().unwrap();
        let mut conv = self.read_session(id).ok_or("对话不存在")?;
        if let Some(t) = &patch.title {
            conv.title = t.clone();
        }
        if let Some(p) = &patch.provider {
            conv.provider = p.clone();
        }
        if let Some(m) = &patch.model {
            conv.model = m.clone();
        }
        if let Some(w) = patch.web_search {
            conv.web_search = w;
        }
        if let Some(d) = patch.deep_think {
            conv.deep_think = d;
        }
        if let Some(e) = &patch.effort {
            conv.effort = e.clone();
        }
        if let Some(m) = &patch.mode {
            conv.mode = m.clone();
        }
        conv.updated_at = now_ms();
        self.write_session(&conv)
    }

    pub fn delete_conversation(&self, id: i64) -> Result<(), String> {
        let _lock = self.session_lock.lock().unwrap();
        // 先释放缓存的 SQLite 连接：否则 Windows 下文件句柄仍被占用，
        // remove_dir_all 会失败，导致会话目录（含消息库与产物）残留并在列表中"复活"
        self.conns.lock().unwrap().remove(&id);
        let dir = self.session_dir(id);
        if dir.exists() {
            // 后台任务（如刚取消的视频轮询器）可能短暂持有连接句柄，重试几次再报错
            let mut last_err = None;
            for _ in 0..5 {
                match fs::remove_dir_all(&dir) {
                    Ok(()) => {
                        last_err = None;
                        break;
                    }
                    Err(e) => {
                        last_err = Some(e);
                        std::thread::sleep(std::time::Duration::from_millis(200));
                    }
                }
            }
            if let Some(e) = last_err {
                return Err(format!("删除会话数据失败（目录可能被占用）：{e}"));
            }
        }
        let _ = fs::remove_file(self.legacy_session_path(id));
        Ok(())
    }

    pub fn clear_all(&self) -> Result<(), String> {
        let _lock = self.session_lock.lock().unwrap();
        // 同 delete_conversation：先关闭所有会话连接再删目录
        self.conns.lock().unwrap().clear();
        let mut failed: Vec<String> = Vec::new();
        if let Ok(rd) = fs::read_dir(&self.sessions_dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                let result = if p.is_dir() {
                    fs::remove_dir_all(&p)
                } else {
                    fs::remove_file(&p)
                };
                if let Err(e) = result {
                    failed.push(format!("{}: {e}", p.display()));
                }
            }
        }
        if !failed.is_empty() {
            return Err(format!("部分会话数据删除失败：{}", failed.join("；")));
        }
        Ok(())
    }

    pub fn touch(&self, id: i64) {
        let _lock = self.session_lock.lock().unwrap();
        if let Some(mut conv) = self.read_session(id) {
            conv.updated_at = now_ms();
            if let Err(e) = self.write_session(&conv) {
                log::error!("[db] 更新会话 {id} 时间戳失败: {e}");
            }
        }
    }

    pub fn set_title_if_default(&self, id: i64, content: &str) {
        let _lock = self.session_lock.lock().unwrap();
        if let Some(mut conv) = self.read_session(id) {
            if conv.title == "新对话" || conv.title.is_empty() {
                conv.title = content.chars().take(30).collect();
                if let Err(e) = self.write_session(&conv) {
                    log::error!("[db] 设置会话 {id} 标题失败: {e}");
                }
            }
        }
    }

    // ==================== 会话产物目录 ====================

    pub fn session_files_dir(&self, id: i64) -> PathBuf {
        self.session_dir(id).join("files")
    }

    pub fn session_images_dir(&self, id: i64) -> PathBuf {
        self.session_dir(id).join("images")
    }

    pub fn session_uploads_dir(&self, id: i64) -> PathBuf {
        self.session_dir(id).join("uploads")
    }

    /// 校验相对路径并返回会话目录内的绝对路径（组件级规范化，禁止 `..` 逃逸；绝对路径返回 None）
    pub fn session_abs_path(&self, id: i64, rel: &str) -> Option<PathBuf> {
        let dir = self.session_dir(id);
        let rel_path = Path::new(rel);
        if rel_path.is_absolute() {
            return None;
        }
        let mut components: Vec<std::path::Component> = Vec::new();
        for c in rel_path.components() {
            match c {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => return None,
                _ => components.push(c),
            }
        }
        if components.is_empty() {
            return Some(dir.clone());
        }
        let norm: PathBuf = components.iter().collect();
        Some(dir.join(norm))
    }

    // ==================== 消息（sessions/<id>/messages.db） ====================

    fn row_to_message(row: &rusqlite::Row) -> rusqlite::Result<Message> {
        let search_results: String = row.get(6)?;
        let artifacts: String = row.get(7)?;
        let attachments: String = row.get(8)?;
        let steps: String = row.get(9)?;
        let items = serde_json::from_str(&search_results).unwrap_or_default();
        let arts = serde_json::from_str(&artifacts).unwrap_or_default();
        let atts = serde_json::from_str(&attachments).unwrap_or_default();
        let sts = serde_json::from_str(&steps).unwrap_or_default();
        Ok(Message {
            id: row.get(0)?,
            conversation_id: row.get(1)?,
            role: row.get(2)?,
            content: row.get(3)?,
            reasoning: row.get(4)?,
            search_results: items,
            artifacts: arts,
            attachments: atts,
            steps: sts,
            created_at: row.get(5)?,
        })
    }

    pub fn list_messages(&self, conv_id: i64) -> Result<Vec<Message>, String> {
        let conn = self.open_session_conn(conv_id)?;
        let conn = conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, conversation_id, role, content, reasoning, created_at, search_results, artifacts, attachments, steps
                 FROM messages WHERE conversation_id = ?1 ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![conv_id], |r| Self::row_to_message(r))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    pub fn list_messages_full(&self, conv_id: i64) -> Result<Vec<DbMessageRow>, String> {
        let conn = self.open_session_conn(conv_id)?;
        let conn = conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, conversation_id, role, content, reasoning, tool_calls, tool_results, search_results, artifacts, attachments, steps, created_at
                 FROM messages WHERE conversation_id = ?1 ORDER BY id",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![conv_id], |r| {
                Ok(DbMessageRow {
                    id: r.get(0)?,
                    conversation_id: r.get(1)?,
                    role: r.get(2)?,
                    content: r.get(3)?,
                    reasoning: r.get(4)?,
                    tool_calls: r.get(5)?,
                    tool_results: r.get(6)?,
                    search_results: r.get(7)?,
                    artifacts: r.get(8)?,
                    attachments: r.get(9)?,
                    steps: r.get(10)?,
                    created_at: r.get(11)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    pub fn get_message(&self, conv_id: i64, message_id: i64) -> Option<DbMessageRow> {
        let conn = self.open_session_conn(conv_id).ok()?;
        let conn = conn.lock().unwrap();
        conn.query_row(
            "SELECT id, conversation_id, role, content, reasoning, tool_calls, tool_results, search_results, artifacts, attachments, steps, created_at
             FROM messages WHERE id = ?1 AND conversation_id = ?2",
            params![message_id, conv_id],
            |r| {
                Ok(DbMessageRow {
                    id: r.get(0)?,
                    conversation_id: r.get(1)?,
                    role: r.get(2)?,
                    content: r.get(3)?,
                    reasoning: r.get(4)?,
                    tool_calls: r.get(5)?,
                    tool_results: r.get(6)?,
                    search_results: r.get(7)?,
                    artifacts: r.get(8)?,
                    attachments: r.get(9)?,
                    steps: r.get(10)?,
                    created_at: r.get(11)?,
                })
            },
        )
        .ok()
    }

    pub fn delete_messages_from(&self, conv_id: i64, message_id: i64) -> Result<(), String> {
        let conn = self.open_session_conn(conv_id)?;
        let conn = conn.lock().unwrap();
        conn.execute(
            "DELETE FROM messages WHERE conversation_id = ?1 AND id >= ?2",
            params![conv_id, message_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_message(
        &self,
        conv_id: i64,
        role: &str,
        content: &str,
        reasoning: &str,
        tool_calls: &str,
        tool_results: &str,
        search_results: &[SearchItem],
        artifacts: &[Artifact],
        steps: &[ToolStep],
    ) -> Result<i64, String> {
        self.insert_message_with_attachments(
            conv_id, role, content, reasoning, tool_calls, tool_results, search_results, artifacts,
            steps, &[],
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_message_with_attachments(
        &self,
        conv_id: i64,
        role: &str,
        content: &str,
        reasoning: &str,
        tool_calls: &str,
        tool_results: &str,
        search_results: &[SearchItem],
        artifacts: &[Artifact],
        steps: &[ToolStep],
        attachments: &[crate::models::Attachment],
    ) -> Result<i64, String> {
        let conn = self.open_session_conn(conv_id)?;
        let conn = conn.lock().unwrap();
        let sr = serde_json::to_string(search_results).unwrap_or_else(|_| "[]".into());
        let ar = serde_json::to_string(artifacts).unwrap_or_else(|_| "[]".into());
        let at = serde_json::to_string(attachments).unwrap_or_else(|_| "[]".into());
        let st = serde_json::to_string(steps).unwrap_or_else(|_| "[]".into());
        conn.execute(
            "INSERT INTO messages (conversation_id, role, content, reasoning, tool_calls, tool_results, search_results, artifacts, attachments, steps, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![conv_id, role, content, reasoning, tool_calls, tool_results, sr, ar, at, st, now_ms()],
        )
        .map_err(|e| e.to_string())?;
        Ok(conn.last_insert_rowid())
    }

    // ==================== 设置（json/settings.json） ====================

    pub fn get_settings(&self) -> AppSettings {
        let path = self.json_dir.join("settings.json");
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_settings(&self, settings: &AppSettings) -> Result<(), String> {
        let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
        // 原子写入：先写临时文件再重命名，避免崩溃时设置文件损坏导致全部配置丢失
        let path = self.json_dir.join("settings.json");
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json).map_err(|e| format!("写入设置失败: {e}"))?;
        fs::rename(&tmp, &path).map_err(|e| format!("保存设置失败: {e}"))?;
        Ok(())
    }

    // ==================== 上下文用量 ====================

    /// 按当前对话模型的上下文容量估算用量；
    /// 已自动压缩（summarized_until 之前的消息）只计摘要文本
    pub fn context_usage(&self, conv_id: i64) -> ContextUsage {
        let settings = self.get_settings();
        let conv = self.get_conversation(conv_id);
        let total = conv
            .as_ref()
            .map(|c| settings.chat_context_total(c))
            .unwrap_or(crate::models::CONTEXT_DEFAULT_TOKENS);
        let summary = conv.as_ref().map(|c| c.summary.clone()).unwrap_or_default();
        let summarized_until = conv.as_ref().map(|c| c.summarized_until).unwrap_or(0);
        let compressed = !summary.is_empty();

        // 请求固定开销：system prompt + 工具 JSON 定义（Agent 模式更高），
        // 不计入会低估用量、延迟压缩触发
        let overhead = match conv.as_ref().map(|c| c.mode.as_str()) {
            Some("agent") => crate::models::CONTEXT_OVERHEAD_AGENT,
            _ => crate::models::CONTEXT_OVERHEAD_CHAT,
        };
        let mut used: u64 = overhead + estimate_tokens(&summary);
        let rows = self.list_messages_full(conv_id).unwrap_or_default();
        for r in &rows {
            if r.id <= summarized_until {
                continue;
            }
            used += estimate_tokens(&r.content)
                + estimate_tokens(&r.reasoning)
                + estimate_tokens(&r.tool_calls)
                + estimate_tokens(&r.tool_results);
            // 附件：图片按固定 token 估算，文档按字节数估算
            // （与 llm/mod.rs 的 MAX_DOC_CHARS=20000 截断一致，文档实际只会发送
            //  截断后的前 20000 字符，估算也按此上限，避免上传大文档撑爆上下文估算）
            if let Ok(atts) = serde_json::from_str::<Vec<crate::models::Attachment>>(&r.attachments)
            {
                for a in atts {
                    if a.kind == "image" {
                        used += crate::models::CONTEXT_IMAGE_TOKENS;
                    } else {
                        used += ((a.size.max(0) as u64) / 4).min(20_000);
                    }
                }
            }
        }
        let percent = if total == 0 {
            0.0
        } else {
            (used as f64 / total as f64).min(1.0)
        };
        ContextUsage {
            used_tokens: used,
            total_tokens: total,
            percent,
            near_full: used as f64 >= total as f64 * CONTEXT_NEAR_FULL_RATIO,
            full: used >= total,
            compressed,
        }
    }

    /// 记录上下文压缩结果：早期对话摘要与已摘要到的最大消息 id
    pub fn update_conversation_summary(
        &self,
        conv_id: i64,
        summary: &str,
        summarized_until: i64,
    ) -> Result<(), String> {
        let _lock = self.session_lock.lock().unwrap();
        if let Some(mut conv) = self.read_session(conv_id) {
            conv.summary = summary.to_string();
            conv.summarized_until = summarized_until;
            conv.updated_at = now_ms();
            self.write_session(&conv)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        // 进程内自增序号 + 时间戳，避免同一毫秒内多个测试目录冲突
        let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("cds_db_test_{}_{seq}", now_ms()));
        let _ = fs::create_dir_all(&d);
        d
    }

    /// 数据保存闭环：建会话 → 写消息 → 读回 → 更新元数据 → 删除会话
    #[test]
    fn session_save_roundtrip() {
        let root = temp_root();
        let db = Db::open(&root).unwrap();
        let settings = AppSettings::default();

        // 创建会话并原子落盘
        let conv = db.create_conversation(&settings).expect("创建会话失败");
        assert!(db.get_conversation(conv.id).is_some(), "会话应立即可读");
        // 原子写不应残留临时文件
        let tmp = db.session_json_path(conv.id).with_extension("json.tmp");
        assert!(!tmp.exists(), "原子写后不应残留 .tmp 文件");

        // 写入并读回消息（含附件）
        let att = Attachment {
            name: "a.png".into(),
            mime: "image/png".into(),
            kind: "image".into(),
            path: "uploads/att_1.png".into(),
            size: 10,
        };
        let mid = db
            .insert_message_with_attachments(
                conv.id, "user", "你好", "", "[]", "[]", &[], &[], &[], &[att],
            )
            .expect("插入消息失败");
        let msgs = db.list_messages(conv.id).expect("读取消息失败");
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "你好");
        assert_eq!(msgs[0].id, mid);
        assert_eq!(msgs[0].attachments.len(), 1);
        assert_eq!(msgs[0].attachments[0].path, "uploads/att_1.png");

        // 元数据更新（标题/摘要）持久化
        db.set_title_if_default(conv.id, "测试标题内容");
        db.update_conversation_summary(conv.id, "摘要", mid).unwrap();
        let updated = db.get_conversation(conv.id).unwrap();
        assert_eq!(updated.title, "测试标题内容");
        assert_eq!(updated.summary, "摘要");
        assert_eq!(updated.summarized_until, mid);

        // 删除会话：目录应被移除，之后的消息读取应报"会话不存在"
        db.delete_conversation(conv.id).expect("删除会话失败");
        assert!(db.get_conversation(conv.id).is_none(), "删除后会话不可读");
        assert!(!db.session_dir(conv.id).exists(), "删除后目录不存在");
        assert!(db.list_messages(conv.id).is_err(), "删除后读取消息应报错");

        let _ = fs::remove_dir_all(&root);
    }

    /// 删除不存在的会话：应正常返回，不报错
    #[test]
    fn delete_missing_session_is_ok() {
        let root = temp_root();
        let db = Db::open(&root).unwrap();
        assert!(db.delete_conversation(999_999_999).is_ok());
        assert!(db.clear_all().is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    /// 设置原子保存：写入后可读回
    #[test]
    fn settings_save_roundtrip() {
        let root = temp_root();
        let db = Db::open(&root).unwrap();
        let mut s = AppSettings::default();
        s.theme = "dark".into();
        db.save_settings(&s).unwrap();
        let loaded = db.get_settings();
        assert_eq!(loaded.theme, "dark");
        let tmp = db.json_dir.join("settings.json.tmp");
        assert!(!tmp.exists(), "设置原子写后不应残留 .tmp 文件");
        let _ = fs::remove_dir_all(&root);
    }
}
