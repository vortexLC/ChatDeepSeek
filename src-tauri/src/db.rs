use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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
///   ├── db/          —— SQLite 数据库（会话上下文记忆）
///   └── sessions/    —— 会话数据（每会话一个 <id>.json）
pub struct Db {
    conn: Mutex<Connection>,
    session_lock: Mutex<()>,
    json_dir: PathBuf,
    sessions_dir: PathBuf,
}

impl Db {
    pub fn open(root: &Path) -> rusqlite::Result<Self> {
        let json_dir = root.join("json");
        let db_dir = root.join("db");
        let sessions_dir = root.join("sessions");
        let _ = fs::create_dir_all(&json_dir);
        let _ = fs::create_dir_all(&db_dir);
        let _ = fs::create_dir_all(&sessions_dir);
        let conn = Connection::open(db_dir.join("chatdeepseek.db"))?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.execute_batch(SCHEMA)?;
        Ok(Db {
            conn: Mutex::new(conn),
            session_lock: Mutex::new(()),
            json_dir,
            sessions_dir,
        })
    }

    fn session_path(&self, id: i64) -> PathBuf {
        self.sessions_dir.join(format!("{id}.json"))
    }

    // ==================== 会话（sessions/*.json） ====================

    fn read_session(&self, id: i64) -> Option<Conversation> {
        fs::read_to_string(self.session_path(id))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    }

    fn write_session(&self, conv: &Conversation) {
        if let Ok(json) = serde_json::to_string_pretty(conv) {
            let _ = fs::write(self.session_path(conv.id), json);
        }
    }

    pub fn list_conversations(&self) -> Vec<Conversation> {
        let _lock = self.session_lock.lock().unwrap();
        let mut list = Vec::new();
        if let Ok(rd) = fs::read_dir(&self.sessions_dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(c) = serde_json::from_str::<Conversation>(&content) {
                        list.push(c);
                    }
                }
            }
        }
        list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        list
    }

    pub fn get_conversation(&self, id: i64) -> Option<Conversation> {
        let _lock = self.session_lock.lock().unwrap();
        self.read_session(id)
    }

    pub fn create_conversation(&self, settings: &AppSettings) -> Conversation {
        let _lock = self.session_lock.lock().unwrap();
        let ts = now_ms();
        let mut id = ts;
        while self.session_path(id).exists() {
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
            created_at: ts,
            updated_at: ts,
        };
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
        conv.updated_at = now_ms();
        self.write_session(&conv);
        Ok(())
    }

    pub fn delete_conversation(&self, id: i64) -> Result<(), String> {
        let _lock = self.session_lock.lock().unwrap();
        let _ = fs::remove_file(self.session_path(id));
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM messages WHERE conversation_id = ?1",
            params![id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn clear_all(&self) -> Result<(), String> {
        let _lock = self.session_lock.lock().unwrap();
        if let Ok(rd) = fs::read_dir(&self.sessions_dir) {
            for entry in rd.flatten() {
                let _ = fs::remove_file(entry.path());
            }
        }
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM messages", []).map_err(|e| e.to_string())?;
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

    // ==================== 消息（db/chatdeepseek.db） ====================

    fn row_to_message(row: &rusqlite::Row) -> rusqlite::Result<Message> {
        let search_results: String = row.get(6)?;
        let items = serde_json::from_str(&search_results).unwrap_or_default();
        Ok(Message {
            id: row.get(0)?,
            conversation_id: row.get(1)?,
            role: row.get(2)?,
            content: row.get(3)?,
            reasoning: row.get(4)?,
            search_results: items,
            created_at: row.get(5)?,
        })
    }

    pub fn list_messages(&self, conv_id: i64) -> rusqlite::Result<Vec<Message>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, role, content, reasoning, created_at, search_results
             FROM messages WHERE conversation_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![conv_id], |r| Self::row_to_message(r))?;
        rows.collect()
    }

    pub fn list_messages_full(&self, conv_id: i64) -> rusqlite::Result<Vec<DbMessageRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, role, content, reasoning, tool_calls, tool_results, search_results, created_at
             FROM messages WHERE conversation_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![conv_id], |r| {
            Ok(DbMessageRow {
                id: r.get(0)?,
                conversation_id: r.get(1)?,
                role: r.get(2)?,
                content: r.get(3)?,
                reasoning: r.get(4)?,
                tool_calls: r.get(5)?,
                tool_results: r.get(6)?,
                search_results: r.get(7)?,
                created_at: r.get(8)?,
            })
        })?;
        rows.collect()
    }

    pub fn get_message(&self, conv_id: i64, message_id: i64) -> Option<DbMessageRow> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id, conversation_id, role, content, reasoning, tool_calls, tool_results, search_results, created_at
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
                    created_at: r.get(8)?,
                })
            },
        )
        .ok()
    }

    pub fn delete_messages_from(&self, conv_id: i64, message_id: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
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
    ) -> Result<i64, String> {
        let conn = self.conn.lock().unwrap();
        let sr = serde_json::to_string(search_results).unwrap_or_else(|_| "[]".into());
        conn.execute(
            "INSERT INTO messages (conversation_id, role, content, reasoning, tool_calls, tool_results, search_results, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![conv_id, role, content, reasoning, tool_calls, tool_results, sr, now_ms()],
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
        let json =
            serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
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
