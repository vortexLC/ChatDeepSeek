use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use crate::models::*;

pub const CONTEXT_TOTAL_TOKENS: u64 = 1_000_000;
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
///           ├── images/        —— 图片产物
///           └── videos/        —— 视频产物
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
        let _ = fs::create_dir_all(dir.join("videos"));
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
                    if let Ok(json) = serde_json::to_string_pretty(&conv) {
                        let _ = fs::write(self.session_json_path(id), json);
                    }
                    let _ = fs::remove_file(&legacy);
                }
            }
        }
    }

    fn open_session_conn(&self, id: i64) -> Result<Arc<Mutex<Connection>>, String> {
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

    fn write_session(&self, conv: &Conversation) {
        if let Ok(json) = serde_json::to_string_pretty(conv) {
            let _ = fs::write(self.session_json_path(conv.id), json);
        }
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

    pub fn create_conversation(&self, settings: &AppSettings) -> Conversation {
        let _lock = self.session_lock.lock().unwrap();
        let ts = now_ms();
        let mut id = ts;
        while self.session_dir(id).exists() || self.legacy_session_path(id).exists() {
            id += 1;
        }
        let conv = Conversation {
            id,
            title: "新对话".into(),
            provider: "anthropic".into(),
            model: settings.default_model.clone(),
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
        };
        self.ensure_session_dirs(id);
        self.write_session(&conv);
        conv
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
        self.write_session(&conv);
        Ok(())
    }

    pub fn delete_conversation(&self, id: i64) -> Result<(), String> {
        let _lock = self.session_lock.lock().unwrap();
        let _ = fs::remove_dir_all(self.session_dir(id));
        let _ = fs::remove_file(self.legacy_session_path(id));
        Ok(())
    }

    pub fn clear_all(&self) -> Result<(), String> {
        let _lock = self.session_lock.lock().unwrap();
        if let Ok(rd) = fs::read_dir(&self.sessions_dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let _ = fs::remove_dir_all(&p);
                } else {
                    let _ = fs::remove_file(&p);
                }
            }
        }
        Ok(())
    }

    pub fn touch(&self, id: i64) {
        let _lock = self.session_lock.lock().unwrap();
        if let Some(mut conv) = self.read_session(id) {
            conv.updated_at = now_ms();
            self.write_session(&conv);
        }
    }

    pub fn set_title_if_default(&self, id: i64, content: &str) {
        let _lock = self.session_lock.lock().unwrap();
        if let Some(mut conv) = self.read_session(id) {
            if conv.title == "新对话" || conv.title.is_empty() {
                conv.title = content.chars().take(30).collect();
                self.write_session(&conv);
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

    pub fn session_videos_dir(&self, id: i64) -> PathBuf {
        self.session_dir(id).join("videos")
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
        let items = serde_json::from_str(&search_results).unwrap_or_default();
        let arts = serde_json::from_str(&artifacts).unwrap_or_default();
        Ok(Message {
            id: row.get(0)?,
            conversation_id: row.get(1)?,
            role: row.get(2)?,
            content: row.get(3)?,
            reasoning: row.get(4)?,
            search_results: items,
            artifacts: arts,
            created_at: row.get(5)?,
        })
    }

    pub fn list_messages(&self, conv_id: i64) -> Result<Vec<Message>, String> {
        let conn = self.open_session_conn(conv_id)?;
        let conn = conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, conversation_id, role, content, reasoning, created_at, search_results, artifacts
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
                "SELECT id, conversation_id, role, content, reasoning, tool_calls, tool_results, search_results, artifacts, created_at
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
                    created_at: r.get(9)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    pub fn get_message(&self, conv_id: i64, message_id: i64) -> Option<DbMessageRow> {
        let conn = self.open_session_conn(conv_id).ok()?;
        let conn = conn.lock().unwrap();
        conn.query_row(
            "SELECT id, conversation_id, role, content, reasoning, tool_calls, tool_results, search_results, artifacts, created_at
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
                    created_at: r.get(9)?,
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
    ) -> Result<i64, String> {
        let conn = self.open_session_conn(conv_id)?;
        let conn = conn.lock().unwrap();
        let sr = serde_json::to_string(search_results).unwrap_or_else(|_| "[]".into());
        let ar = serde_json::to_string(artifacts).unwrap_or_else(|_| "[]".into());
        conn.execute(
            "INSERT INTO messages (conversation_id, role, content, reasoning, tool_calls, tool_results, search_results, artifacts, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![conv_id, role, content, reasoning, tool_calls, tool_results, sr, ar, now_ms()],
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
        fs::write(self.json_dir.join("settings.json"), json).map_err(|e| e.to_string())
    }

    // ==================== 上下文用量 ====================

    pub fn context_usage(&self, conv_id: i64) -> ContextUsage {
        let rows = self.list_messages_full(conv_id).unwrap_or_default();
        let mut used: u64 = 0;
        for r in &rows {
            used += estimate_tokens(&r.content)
                + estimate_tokens(&r.reasoning)
                + estimate_tokens(&r.tool_calls)
                + estimate_tokens(&r.tool_results);
        }
        let total = CONTEXT_TOTAL_TOKENS;
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
        }
    }
}
