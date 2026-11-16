# Installs Goko on Windows (x86_64):
#
#   irm https://github.com/asta-nguyen/goko/releases/latest/download/install.ps1 | iex
#
# Downloads the latest release, checks its sha256, and installs it under
# %LOCALAPPDATA%\Goko\app with a Start-menu shortcut, a Desktop shortcut on the
# first install, and the `goko` command on the user PATH. Running it again
# updates the install in place.
#
# Keep this file ASCII without a byte-order mark: it runs through `irm | iex`,
# where Windows PowerShell 5.1 can misread UTF-8 and turn a mark or a
# non-ASCII character into stray text.
$ErrorActionPreference = "Stop"

$releases = if ($env:GOKO_RELEASES_URL) { $env:GOKO_RELEASES_URL } else {
    "https://github.com/asta-nguyen/goko/releases/latest/download"
}
$installDir = if ($env:GOKO_APP_DIR) { $env:GOKO_APP_DIR } else {
    Join-Path $env:LOCALAPPDATA "Goko\app"
}
$target = "windows-x86_64"

function Fail($message) {
    Write-Error "goko: $message"
    exit 1
}

# The release names its archives in latest.json so the installer never guesses
# a version number.
$manifestUrl = "$releases/latest.json"
try {
    $manifest = Invoke-RestMethod -Uri $manifestUrl -UseBasicParsing
} catch {
    Fail "cannot read $manifestUrl"
}
$url = $manifest.targets.$target.url
$sha = $manifest.targets.$target.sha256
if (-not $url) { Fail "the release has no $target build" }

$temp = Join-Path $env:TEMP "goko-install-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Force $temp | Out-Null
try {
    $zip = Join-Path $temp "goko.zip"
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
    $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $sha) { Fail "sha256 mismatch: expected $sha, got $actual" }

    Expand-Archive -Path $zip -DestinationPath $temp -Force
    $source = Join-Path $temp "goko"
    if (-not (Test-Path (Join-Path $source "goko-rune.exe"))) {
        Fail "the archive does not contain goko-rune.exe"
    }

    # Replace the install in place; a running app keeps its mapped files until
    # it exits, which is the same update story as the macOS installer.
    $firstInstall = -not (Test-Path $installDir)
    if (-not $firstInstall) { Remove-Item -Recurse -Force $installDir }
    New-Item -ItemType Directory -Force $installDir | Out-Null
    Copy-Item -Recurse -Force (Join-Path $source "*") $installDir

    # `goko` on the user PATH, pointing at the launcher beside the app.
    $launcher = Join-Path $installDir "goko.cmd"
    Set-Content -Path $launcher -Value "@echo off`r`nstart `"`" `"%~dp0goko-rune.exe`" %*" -Encoding ASCII
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not $userPath) { $userPath = "" }
    if (($userPath -split ";") -notcontains $installDir) {
        $joined = if ($userPath) { "$userPath;$installDir" } else { $installDir }
        [Environment]::SetEnvironmentVariable("Path", $joined, "User")
    }

    # Shortcuts so the app is discoverable without the shell. The Start-menu
    # one is refreshed on every run; the Desktop one is made on the first
    # install only, so an update does not bring back a shortcut the user
    # deleted.
    $shell = New-Object -ComObject WScript.Shell
    $folders = @([Environment]::GetFolderPath("Programs"))
    if ($firstInstall) { $folders += [Environment]::GetFolderPath("Desktop") }
    foreach ($folder in $folders) {
        $shortcut = $shell.CreateShortcut((Join-Path $folder "Goko.lnk"))
        $shortcut.TargetPath = Join-Path $installDir "goko-rune.exe"
        $shortcut.WorkingDirectory = $installDir
        $shortcut.Save()
    }

    Write-Host ""
    Write-Host "  Goko $($manifest.version) installed successfully!" -ForegroundColor Green
    Write-Host ""
    Write-Host "  To get started, restart your terminal and run:" -ForegroundColor Gray
    Write-Host "    goko" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "  Or launch Goko from the Start menu." -ForegroundColor DarkGray
    Write-Host ""
} finally {
    Remove-Item -Recurse -Force $temp -ErrorAction SilentlyContinue
}
