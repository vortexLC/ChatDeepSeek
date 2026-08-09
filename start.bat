@echo off
title ChatDeepSeek Launcher
cd /d "%~dp0"

echo ==============================================
echo   ChatDeepSeek One-Click Launcher
echo ==============================================
echo.

rem ---------- environment check ----------
where node >nul 2>nul
if errorlevel 1 (
    echo [ERROR] Node.js not found. Please install from: https://nodejs.org/
    echo.
    pause
    exit /b 1
)

where cargo >nul 2>nul
if errorlevel 1 (
    echo [ERROR] Rust toolchain not found. Please install from: https://rustup.rs/
    echo.
    pause
    exit /b 1
)

rem ---------- auto install deps on first run ----------
if not exist node_modules (
    echo [INFO] First run detected. Installing frontend dependencies...
    echo.
    call npm install
    if errorlevel 1 (
        echo.
        echo [ERROR] npm install failed. Please check your network and retry.
        pause
        exit /b 1
    )
)

rem ---------- menu ----------
echo Please choose an action:
echo.
echo   [1] Quick start (dev mode, hot reload)
echo   [2] Build release and run (first build is slow)
echo   [3] Build installer (NSIS, choose install dir)
echo   [4] Build portable (single exe + copy to dist-portable)
echo   [0] Exit
echo.
set /p choice=Enter a number and press Enter: 

if "%choice%"=="1" goto dev
if "%choice%"=="2" goto release
if "%choice%"=="3" goto build_only
if "%choice%"=="4" goto portable
exit /b 0

:dev
echo.
echo [INFO] Starting dev mode. First launch compiles the Rust backend (a few minutes)...
echo [INFO] The ChatDeepSeek window will pop up. Close it or press Ctrl+C to stop.
echo.
call npm run tauri dev
goto end

:release
echo.
echo [INFO] Building release version (full compile on first run, please wait)...
call npm run tauri build
if errorlevel 1 (
    echo.
    echo [ERROR] Build failed. See the error messages above.
    pause
    exit /b 1
)
echo.
echo [INFO] Build succeeded. Launching app...
start "" "src-tauri\target\release\chat-deepseek.exe"
goto end

:build_only
echo.
echo [INFO] Building release installer (NSIS, first build is slow, please wait)...
call npm run tauri build
if errorlevel 1 (
    echo.
    echo [ERROR] Build failed. See the error messages above.
    pause
    exit /b 1
)
echo.
echo [INFO] Build finished. Installer is in src-tauri\target\release\bundle\nsis\
echo [INFO] The installer lets you choose the install folder; data is stored in the install dir.
goto end

:portable
echo.
echo [INFO] Building release version and packaging portable exe (first build is slow, please wait)...
call npm run bundle:portable
if errorlevel 1 (
    echo.
    echo [ERROR] Build failed. See the error messages above.
    pause
    exit /b 1
)
echo.
echo [INFO] Portable exe is in dist-portable\ChatDeepSeek.exe
echo [INFO] Data (data folder) is auto-created next to the exe when you run it.
goto end

:end
echo.
echo Done. Press any key to close this window.
pause >nul
