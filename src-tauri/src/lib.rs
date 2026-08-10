mod agent;
mod commands;
mod db;
mod llm;
mod models;

use std::path::PathBuf;
use std::sync::Arc;

use crate::{commands::AppState, db::Db};
use tauri::Manager;

/// 向上查找项目根目录（含 package.json + src-tauri 标记），仅开发模式使用
#[cfg(debug_assertions)]
fn resolve_project_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let mut dir = Some(cwd.as_path());
    while let Some(d) = dir {
        if d.join("src-tauri").is_dir() && d.join("package.json").is_file() {
            return Some(d.to_path_buf());
        }
        if d.join("package.json").is_file() && d.join("vite.config.ts").is_file() {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// 数据根目录定位：
///  1. 开发模式（debug 构建）：项目根目录 /data
///  2. 生产模式（便携版 / 安装版 release 构建）：exe 同目录 /data —— 便携包运行即可用，
///     安装版数据默认落在用户选择的安装目录中
fn resolve_data_root() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        if let Some(root) = resolve_project_root() {
            return Some(root.join("data"));
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("data")))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_data = app.path().app_data_dir()?;
            let mut root = resolve_data_root().unwrap_or(app_data.join("data"));
            // exe 目录不可写（如 Program Files 无权限）时回退到系统应用数据目录
            if std::fs::create_dir_all(&root).is_err() {
                root = app_data.join("data");
                std::fs::create_dir_all(&root)?;
            }
            let db = Db::open(&root)?;
            let state = Arc::new(AppState::new(db)?);
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_initial_state,
            commands::list_conversations,
            commands::create_conversation,
            commands::update_conversation,
            commands::delete_conversation,
            commands::clear_all_conversations,
            commands::get_messages,
            commands::get_context_status,
            commands::get_settings,
            commands::save_settings,
            commands::send_message,
            commands::edit_and_resend,
            commands::stop_generation,
            commands::test_deepseek_connection,
            commands::fetch_webpage,
            commands::fetch_file_content,
            commands::get_artifact_abs_path,
            commands::respond_permission,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
