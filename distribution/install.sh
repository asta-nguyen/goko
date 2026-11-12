#!/bin/sh
# Installs Goko on macOS (Apple Silicon) or Linux (x86_64, aarch64):
#
#   curl -fsSL https://github.com/asta-nguyen/goko/releases/latest/download/install.sh | sh
#
# Downloads the latest release, checks its sha256 (and, on macOS, its
# signature), and installs it: Goko.app in ~/Applications, or on Linux the
# `goko` command in ~/.local/bin and a launcher entry. Running it again updates.
set -eu

releases=${GOKO_RELEASES_URL:-https://github.com/asta-nguyen/goko/releases/latest/download}
bin="$HOME/.local/bin"

say() { printf '%s\n' "$*"; }
fail() { say "goko: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "needs $1"; }
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

need curl
need tar
case "$(uname -s)" in
  Darwin) os=macos ;;
  Linux) os=linux ;;
  *) fail "no build for $(uname -s); build from source: https://github.com/asta-nguyen/goko#build" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  arm64 | aarch64) arch=aarch64 ;;
  *) fail "no build for $(uname -m)" ;;
esac
if [ "$os" = macos ] && [ "$arch" = x86_64 ]; then
  # Rosetta reports x86_64 even on Apple Silicon.
  if [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" = 1 ]; then
    arch=aarch64
  else
    fail "Intel Macs are not supported; build from source: https://github.com/asta-nguyen/goko#build"
  fi
fi
target="$os-$arch"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl -fsSL "$releases/latest.json" -o "$tmp/latest.json" || fail "cannot reach $releases"
# Read without jq: the manifest is flat enough for sed once whitespace is gone.
flat=$(tr -d '\n ' < "$tmp/latest.json")
entry=$(printf '%s' "$flat" | sed -n "s/.*\"$target\":{\([^}]*\)}.*/\1/p")
url=$(printf '%s' "$entry" | sed -n 's/.*"url":"\([^"]*\)".*/\1/p')
want=$(printf '%s' "$entry" | sed -n 's/.*"sha256":"\([^"]*\)".*/\1/p')
version=$(printf '%s' "$flat" | sed -n 's/^{"version":"\([^"]*\)".*/\1/p')
[ -n "$url" ] && [ -n "$want" ] || fail "no build for $target yet"

say "Installing Goko ${version:-} for $target ..."
curl -fsSL "$url" -o "$tmp/goko.tar.gz" || fail "download failed"
[ "$(sha256 "$tmp/goko.tar.gz")" = "$want" ] || fail "download is damaged (checksum mismatch); try again"
tar -xzf "$tmp/goko.tar.gz" -C "$tmp"

if [ "$os" = macos ]; then
  apps="$HOME/Applications"
  app="$apps/Goko.app"
  [ -d "$tmp/Goko.app" ] || fail "the archive has no Goko.app"
  /usr/bin/codesign --verify --deep --strict "$tmp/Goko.app" || fail "invalid app signature"
  mkdir -p "$apps"
  rm -rf "$app"
  mv "$tmp/Goko.app" "$app"
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$app" 2>/dev/null || true
  say "Goko is installed in $app."
  running="$app/Contents/MacOS/goko-rune"
  if [ -d /Applications/Goko.app ]; then
    say "Note: another copy exists at /Applications/Goko.app; open the one in ~/Applications."
  fi
else
  data=${XDG_DATA_HOME:-$HOME/.local/share}
  home="$data/goko"
  [ -x "$tmp/goko/goko-rune" ] && [ -x "$tmp/goko/gokod" ] || fail "the archive has no goko-rune and gokod"
  mkdir -p "$data" "$bin" "$data/applications"
  # The app looks for gokod beside its own executable, so they stay together.
  rm -rf "$home.new"
  mv "$tmp/goko" "$home.new"
  rm -rf "$home"
  mv "$home.new" "$home"
  ln -sfn "$home/goko-rune" "$bin/goko"
  icon=utilities-terminal
  [ ! -f "$home/icon.png" ] || icon="$home/icon.png"
  cat > "$data/applications/goko.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Goko
Comment=Run interactive command-line agents by project
Exec=$bin/goko
Icon=$icon
Terminal=false
Categories=Development;
StartupWMClass=goko-rune
DESKTOP
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$data/applications" 2>/dev/null || true
  say "Goko is installed in $home; run it with: goko"
  case ":$PATH:" in
    *":$bin:"*) ;;
    *) say "Add $bin to your PATH to run goko from a terminal." ;;
  esac
  say "It needs a desktop session and a working Vulkan or OpenGL driver."
  running="$home/goko-rune"
fi

if pgrep -f "$running" >/dev/null 2>&1; then
  say "Quit and reopen Goko to use the new version."
fi
if pgrep -x gokod >/dev/null 2>&1; then
  say "A gokod daemon is running; it keeps serving your sessions until they end, so a new"
  say "daemon starts only after you stop it (this ends those sessions)."
fi
