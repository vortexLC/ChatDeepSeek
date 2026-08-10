$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root "src-tauri\target\release\chat-deepseek.exe"
$out = Join-Path $root "dist-portable"

if (-not (Test-Path $exe)) {
    Write-Host "[ERROR] build output not found: $exe" -ForegroundColor Red
    Write-Host "run 'npm run dev:release' first"
    exit 1
}

New-Item -ItemType Directory -Path $out -Force | Out-Null
Copy-Item $exe (Join-Path $out "ChatDeepSeek.exe") -Force
Write-Host "[OK] portable exe updated: $out\ChatDeepSeek.exe"
Start-Process (Join-Path $out "ChatDeepSeek.exe")
Write-Host "[OK] app started"
