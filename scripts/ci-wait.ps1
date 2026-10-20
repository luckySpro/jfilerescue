# Poll a GitHub Actions run until it completes or any job fails.
# Usage: .\scripts\ci-wait.ps1 -RunId 123 [-MaxMinutes 10]
param([Parameter(Mandatory)][string]$RunId, [int]$MaxMinutes = 10, [string]$Repo = "luckySpro/jfilerescue")
$env:Path = "C:\Program Files\GitHub CLI;C:\Program Files\Git\cmd;" + $env:Path
. (Join-Path $PSScriptRoot "gh-env.ps1")
$deadline = (Get-Date).AddMinutes($MaxMinutes)
while ($true) {
    $j = gh run view $RunId -R $Repo --json status,conclusion,jobs 2>$null | ConvertFrom-Json
    $parts = foreach ($job in $j.jobs) {
        $n = ($job.name -split ",")[0] -replace "^build \(", ""
        "$n=$($job.status)/$($job.conclusion)"
    }
    Write-Output ("[{0:HH:mm:ss}] {1} :: {2}" -f (Get-Date), $j.status, ($parts -join "  "))
    if ($j.status -eq "completed") { break }
    if ($j.jobs | Where-Object { $_.conclusion -eq "failure" }) { Write-Output "FAILURE detected"; break }
    if ((Get-Date) -gt $deadline) { Write-Output "timeout"; break }
    Start-Sleep 30
}
Write-Output "conclusion: $($j.conclusion)"
