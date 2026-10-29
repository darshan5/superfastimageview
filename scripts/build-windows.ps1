# Builds the optimized Windows executable into dist\.
#   powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")
cargo build --release
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\release\superfastimageview.exe dist\SuperFastImageView.exe -Force
Write-Host "Built dist\SuperFastImageView.exe"
