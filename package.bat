@echo off
title ChatDeepSeek One-Click Packager
cd /d "%~dp0"

echo ==============================================
echo   ChatDeepSeek Packager
echo   Output: portable exe + NSIS installer
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

rem ---------- first run: install deps ----------
if not exist node_modules (
    echo [INFO] Installing frontend dependencies...
    echo.
    call npm install
    if errorlevel 1 (
        echo.
        echo [ERROR] npm install failed. Please check your network and retry.
        pause
        exit /b 1
    )
)

rem ---------- build release + installer ----------
echo.
echo [INFO] Building release version and NSIS installer (first build is slow, please wait)...
echo.
call npm run tauri build
if errorlevel 1 (
    echo.
    echo [ERROR] Build failed. See the error messages above.
    pause
    exit /b 1
)

rem ---------- collect deliverables into dist-portable ----------
if exist "dist-portable" rmdir /s /q "dist-portable"
mkdir "dist-portable"

copy /y "src-tauri\target\release\chat-deepseek.exe" "dist-portable\ChatDeepSeek.exe" >nul
if errorlevel 1 (
    echo [ERROR] Failed to copy portable exe.
    pause
    exit /b 1
)

set "SETUP="
for %%f in ("src-tauri\target\release\bundle\nsis\*setup.exe") do set "SETUP=%%f"
if not defined SETUP (
    echo [ERROR] Installer not found in src-tauri\target\release\bundle\nsis\
    pause
    exit /b 1
)
copy /y "%SETUP%" "dist-portable\" >nul
if errorlevel 1 (
    echo [ERROR] Failed to copy installer.
    pause
    exit /b 1
)

rem ---------- clean up intermediates ----------
echo.
echo [INFO] Cleaning up build intermediates...
if exist "src-tauri\target\release\nsis" rmdir /s /q "src-tauri\target\release\nsis"
if exist "src-tauri\target\release\bundle" rmdir /s /q "src-tauri\target\release\bundle"
if exist "dist" rmdir /s /q "dist"
del /q "src-tauri\target\release\*.pdb" 2>nul

rem ---------- report ----------
echo.
echo ==============================================
echo   [OK] Packaging finished. Deliverables:
echo.
echo     Portable :  dist-portable\ChatDeepSeek.exe
for %%g in ("%SETUP%") do echo     Installer:  dist-portable\%%~nxg
echo.
echo   Data folder is auto-created next to the exe
echo   (portable: exe dir; installed: install dir).
echo ==============================================
echo.
pause
