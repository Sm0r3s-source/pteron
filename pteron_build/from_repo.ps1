# Build Pteron from this repository on Windows. You need the Servo build setup first (see BUILD.md).
#
#   powershell -ExecutionPolicy Bypass -File .\pteron_build\from_repo.ps1
#   powershell -ExecutionPolicy Bypass -File .\pteron_build\from_repo.ps1 -Production   # smaller and faster, much longer build
#
# Servo v0.7.0 is fetched into ..\servo-v0.7.0 (or -Source) and Pteron's files are copied over it.
param([string]$Source, [switch]$Production)
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
if (-not $Source) { $Source = Join-Path (Split-Path -Parent $repo) "servo-v0.7.0" }
if (-not (Test-Path (Join-Path $Source "mach.bat"))) {
    git clone --depth 1 --branch v0.7.0 https://github.com/servo/servo.git $Source
    if ($LASTEXITCODE -ne 0) { throw "Could not fetch Servo." }
}

# Pteron's files go over the Servo checkout (not the repository's own housekeeping folders).
$skip = ".git", ".github", "pteron_ci", "pteron_build", "screenshots", "benchmarks"
Get-ChildItem $repo -Recurse -File -Force | ForEach-Object {
    $rel = $_.FullName.Substring($repo.Length + 1)
    if ($skip -contains ($rel -split '[\\/]')[0]) { return }
    $dest = Join-Path $Source $rel
    New-Item -ItemType Directory -Force -Path (Split-Path $dest) | Out-Null
    Copy-Item $_.FullName $dest -Force
    (Get-Item $dest).LastWriteTime = Get-Date   # so that cargo notices the file
}
$manifest = Join-Path $Source "ports\servoshell\platform\windows\servoshell.exe.manifest"
if (Test-Path $manifest) {
    $text = Get-Content $manifest -Raw
    [IO.File]::WriteAllText($manifest, $text.Replace('name="servo.ServoShell"', 'name="Pteron.Pteron"'), (New-Object Text.UTF8Encoding($false)))
}
# Official builds carry the author's public update key. Without it this placeholder is used:
# your build works, but it cannot install updates by itself (rebuild to update).
$key = Join-Path $Source "ports\servoshell\update_key.pub"
if (-not (Test-Path $key)) { [IO.File]::WriteAllText($key, "AAAA") }

Push-Location $Source
try {
    if ($Production) { & .\mach.bat build --production } else { & .\mach.bat build -r }
    if ($LASTEXITCODE -ne 0) { throw "Build failed (exit code $LASTEXITCODE)." }
} finally { Pop-Location }
$out = if ($Production) { "production" } else { "release" }
Write-Host "Built: $Source\target\$out\servoshell.exe" -ForegroundColor Green
