$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root "src-tauri\target\release\chat-deepseek.exe"
$out = Join-Path $root "dist-portable"

if (-not (Test-Path $exe)) {
    Write-Host "[ERROR] 未找到发布版程序: $exe" -ForegroundColor Red
    Write-Host "请先执行: npm run bundle:portable (自动构建后复制)"
    exit 1
}

New-Item -ItemType Directory -Path $out -Force | Out-Null
Copy-Item $exe (Join-Path $out "ChatDeepSeek.exe") -Force
Write-Host "[OK] 便携版已生成: $out\ChatDeepSeek.exe"
Write-Host "[INFO] 便携版数据目录自动生成在 ChatDeepSeek.exe 同目录的 data 文件夹中"
