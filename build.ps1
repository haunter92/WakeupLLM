# build.ps1 - Build wakeupLLM and bundle llama-server.exe
param(
    [string]$ServerPath = "E:\AI\llama.cpp\build\bin\Release\llama-server.exe"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$src = Join-Path $root "src-tauri"

Write-Host "Building wakeupLLM..." -ForegroundColor Cyan
cargo build --release --manifest-path "$src\Cargo.toml"
if ($LASTEXITCODE -ne 0) {
    Write-Host "Build FAILED" -ForegroundColor Red
    exit 1
}

$exe = Join-Path $root "wakeupllm.exe"
$src_exe = Join-Path $src "target\release\wakeupllm.exe"
Copy-Item $src_exe $exe -Force
$size = [math]::Round((Get-Item $exe).Length / 1MB, 1)
Write-Host "wakeupllm.exe ($size MB)" -ForegroundColor Green

Write-Host ""
Write-Host "Bundling llama-server.exe..." -ForegroundColor Cyan
if (-not (Test-Path $ServerPath)) {
    Write-Host "WARNING: $ServerPath not found, skipping" -ForegroundColor Yellow
} else {
    $target = Join-Path $root "llama-server.exe"
    Copy-Item $ServerPath $target -Force
    $s = [math]::Round((Get-Item $target).Length / 1MB, 1)
    Write-Host "llama-server.exe ($s MB)" -ForegroundColor Green
}

Write-Host ""
Write-Host "Distribution folder:" -ForegroundColor Cyan
Get-ChildItem $root -Filter "*.exe" | ForEach-Object {
    $s = [math]::Round($_.Length / 1MB, 1)
    Write-Host "  $($_.Name) ($s MB)"
}
Get-ChildItem $root -Filter "model-config.json" | ForEach-Object {
    Write-Host "  $($_.Name)"
}
