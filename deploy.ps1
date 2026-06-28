# deploy.ps1 - Copy llama-server.exe alongside wakeupllm.exe for distribution
param(
    [string]$ServerPath = "E:\AI\llama.cpp\build\bin\Release\llama-server.exe",
    [string]$TargetDir = "E:\AI\llama.cpp\proxy\wakeupLLM"
)

$target = Join-Path $TargetDir "llama-server.exe"

if (-not (Test-Path $ServerPath)) {
    Write-Host "ERROR: $ServerPath not found" -ForegroundColor Red
    exit 1
}

Copy-Item $ServerPath $target -Force
$size = [math]::Round((Get-Item $target).Length / 1MB, 1)
Write-Host "Copied llama-server.exe ($size MB) to $TargetDir" -ForegroundColor Green
Write-Host ""
Write-Host "Distribution files:" -ForegroundColor Cyan
Get-ChildItem $TargetDir -Filter "*.exe" | ForEach-Object {
    $s = [math]::Round($_.Length / 1MB, 1)
    Write-Host "  $($_.Name) ($s MB)"
}
