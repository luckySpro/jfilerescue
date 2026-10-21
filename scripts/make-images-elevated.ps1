# Elevated helper: build the NTFS/FAT32/exFAT test images with diskpart, then run
# the real-image integration tests. The UAC prompt is unavoidable (VHD attach).
# Usage (from an elevated PowerShell, or let Claude launch it via Start-Process -Verb RunAs):
#   .\scripts\make-images-elevated.ps1
$ErrorActionPreference = "Continue"
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$log = Join-Path $root "test-images\make.log"
New-Item -ItemType Directory -Force (Join-Path $root "test-images") | Out-Null
& "C:\Program Files\nodejs\node.exe" scripts/make-test-images.mjs --size-mb 64 *> $log
"exit: $LASTEXITCODE" | Out-File $log -Append
Get-ChildItem (Join-Path $root "test-images") | Select-Object Name, Length | Out-File $log -Append
"done" | Out-File (Join-Path $root "test-images\done.txt")
