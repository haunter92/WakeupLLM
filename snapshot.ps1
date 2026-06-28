param(
    [string]$SourceDir = "E:\AI\llama.cpp\proxy\wakeupLLM",
    [string]$BackupDir = "E:\AI\llama.cpp\proxy\wakeupLLM\backup"
)

$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$zipName = "wakeupLLM-snapshot-$timestamp.zip"
$zipPath = Join-Path $BackupDir $zipName

if (-not (Test-Path $BackupDir)) {
    New-Item -ItemType Directory -Path $BackupDir -Force | Out-Null
}

$excludeDirs = @('target', '__pycache__', 'test-results', 'backup')
$excludeExts = @('.pyc')

$files = Get-ChildItem -Path $SourceDir -Recurse -File | Where-Object {
    $path = $_.FullName
    $skip = $false
    foreach ($d in $excludeDirs) {
        if ($path -like "*\$d\*" -or $path -like "*\$d") { $skip = $true; break }
    }
    if (-not $skip) {
        foreach ($e in $excludeExts) {
            if ($_.Extension -eq $e) { $skip = $true; break }
        }
    }
    -not $skip
}

Compress-Archive -Path $files.FullName -DestinationPath $zipPath -Force

$size = (Get-Item $zipPath).Length
Write-Host "Snapshot: $zipPath ($size bytes)"
