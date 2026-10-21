# Elevated diagnostic: dump USN journal entries for the test folder / Recycle Bin and
# a few MFT facts. Run: Start-Process powershell -Verb RunAs -ArgumentList '-NoProfile -ExecutionPolicy Bypass -File <this file>'
$out = Join-Path $PSScriptRoot "..\target\diag"
New-Item -ItemType Directory -Force $out | Out-Null
"== USN journal: entries mentioning testjfile or Recycle Bin (newest last) ==" | Out-File "$out\usn.txt" -Encoding utf8
fsutil usn readjournal C: csv 2>&1 | Select-String -Pattern 'testjfile', '\$R[A-Z0-9]{6}', '\$I[A-Z0-9]{6}' | Select-Object -Last 400 | ForEach-Object { $_.Line } | Out-File "$out\usn.txt" -Append -Encoding utf8
"== USN journal info ==" | Out-File "$out\usn.txt" -Append -Encoding utf8
fsutil usn queryjournal C: 2>&1 | Out-File "$out\usn.txt" -Append -Encoding utf8
"== fsinfo ntfsinfo C: ==" | Out-File "$out\usn.txt" -Append -Encoding utf8
fsutil fsinfo ntfsinfo C: 2>&1 | Out-File "$out\usn.txt" -Append -Encoding utf8
"done $(Get-Date -Format o)" | Out-File "$out\done.txt" -Encoding utf8
Write-Host "diagnostic written to $out\usn.txt"
Start-Sleep 2
