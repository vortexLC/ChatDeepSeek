use std::path::{Path, PathBuf};

use serde_json::json;
use tauri::AppHandle;

use crate::commands::AppState;
use crate::llm::CancelToken;
use crate::models::{Artifact, SearchItem};

pub const TOOL_WEB_SEARCH: &str = "web_search";
pub const TOOL_READ_FILE: &str = "read_file";
pub const TOOL_WRITE_FILE: &str = "write_file";
pub const TOOL_EDIT_FILE: &str = "edit_file";
pub const TOOL_DELETE_FILE: &str = "delete_file";
pub const TOOL_LIST_FILES: &str = "list_files";
pub const TOOL_GLOB: &str = "glob";
pub const TOOL_GREP: &str = "grep";
pub const TOOL_BASH: &str = "bash";
pub const TOOL_GENERATE_IMAGE: &str = "generate_image";

pub struct ToolOutcome {
    pub content: String,
    pub artifacts: Vec<Artifact>,
    /// web_search 返回的原始搜索结果（持久化后供前端展示搜索来源卡片）
    pub search_items: Vec<SearchItem>,
}

/// 按会话模式构建工具集合（OpenAI 兼容格式）
/// chat：联网搜索（可选）+ 图片生成；agent：文件/编程工具 + 图片生成（+ 联网搜索）
pub fn tools_for_mode(mode: &str, web_search: bool) -> Vec<serde_json::Value> {
    let mut tools: Vec<serde_json::Value> = Vec::new();
    if web_search {
        tools.push(web_search_tool());
    }
    match mode {
        crate::models::MODE_AGENT => {
            tools.extend(file_tools());
            tools.push(generate_image_tool());
        }
        // chat 模式（含图片生成）
        _ => {
            tools.push(generate_image_tool());
        }
    }
    tools
}

/// 分发执行工具调用
pub async fn execute_tool(
    app: &AppHandle,
    state: &std::sync::Arc<AppState>,
    conv_id: i64,
    name: &str,
    arguments: &str,
    settings: &crate::models::AppSettings,
    token: &CancelToken,
) -> Result<ToolOutcome, String> {
    let outcome = match name {
        TOOL_WEB_SEARCH => {
            let outcome = crate::llm::search::execute_search(
                app, state, conv_id, arguments, &settings.search, token,
            )
            .await?;
            Ok(ToolOutcome {
                content: outcome.summary,
                artifacts: Vec::new(),
                search_items: outcome.items,
            })
        }
        TOOL_READ_FILE => Ok(ToolOutcome {
            content: read_file(app, state, conv_id, arguments).await,
            artifacts: Vec::new(),
            search_items: Vec::new(),
        }),
        TOOL_WRITE_FILE => Ok(ToolOutcome {
            content: write_file(app, state, conv_id, arguments).await,
            artifacts: Vec::new(),
            search_items: Vec::new(),
        }),
        TOOL_EDIT_FILE => Ok(ToolOutcome {
            content: edit_file(app, state, conv_id, arguments).await,
            artifacts: Vec::new(),
            search_items: Vec::new(),
        }),
        TOOL_DELETE_FILE => Ok(ToolOutcome {
            content: delete_file(app, state, conv_id, arguments).await,
            artifacts: Vec::new(),
            search_items: Vec::new(),
        }),
        TOOL_LIST_FILES => Ok(ToolOutcome {
            content: list_files(app, state, conv_id, arguments).await,
            artifacts: Vec::new(),
            search_items: Vec::new(),
        }),
        TOOL_GLOB => Ok(ToolOutcome {
            content: glob_files(state, conv_id, arguments),
            artifacts: Vec::new(),
            search_items: Vec::new(),
        }),
        TOOL_GREP => Ok(ToolOutcome {
            content: grep_files(state, conv_id, arguments),
            artifacts: Vec::new(),
            search_items: Vec::new(),
        }),
        TOOL_BASH => {
            let output = run_bash(app, state, conv_id, arguments, token).await?;
            Ok(ToolOutcome {
                content: output,
                artifacts: Vec::new(),
                search_items: Vec::new(),
            })
        }
        TOOL_GENERATE_IMAGE => {
            let (content, artifacts) = crate::agent::generate::generate_image(
                app, state, settings, conv_id, arguments, token,
            )
            .await?;
            Ok(ToolOutcome {
                content,
                artifacts,
                search_items: Vec::new(),
            })
        }
        _ => Err(format!("未知工具: {name}")),
    };
    // 统一截断超长工具输出，保护上下文容量
    let mut outcome = outcome?;
    const MAX_TOOL_OUTPUT: usize = 6000;
    if outcome.content.chars().count() > MAX_TOOL_OUTPUT {
        let truncated: String = outcome.content.chars().take(MAX_TOOL_OUTPUT).collect();
        let total = outcome.content.chars().count();
        outcome.content = format!("{truncated}\n\n[输出过长，已截断（共 {total} 字符，仅保留前 {MAX_TOOL_OUTPUT} 字符）]");
    }
    Ok(outcome)
}

