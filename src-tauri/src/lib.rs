mod agent;
mod commands;
mod db;
mod llm;
mod models;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::{commands::AppState, db::Db};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WindowEvent};

/// 用户点击托盘"退出"后置为 true，允许窗口真正关闭
static EXITING: AtomicBool = AtomicBool::new(false);
/// 托盘退出：等待主窗口真正销毁后再退出，避免 WebView2 卸载竞态
static EXIT_PENDING: AtomicBool = AtomicBool::new(false);

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn setup_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, "show", "显示主界面", true, None::<&str>)?;
    let exit_item = MenuItem::with_id(app, "exit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &exit_item])?;

    let icon = app
        .default_window_icon()
        .cloned()
        .expect("missing default window icon");

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("ChatDeepSeek")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "exit" => {
                EXITING.store(true, Ordering::SeqCst);
                if let Some(state) = app.try_state::<Arc<AppState>>() {
                    commands::cancel_all_tasks(&state);
                }
                if let Some(window) = app.get_webview_window("main") {
                    // 先销毁主窗口，让 WebView2 完成卸载，再在 Destroyed 事件中真正退出，
                    // 避免 "Failed to unregister class Chrome_WidgetWin_0" 报错
                    EXIT_PENDING.store(true, Ordering::SeqCst);
                    let _ = window.destroy();
                } else {
                    app.cleanup_before_exit();
                    app.exit(0);
                }
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

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
        .on_window_event(|window, event| {
            match event {
                WindowEvent::CloseRequested { api, .. } => {
                    if !EXITING.load(Ordering::SeqCst) {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
                WindowEvent::Destroyed => {
                    if EXIT_PENDING.swap(false, Ordering::SeqCst) {
                        let app = window.app_handle();
                        app.cleanup_before_exit();
                        app.exit(0);
                    }
                }
                _ => {}
            }
        })
        .setup(|app| {
            setup_tray(app)?;
            let app_data = app.path().app_data_dir()?;
            let mut root = resolve_data_root().unwrap_or(app_data.join("data"));
            // exe 目录不可写（如 Program Files 无权限）时回退到系统应用数据目录
            if std::fs::create_dir_all(&root).is_err() {
                root = app_data.join("data");
                std::fs::create_dir_all(&root)?;
            }
            let db = Db::open(&root)?;
            // 遗留设置迁移：旧版 deepseek / gen 配置 -> 统一模型提供商体系
            let settings = db.get_settings();
            let migrated = crate::models::migrate_legacy_providers(&settings);
            if migrated.providers != settings.providers {
                let _ = db.save_settings(&migrated);
            }
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
            commands::list_jobs,
            commands::test_deepseek_connection,
            commands::list_sf_video_models,
            commands::test_model,
            commands::fetch_webpage,
            commands::fetch_file_content,
            commands::get_artifact_abs_path,
            commands::respond_permission,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
