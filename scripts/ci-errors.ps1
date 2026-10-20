# Print the failed-step log of a run, stripped of ANSI/timestamps, filtered to error lines.
# Usage: .\scripts\ci-errors.ps1 -RunId 123 [-Context 8]
param([Parameter(Mandatory)][string]$RunId, [int]$Context = 6, [string]$Repo = "luckySpro/jfilerescue")
$env:Path = "C:\Program Files\GitHub CLI;C:\Program Files\Git\cmd;" + $env:Path
. (Join-Path $PSScriptRoot "gh-env.ps1")
$raw = (gh run view $RunId -R $Repo --log-failed 2>&1 | Out-String) -split "`n"
$steps = $raw | ForEach-Object { ($_ -split "`t")[0..1] -join " / " } | Group-Object | Sort-Object Count -Descending | Select-Object -First 5 | ForEach-Object { "$($_.Count)x $($_.Name)" }
Write-Output "== failed steps =="; $steps
$lines = $raw | ForEach-Object { (($_ -split "`t", 3)[2]) -replace "\x1b\[[0-9;]*m", "" -replace "^\S+Z ", "" }
$idx = @()
for ($i = 0; $i -lt $lines.Count; $i++) { if ($lines[$i] -match "^error|error\[E|error: |panicked|FAILED|Error:|##\[error\]|failed to") { $idx += $i } }
Write-Output "== $($idx.Count) error lines =="
$shown = @{}
foreach ($i in ($idx | Select-Object -First 12)) {
    for ($k = [Math]::Max(0, $i - 2); $k -le [Math]::Min($lines.Count - 1, $i + $Context); $k++) {
        if (-not $shown[$k]) { $shown[$k] = $true; Write-Output $lines[$k] }
    }
    Write-Output "--"
}