// ==================== 文件工具 ====================

/// 路径解析与权限：
/// - 工作区根为会话 files/ 目录（与 bash 沙箱一致）：目录内相对路径允许直接访问
/// - 绝对路径或以 `..` 逃逸出工作区的路径：默认不允许，需用户确认后才可访问
async fn resolve_path(
    app: &AppHandle,
    state: &AppState,
    conv_id: i64,
    rel: &str,
    tool: &str,
) -> Result<PathBuf, String> {
    use std::path::Component;
    let base = state.db.session_files_dir(conv_id);
    let rel_path = Path::new(rel);

    if rel_path.is_absolute() {
        crate::commands::request_permission(app, state, conv_id, tool, rel).await?;
        return Ok(rel_path.to_path_buf());
    }

    // 组件级规范化：处理 . 与 ..
    let mut stack: Vec<Component> = Vec::new();
    for c in rel_path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if stack.is_empty() {
                    stack.push(c);
                } else {
                    stack.pop();
                }
            }
            other => stack.push(other),
        }
    }
    let escaped = stack
        .iter()
        .any(|c| matches!(c, Component::ParentDir));
    let norm: PathBuf = stack.iter().collect();
    if escaped {
        // 逃逸出工作区：需用户确认
        crate::commands::request_permission(app, state, conv_id, tool, rel).await?;
        return Ok(base.join(norm));
    }
    Ok(base.join(norm))
}

async fn read_file(app: &AppHandle, state: &AppState, conv_id: i64, arguments: &str) -> String {
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let path = args["path"].as_str().unwrap_or("").trim();
    if path.is_empty() {
        return "read_file 缺少 path 参数".into();
    }
    let p = match resolve_path(app, state, conv_id, path, TOOL_READ_FILE).await {
        Ok(p) => p,
        Err(e) => return e,
    };
    if !p.is_file() {
        return format!("文件不存在: {path}");
    }
    let content = match std::fs::read_to_string(&p) {
        Ok(c) => c,
        Err(e) => return format!("读取文件失败: {e}"),
    };
    // 截断过长输出
    let truncated = content.chars().take(16000).collect::<String>();
    let more = if content.chars().count() > 16000 { "\n...(内容过长已截断)".to_string() } else { String::new() };
    format!("【文件 {path}】（{} 字符）：\n{truncated}{more}", content.chars().count())
}

async fn write_file(app: &AppHandle, state: &AppState, conv_id: i64, arguments: &str) -> String {
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let path = args["path"].as_str().unwrap_or("").trim();
    let content = args["content"].as_str().unwrap_or("");
    if path.is_empty() {
        return "write_file 缺少 path 参数".into();
    }
    let p = match resolve_path(app, state, conv_id, path, TOOL_WRITE_FILE).await {
        Ok(p) => p,
        Err(e) => return e,
    };
    if let Some(parent) = p.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return format!("创建目录失败: {e}");
        }
    }
    match std::fs::write(&p, content) {
        Ok(_) => format!("已写入文件 {path}（{} 字节）", content.len()),
        Err(e) => format!("写入文件失败: {e}"),
    }
}

