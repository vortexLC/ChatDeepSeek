# 从 .git 对象库中按路径恢复指定提交里的文件（PowerShell 实现，无 git 依赖）
param(
    [Parameter(Mandatory = $true)][string]$RepoPath,
    [Parameter(Mandatory = $true)][string]$TargetPath,
    [Parameter(Mandatory = $true)][string]$OutputFile,
    [string]$Commit = ""
)

$gitDir = Join-Path $RepoPath '.git'
$tmpObj = Join-Path $env:TEMP 'gitobj_tmp.bin'

# 解压 git 对象到临时文件
function Read-GitObjectToFile([string]$hash, [string]$outFile) {
    $dir = Join-Path $gitDir ('objects\' + $hash.Substring(0, 2))
    $file = Join-Path $dir $hash.Substring(2)
    if (-not (Test-Path $file)) { return $false }
    $bytes = [System.IO.File]::ReadAllBytes($file)
    $ms = New-Object System.IO.MemoryStream(,$bytes[2..($bytes.Length - 1)])
    $ds = New-Object System.IO.Compression.DeflateStream($ms, [System.IO.Compression.CompressionMode]::Decompress)
    $out = New-Object System.IO.MemoryStream
    $ds.CopyTo($out)
    $ds.Dispose(); $ms.Dispose()
    [System.IO.File]::WriteAllBytes($outFile, $out.ToArray())
    $out.Dispose()
    return $true
}

function Get-BlobForPath([string]$treeHash, [string]$prefix) {
    if (-not (Read-GitObjectToFile $treeHash $tmpObj)) { return $null }
    $tree = [System.IO.File]::ReadAllBytes($tmpObj)
    $i = 0
    $len = $tree.Length
    while ($i -lt $len) {
        $sp = [Array]::IndexOf($tree, [byte]0x20, $i)
        if ($sp -lt 0) { break }
        $mode = [System.Text.Encoding]::ASCII.GetString($tree[$i..($sp - 1)])
        $nul = [Array]::IndexOf($tree, [byte]0x00, $sp)
        if ($nul -lt 0) { break }
        $name = [System.Text.Encoding]::UTF8.GetString($tree[($sp + 1)..($nul - 1)])
        $shaBytes = $tree[($nul + 1)..($nul + 20)]
        $sha = -join ($shaBytes | ForEach-Object { $_.ToString('x2') })
        $path = if ($prefix) { $prefix + '/' + $name } else { $name }
        if ($mode -eq '40000' -or $mode -eq '040000') {
            $r = Get-BlobForPath $sha $path
            if ($r) { return $r }
        } elseif ($path -eq $TargetPath) {
            return $sha
        }
        $i = $nul + 21
    }
    return $null
}

$headRef = Join-Path $gitDir 'refs\heads\main'
$head = (Get-Content $headRef -ErrorAction SilentlyContinue).Trim()
if (-not $head) {
    $head = ((Get-Content (Join-Path $gitDir 'logs\HEAD') | Select-Object -Last 1) -split ' ')[1]
}
if ($Commit) { $head = $Commit }
Write-Output "HEAD: $head"
if (-not (Read-GitObjectToFile $head $tmpObj)) { Write-Error "无法读取提交对象 $head"; exit 1 }
$commitText = [System.Text.Encoding]::ASCII.GetString([System.IO.File]::ReadAllBytes($tmpObj))
$treeHash = ([regex]::Match($commitText, 'tree ([0-9a-f]{40})')).Groups[1].Value
Write-Output "Tree: $treeHash"
$blobHash = Get-BlobForPath $treeHash ''
if (-not $blobHash) { Write-Error "路径 $TargetPath 在提交 $head 中不存在"; exit 1 }
Write-Output "Blob: $blobHash"
if (-not (Read-GitObjectToFile $blobHash $tmpObj)) { Write-Error "无法读取 blob"; exit 1 }
$raw = [System.IO.File]::ReadAllBytes($tmpObj)
# 剥离 git 对象头（"blob <size>\0"）
$nul = [Array]::IndexOf($raw, [byte]0x00)
if ($nul -lt 0) { Write-Error "对象格式异常"; exit 1 }
$content = $raw[($nul + 1)..($raw.Length - 1)]
$text = [System.Text.Encoding]::UTF8.GetString($content)
[System.IO.File]::WriteAllText($OutputFile, $text, (New-Object System.Text.UTF8Encoding($false)))
Write-Output "Recovered $($content.Length) bytes -> $OutputFile"
