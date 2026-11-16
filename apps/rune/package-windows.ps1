# Build a release Goko for Windows and pack it for a GitHub release.
#
# Usage: pwsh -File apps/rune/package-windows.ps1 -Version <version> -Base <download-base-url>
#
# Writes dist/Goko-<version>-windows-x86_64.zip (a `goko/` folder holding
# `goko-rune.exe` and `gokod.exe`, which the app looks for beside itself) and
# dist/windows-x86_64.target.json, the entry for latest.json.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$Base
)

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "../..")

$target = "windows-x86_64"

cargo build --workspace --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

if (Test-Path dist) { Remove-Item -Recurse -Force dist }
New-Item -ItemType Directory -Force dist/goko | Out-Null
Copy-Item target/release/goko-rune.exe dist/goko/
Copy-Item target/release/gokod.exe dist/goko/
Copy-Item apps/rune/assets/icon.png dist/goko/icon.png
Copy-Item README.md dist/goko/

$file = "Goko-$Version-$target.zip"
Compress-Archive -Path dist/goko -DestinationPath "dist/$file"
Remove-Item -Recurse -Force dist/goko

$sha = (Get-FileHash "dist/$file" -Algorithm SHA256).Hash.ToLowerInvariant()

$entry = [ordered]@{ $target = [ordered]@{ url = "$Base/$file"; sha256 = $sha } }
$json = $entry | ConvertTo-Json -Compress -Depth 4
# Keep the file LF-only and trailing-newline-free, like the shell packagers.
[System.IO.File]::WriteAllText("dist/$target.target.json", $json + "`n")

Write-Output "packed dist/$file ($sha)"