async fn edit_file(app: &AppHandle, state: &AppState, conv_id: i64, arguments: &str) -> String {
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let path = args["path"].as_str().unwrap_or("").trim();
    let old_string = args["old_string"].as_str().unwrap_or("");
    let new_string = args["new_string"].as_str().unwrap_or("");
    if path.is_empty() || old_string.is_empty() {
        return "edit_file 需要 path 与 old_string 参数".into();
    }
    let p = match resolve_path(app, state, conv_id, path, TOOL_EDIT_FILE).await {
        Ok(p) => p,
        Err(e) => return e,
    };
    let content = match std::fs::read_to_string(&p) {
        Ok(c) => c,
        Err(e) => return format!("读取文件失败: {e}"),
    };
    if !content.contains(old_string) {
        return format!("未在文件 {path} 中找到要替换的内容（old_string 不存在）");
    }
    let updated = content.replacen(old_string, new_string, 1);
    match std::fs::write(&p, &updated) {
        Ok(_) => format!("已修改文件 {path}：替换了 {} 字符", new_string.len()),
        Err(e) => format!("写入文件失败: {e}"),
    }
}

async fn delete_file(app: &AppHandle, state: &AppState, conv_id: i64, arguments: &str) -> String {
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let path = args["path"].as_str().unwrap_or("").trim();
    if path.is_empty() {
        return "delete_file 缺少 path 参数".into();
    }
    let p = match resolve_path(app, state, conv_id, path, TOOL_DELETE_FILE).await {
        Ok(p) => p,
        Err(e) => return e,
    };
    if !p.exists() {
        return format!("文件不存在: {path}");
    }
    match std::fs::remove_file(&p) {
        Ok(_) => format!("已删除文件 {path}"),
        Err(e) => {
            // 尝试删除空目录
            if std::fs::remove_dir(&p).is_ok() {
                format!("已删除目录 {path}")
            } else {
                format!("删除失败: {e}")
            }
        }
    }
}

async fn list_files(app: &AppHandle, state: &AppState, conv_id: i64, arguments: &str) -> String {
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let dir = args["dir"].as_str().unwrap_or("files").trim();
    let p = if dir.is_empty() || dir == "." {
        state.db.session_files_dir(conv_id)
    } else {
        match resolve_path(app, state, conv_id, dir, TOOL_LIST_FILES).await {
            Ok(p) => p,
            Err(e) => return e,
        }
    };
    if !p.is_dir() {
        return format!("目录不存在: {dir}");
    }
    let base = state.db.session_files_dir(conv_id);
    let mut out = String::from("【会话文件目录】\n");
    let _ = walk_files(&p, &base, 0, &mut out, 2);
    if out == "【会话文件目录】\n" {
        out.push_str("（空目录）");
    }
    out
}

fn walk_files(dir: &Path, base: &Path, depth: usize, out: &mut String, max_depth: usize) -> std::io::Result<()> {
    if depth > max_depth {
        return Ok(());
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        if path.is_dir() {
            out.push_str(&format!("{}{}/\n", "  ".repeat(depth), e.file_name().to_string_lossy()));
            walk_files(&path, base, depth + 1, out, max_depth)?;
        } else {
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            out.push_str(&format!("{}{}  ({} B)\n", "  ".repeat(depth), e.file_name().to_string_lossy(), size));
        }
    }
    Ok(())
}

fn glob_files(state: &AppState, conv_id: i64, arguments: &str) -> String {
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let pattern = args["pattern"].as_str().unwrap_or("").trim();
    if pattern.is_empty() {
        return "glob 缺少 pattern 参数".into();
    }
    let base = state.db.session_files_dir(conv_id);
    let mut matches = Vec::new();
    for entry in walkdir_collect(&base) {
        let rel = entry.strip_prefix(&base).map(|r| r.display().to_string()).unwrap_or_default();
        if glob_match(pattern, &rel) {
            matches.push(rel);
        }
    }
    matches.sort();
    if matches.is_empty() {
        return format!("没有匹配 {pattern} 的文件");
    }
    format!("匹配到 {} 个文件：\n{}", matches.len(), matches.join("\n"))
}

