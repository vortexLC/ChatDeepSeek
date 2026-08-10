@echo off
title ChatDeepSeek 打包器
cd /d "%~dp0"

echo ==============================================
echo   ChatDeepSeek 打包器
echo   输出：便携版 exe + NSIS 安装包
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

rem ---------- 项目目录检查 ----------
if not exist "package.json" (
    echo [错误] 未找到 package.json，请确认脚本位于项目根目录。
    pause
    exit /b 1
)
if not exist "src-tauri" (
    echo [错误] 未找到 src-tauri 目录，请确认脚本位于项目根目录。
    pause
    exit /b 1
)

rem ---------- 显示当前版本 ----------
set "APP_VER="
for /f "usebackq delims=" %%v in (`node -p "require('./package.json').version" 2^>nul`) do set "APP_VER=%%v"
if defined APP_VER echo [信息] 当前版本：%APP_VER%
echo.

rem ---------- 依赖安装（首次或依赖不完整时） ----------
if not exist "node_modules\.bin\tauri.cmd" (
    echo [信息] 安装前端依赖（视网络情况可能需要几分钟）...
    echo.
    call npm install --no-audit --no-fund
    if errorlevel 1 (
        echo.
        echo [错误] npm install 失败，请检查网络后重新运行本脚本。
        pause
        exit /b 1
    )
)

rem ---------- 构建 release + NSIS 安装包 ----------
echo.
echo [信息] 构建 release 版与 NSIS 安装包（首次构建较慢，请耐心等待）...
echo.
call npm run tauri build
if errorlevel 1 (
    echo.
    echo [错误] 构建失败，请查看上方错误信息。
    pause
    exit /b 1
)

rem ---------- 收集产物到 dist-portable ----------
rem 只清理旧的交付文件，不删除 dist-portable 目录：
rem 便携版运行后会在其中生成 data\ 用户数据（会话记录等），整目录删除会误删数据
if not exist "dist-portable" mkdir "dist-portable"
if exist "dist-portable\ChatDeepSeek.exe" del /q "dist-portable\ChatDeepSeek.exe"
del /q "dist-portable\*setup.exe" 2>nul

copy /y "src-tauri\target\release\chat-deepseek.exe" "dist-portable\ChatDeepSeek.exe" >nul
if errorlevel 1 (
    echo [错误] 便携版 exe 复制失败。
    pause
    exit /b 1
)

set "SETUP="
for %%f in ("src-tauri\target\release\bundle\nsis\*setup.exe") do set "SETUP=%%f"
if not defined SETUP (
    echo [错误] 未在 src-tauri\target\release\bundle\nsis\ 找到安装包。
    pause
    exit /b 1
)
copy /y "%SETUP%" "dist-portable\" >nul
if errorlevel 1 (
    echo [错误] 安装包复制失败。
    pause
    exit /b 1
)

rem ---------- 清理构建中间产物 ----------
echo.
echo [信息] 清理构建中间产物...
if exist "src-tauri\target\release\nsis" rmdir /s /q "src-tauri\target\release\nsis"
if exist "src-tauri\target\release\bundle" rmdir /s /q "src-tauri\target\release\bundle"
if exist "dist" rmdir /s /q "dist"
del /q "src-tauri\target\release\*.pdb" 2>nul

rem ---------- 结果报告 ----------
echo.
echo ==============================================
echo   [完成] 打包成功，交付文件：
echo.
echo     便携版：  dist-portable\ChatDeepSeek.exe
for %%g in ("%SETUP%") do echo     安装包：  dist-portable\%%~nxg
echo.
echo   运行便携版时，数据目录 data\ 会自动创建在 exe 旁边；
echo   安装版数据保存在安装目录。dist-portable 中原有的
echo   data\ 目录已被保留，不会因重新打包而丢失。
echo ==============================================
echo.
pause
