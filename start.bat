@echo off
title ChatDeepSeek 启动器
cd /d "%~dp0"

echo ==============================================
echo   ChatDeepSeek 一键启动器
echo ==============================================
echo.

rem ---------- 环境检查 ----------
set "ERR=0"
where node >nul 2>nul
if errorlevel 1 (
    echo [错误] 未找到 Node.js，请先安装：https://nodejs.org/
    set "ERR=1"
)
where npm >nul 2>nul
if errorlevel 1 (
    echo [错误] 未找到 npm，说明 Node.js 安装不完整，请重新安装 Node.js
    set "ERR=1"
)
where cargo >nul 2>nul
if errorlevel 1 (
    echo [错误] 未找到 Rust 工具链，请先安装：https://rustup.rs/
    set "ERR=1"
)
if "%ERR%"=="1" (
    echo.
    pause
    exit /b 1
)

rem ---------- WebView2 Runtime 检测 ----------
rem 本应用打包配置为 skip 自动安装 WebView2，系统缺失时应用无法启动（白屏/报错）
reg query "HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" /v pv >nul 2>nul
if errorlevel 1 (
    reg query "HKLM\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" /v pv >nul 2>nul
    if errorlevel 1 (
        reg query "HKCU\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" /v pv >nul 2>nul
        if errorlevel 1 (
            echo [警告] 未检测到 WebView2 Runtime，应用可能无法启动。
            echo         请安装：https://developer.microsoft.com/microsoft-edge/webview2/
            echo.
        )
    )
)

rem ---------- 依赖安装（首次或依赖不完整时） ----------
rem 检查 .bin\tauri.cmd 而非 node_modules：依赖装到一半时 node_modules 已存在但不可用
if not exist "node_modules\.bin\tauri.cmd" (
    if exist "node_modules" (
        echo [信息] 依赖不完整，重新安装...
    ) else (
        echo [信息] 首次运行，安装前端依赖（视网络情况可能需要几分钟）...
    )
    echo.
    call npm install --no-audit --no-fund
    if errorlevel 1 (
        echo.
        echo [错误] npm install 失败，请检查网络后重新运行本脚本。
        pause
        exit /b 1
    )
)

rem ---------- 菜单 ----------
echo 请选择操作：
echo.
echo   [1] 快速启动（构建 release 版并运行，推荐）
echo   [2] 开发模式（热更新；未签名 debug 版可能被 Smart App Control 拦截）
echo   [3] 构建安装包（NSIS，可选安装目录）
echo   [4] 构建便携版（单 exe，输出到 dist-portable）
echo   [0] 退出
echo.

rem 支持命令行参数直接执行，如：start.bat 1
if not "%~1"=="" set "choice=%~1"

:menu
if not defined choice set /p "choice=请输入数字并回车："
if "%choice%"=="1" goto dev
if "%choice%"=="2" goto dev_hot
if "%choice%"=="3" goto build_only
if "%choice%"=="4" goto portable
if "%choice%"=="0" exit /b 0
echo [错误] 无效输入（%choice%），请重新选择。
set "choice="
goto menu

:dev
echo.
if not exist "scripts\run-release.ps1" (
    echo [错误] 缺少 scripts\run-release.ps1，请确认项目文件完整。
    pause
    exit /b 1
)
echo [信息] 构建 release 版并启动（首次构建较慢，请耐心等待）...
echo [信息] 使用 release 构建，可避免 Smart App Control 拦截未签名 exe。
echo.
call npm run dev:release
if errorlevel 1 (
    echo.
    echo [错误] 启动失败，请查看上方错误信息。
    pause
)
goto end

:dev_hot
echo.
echo [警告] 开发模式使用未签名的 debug 构建。
echo [警告] 若出现 "application control policy has blocked this file (os error 4551)"，
echo [警告] 说明 Smart App Control 已开启：请改用选项 [1]，或在
echo [警告] Windows 安全中心 → 应用和浏览器控制 中关闭 Smart App Control。
echo.
echo [信息] 正在启动开发模式，首次运行需编译 Rust 后端（约几分钟）...
echo.
call npm run tauri dev
if errorlevel 1 (
    echo.
    echo [错误] 开发模式启动失败，请查看上方错误信息。
    pause
)
goto end

:build_only
echo.
echo [信息] 构建 release 安装包（NSIS，首次构建较慢，请耐心等待）...
call npm run tauri build
if errorlevel 1 (
    echo.
    echo [错误] 构建失败，请查看上方错误信息。
    pause
    exit /b 1
)
echo.
echo [信息] 构建完成：安装包位于 src-tauri\target\release\bundle\nsis\
echo [信息] 安装包可自定义安装目录，数据保存在安装目录中。
goto end

:portable
echo.
echo [信息] 构建 release 版并打包便携版（首次构建较慢，请耐心等待）...
call npm run bundle:portable
if errorlevel 1 (
    echo.
    echo [错误] 构建失败，请查看上方错误信息。
    pause
    exit /b 1
)
echo.
echo [信息] 便携版位于 dist-portable\ChatDeepSeek.exe
echo [信息] 运行时会自动在 exe 旁边创建 data\ 数据目录。
goto end

:end
echo.
echo 完成。按任意键关闭窗口。
pause >nul