/// 支持 `*`（单段内任意）与 `**`（跨目录任意）的 glob 匹配
fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut dstar) = (None::<usize>, None::<usize>);
    let (mut mark_star, mut mark_dstar) = (0usize, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi + 1 < p.len() && p[pi] == '*' && p[pi + 1] == '*' {
            dstar = Some(pi);
            mark_dstar = ti;
            pi += 2;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark_star = ti;
            pi += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark_star += 1;
            ti = mark_star;
        } else if let Some(dp) = dstar {
            pi = dp + 2;
            mark_dstar += 1;
            ti = mark_dstar;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn grep_files(state: &AppState, conv_id: i64, arguments: &str) -> String {
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let pattern = args["pattern"].as_str().unwrap_or("").trim();
    if pattern.is_empty() {
        return "grep 缺少 pattern 参数".into();
    }
    let base = state.db.session_files_dir(conv_id);
    let mut out = String::new();
    let mut count = 0usize;
    for entry in walkdir_collect(&base) {
        if !entry.is_file() {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&entry) {
            for (i, line) in content.lines().enumerate() {
                if line.contains(pattern) {
                    let rel = entry
                        .strip_prefix(&base)
                        .map(|r| r.display().to_string())
                        .unwrap_or_default();
                    out.push_str(&format!("{}:{i}: {}\n", rel, line.chars().take(160).collect::<String>()));
                    count += 1;
                    if count >= 100 {
                        out.push_str("...(结果过多已截断)");
                        return out;
                    }
                }
            }
        }
    }
    if count == 0 {
        format!("未找到匹配 {pattern} 的内容")
    } else {
        format!("找到 {count} 处匹配：\n{out}")
    }
}

fn walkdir_collect(base: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(base) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walkdir_collect(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// 检测 bash 命令是否可能访问工作区之外（盘符路径 / UNC / cd /d 切换目录）
/// 检测到则需用户确认后才能执行
fn command_touches_outside(command: &str) -> bool {
    let lower = command.to_lowercase();
    lower.contains(":\\")
        || lower.contains("\\\\")
        || lower.contains("cd /d ")
        || lower.contains("cd\\")
        || lower.contains("%userprofile%")
        || lower.contains("%appdata%")
        || lower.contains("c:\\windows")
}

// ==================== bash 沙箱 ====================

/// 在会话 files 目录内以 cmd 执行命令（AppContainer 操作系统级隔离沙箱）
async fn run_bash(
    app: &AppHandle,
    state: &AppState,
    conv_id: i64,
    arguments: &str,
    token: &CancelToken,
) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    let command = args["command"].as_str().unwrap_or("").trim();
    if command.is_empty() {
        return Err("bash 缺少 command 参数".into());
    }
    // 命令可能访问工作区之外（盘符/UNC/环境变量路径等）：需用户确认
    if command_touches_outside(command) {
        crate::commands::request_permission(app, state, conv_id, TOOL_BASH, command).await?;
    }
    let cwd = state.db.session_files_dir(conv_id);
    let _ = std::fs::create_dir_all(&cwd);

    // 初始化 AppContainer 沙箱（失败则拒绝执行，安全第一）
    let sandbox = crate::agent::sandbox::ContainerSandbox::for_session(conv_id)?;
    if let Err(e) = sandbox.grant_access(&cwd) {
        return Err(format!("沙箱初始化失败：{e}（已拒绝执行命令）"));
    }

    // 命令在阻塞线程中运行（std Command + AppContainer 属性），外层处理取消与超时
    let cmd_str = command.to_string();
    let cwd2 = cwd.clone();
    let handle = tokio::task::spawn_blocking(move || {
        let sb = sandbox;
        match sb.run(&cwd2, &cmd_str) {
            Ok((out, err, code)) => Ok(crate::agent::sandbox::format_output(&out, &err, code)),
            Err(e) => Err(e),
        }
    });

    let result = {
        let cancel_fut = async {
            token.wait().await;
            // 无法中断阻塞线程中的子进程，超时（60s）兜底
        };
        let run_fut = tokio::time::timeout(std::time::Duration::from_secs(60), handle);
        tokio::pin!(run_fut);
        tokio::select! {
            _ = cancel_fut => Err("已停止生成".into()),
            res = &mut run_fut => match res {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("命令执行超时（60 秒）".into()),
            },
        }
    };
    result
}

// ==================== 工具定义（OpenAI 兼容格式） ====================

fn tool_def(name: &str, description: &str, properties: serde_json::Value, required: Vec<&str>) -> serde_json::Value {
    json!({
        "name": name,
        "description": description,
        "input_schema": {
            "type": "object",
            "properties": properties,
            "required": required
        }
    })
}

pub fn web_search_tool() -> serde_json::Value {
    json!({
        "name": TOOL_WEB_SEARCH,
        "description": "搜索互联网获取实时、最新的信息。推荐策略：简单日常任务、事实类数据检索（如新闻、百科、常识、天气、名人资料）请使用 provider=tavily（快速轻量）；专业垂直领域内容（如财经股票、学术论文、医疗健康、法律条文、代码技术、安全漏洞）请使用 provider=anysearch（专业深度）。provider 默认 auto 由系统智能选择。",
        "input_schema": {
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "搜索查询关键词，应具体、包含核心实体与限定词" },
                "provider": { "type": "string", "enum": ["auto", "tavily", "anysearch"], "description": "搜索引擎选择：auto 自动 / tavily 日常快速 / anysearch 专业深度，默认 auto" }
            },
            "required": ["query"]
        }
    })
}

fn file_tools() -> Vec<serde_json::Value> {
    vec![
        tool_def(
            TOOL_READ_FILE,
            "读取会话目录内的文本文件内容。path 为会话目录内相对路径（如 docs/readme.md）。",
            json!({"path": {"type": "string", "description": "会话目录内相对路径"}}),
            vec!["path"],
        ),
        tool_def(
            TOOL_WRITE_FILE,
            "在会话目录内创建或覆盖写入一个文本文件（自动创建父目录）。",
            json!({
                "path": {"type": "string", "description": "会话目录内相对路径"},
                "content": {"type": "string", "description": "文件完整内容"}
            }),
            vec!["path", "content"],
        ),
        tool_def(
            TOOL_EDIT_FILE,
            "编辑会话目录内文件：将 old_string 精确替换为 new_string（只替换第一次出现）。",
            json!({
                "path": {"type": "string", "description": "会话目录内相对路径"},
                "old_string": {"type": "string", "description": "待替换的原文片段（必须精确匹配）"},
                "new_string": {"type": "string", "description": "替换后的内容"}
            }),
            vec!["path", "old_string", "new_string"],
        ),
        tool_def(
            TOOL_DELETE_FILE,
            "删除会话目录内的文件（或空目录）。危险操作，请谨慎调用。",
            json!({"path": {"type": "string", "description": "会话目录内相对路径"}}),
            vec!["path"],
        ),
        tool_def(
            TOOL_LIST_FILES,
            "列出会话 files 目录中的文件结构（dir 为相对 files 的子目录，默认为根目录）。",
            json!({"dir": {"type": "string", "description": "相对目录，默认 ."}}),
            vec![],
        ),
        tool_def(
            TOOL_GLOB,
            "按 glob 模式匹配会话 files 目录中的文件路径（支持 * 与 ?）。",
            json!({"pattern": {"type": "string", "description": "如 **\\*.rs 或 src/*.ts"}}),
            vec!["pattern"],
        ),
        tool_def(
            TOOL_GREP,
            "在会话 files 目录中搜索包含指定文本的文件与行号。",
            json!({"pattern": {"type": "string", "description": "要搜索的文本"}}),
            vec!["pattern"],
        ),
        tool_def(
            TOOL_BASH,
            "在隔离沙箱（会话 files 目录）中执行 shell 命令（Windows cmd 语法），用于运行脚本、构建、测试等。输出将被返回。",
            json!({"command": {"type": "string", "description": "要执行的命令（cmd 语法，工作目录为会话 files 目录）"}}),
            vec!["command"],
        ),
    ]
}

fn generate_image_tool() -> serde_json::Value {
    tool_def(
        TOOL_GENERATE_IMAGE,
        "根据文字描述生成一张图片（AI 绘画）。当用户要求生成、绘制、设计图片或图像时，必须调用此工具——系统会调用专门的绘图模型完成生成，你无需也无法自己'画'出来，直接调用即可。生成结果自动保存到会话 images 目录。",
        json!({
            "prompt": {"type": "string", "description": "图片描述，越详细越好（主体、场景、风格、光线、构图等）"},
            "image_size": {"type": "string", "enum": ["1024x1024", "960x1280", "768x1024", "720x1440", "720x1280"], "description": "图片尺寸，默认 1024x1024"},
            "negative_prompt": {"type": "string", "description": "负面提示词（不希望出现的内容）"}
        }),
        vec!["prompt"],
    )
}
