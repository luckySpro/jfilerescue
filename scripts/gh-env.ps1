# Prints GH_TOKEN from the Git Credential Manager cache. Dot-source: . .\scripts\gh-env.ps1
$q = Join-Path $env:TEMP "jfr-gcq.txt"
[IO.File]::WriteAllText($q, "protocol=https`nhost=github.com`n`n")
$fill = cmd /c "git credential fill < `"$q`""
Remove-Item -LiteralPath $q -ErrorAction SilentlyContinue
$env:GH_TOKEN = ($fill | Where-Object { $_ -like "password=*" }) -replace "^password=", ""
if (-not $env:GH_TOKEN) { Write-Error "no cached GitHub credential" }
