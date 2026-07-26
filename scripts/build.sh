#!/bin/sh
# herdr [[build]] step for herdr-pets.
#
# Preferred path: download the prebuilt release binary for this platform, so
# installing needs no Rust toolchain. Fallback: build from source with cargo
# (used on platforms we don't publish, or when the download is unavailable).
# Either way the binary lands at target/release/herdr-pets, where the manifest's
# pane/action commands point.
#
# Dry-run: `scripts/build.sh --print-url` prints the resolved download URL (an
# empty line when the platform is unsupported) and exits without downloading.
# tests/build_script.rs uses it; set HERDR_PETS_FAKE_UNAME_S / _M to override
# platform detection under test.
set -e

REPO="vandemaelefelix/herdr-pets"
BIN_DIR="target/release"
DEST="$BIN_DIR/herdr-pets"

# --- version from the manifest (single source of truth) ---------------------
manifest_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
VERSION=$(sed -n 's/^version[[:space:]]*=[[:space:]]*"\(.*\)".*/\1/p' \
  "$manifest_dir/herdr-plugin.toml" | head -n1)

# --- detect target triple ---------------------------------------------------
os=${HERDR_PETS_FAKE_UNAME_S:-$(uname -s)}
arch=${HERDR_PETS_FAKE_UNAME_M:-$(uname -m)}
target=""
case "$os" in
Darwin)
  case "$arch" in
  arm64 | aarch64) target="aarch64-apple-darwin" ;;
  x86_64) target="x86_64-apple-darwin" ;;
  esac
  ;;
Linux)
  case "$arch" in
  x86_64) target="x86_64-unknown-linux-gnu" ;;
  aarch64 | arm64) target="aarch64-unknown-linux-gnu" ;;
  esac
  ;;
esac

url=""
if [ -n "$target" ] && [ -n "$VERSION" ]; then
  url="https://github.com/$REPO/releases/download/v$VERSION/herdr-pets-$target"
fi

# --- dry-run for tests -------------------------------------------------------
if [ "$1" = "--print-url" ]; then
  printf '%s\n' "$url"
  exit 0
fi

# --- source build fallback ---------------------------------------------------
build_from_source() {
  echo "herdr-pets: building from source with cargo" >&2
  # Source ~/.cargo/env so cargo is found on GUI / login-less launches.
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
  exec cargo build --release
}

# --- fetch, verify, else fall back ------------------------------------------
[ -n "$url" ] || build_from_source

echo "herdr-pets: fetching prebuilt $target" >&2
mkdir -p "$BIN_DIR"
tmp="$DEST.download"
if curl -fsSL "$url" -o "$tmp" 2>/dev/null && [ -s "$tmp" ]; then
  chmod +x "$tmp"
  mv "$tmp" "$DEST"
  # Trust the binary only if it actually runs on this machine.
  if "$DEST" --version >/dev/null 2>&1; then
    echo "herdr-pets: installed prebuilt binary" >&2
    exit 0
  fi
fi
rm -f "$tmp"
echo "herdr-pets: prebuilt unavailable, falling back to source build" >&2
build_from_source
