#!/bin/sh
# Install or update mcp-brain from a GitHub release on macOS or Linux.
#
#   curl -fsSL https://raw.githubusercontent.com/btravers/mcp-brain/main/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --version v0.4.0 --dir /usr/local/bin
#
# Re-running updates in place. The archive is checked against the release's
# SHA256SUMS before anything is installed.
set -eu

REPO="btravers/mcp-brain"
version="latest"
dir="${MCP_BRAIN_INSTALL_DIR:-$HOME/.local/bin}"

fail() {
    echo "mcp-brain install: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version) [ $# -ge 2 ] || fail "--version needs a tag such as v0.4.0"; version="$2"; shift 2 ;;
        --dir) [ $# -ge 2 ] || fail "--dir needs a directory"; dir="$2"; shift 2 ;;
        -h | --help)
            echo "usage: install.sh [--version vX.Y.Z] [--dir DIR]"
            echo "Defaults: the latest release, installed into \$MCP_BRAIN_INSTALL_DIR or ~/.local/bin."
            exit 0 ;;
        *) fail "unknown option: $1 (see --help)" ;;
    esac
done

os="$(uname -s)"
arch="$(uname -m)"
# A shell under Rosetta reports x86_64 on Apple Silicon.
if [ "$os/$arch" = "Darwin/x86_64" ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = "1" ]; then
    arch="arm64"
fi
case "$os/$arch" in
    Darwin/arm64) target="aarch64-apple-darwin" ;;
    Linux/x86_64 | Linux/amd64) target="x86_64-unknown-linux-musl" ;;
    Linux/aarch64 | Linux/arm64)
        target="aarch64-unknown-linux-musl"
        # The ARM64 build uses half-precision instructions (ARMv8.2-A FP16).
        grep -qw fphp /proc/cpuinfo 2>/dev/null ||
            fail "this ARM CPU lacks FP16 (ARMv8.2-A), which the Linux ARM64 build requires; build from source instead" ;;
    Darwin/x86_64) fail "Intel macOS builds are not provided; build from source instead" ;;
    *) fail "no release build for $os $arch; on Windows, download the zip from the Releases page" ;;
esac

command -v curl >/dev/null 2>&1 || fail "curl is required"
if command -v sha256sum >/dev/null 2>&1; then
    sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
    sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
    fail "sha256sum or shasum is required to verify the download"
fi

if [ "$version" = "latest" ]; then
    base="https://github.com/$REPO/releases/latest/download"
else
    base="https://github.com/$REPO/releases/download/$version"
fi
archive="mcp-brain-$target.tar.gz"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/mcp-brain.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
trap 'exit 1' INT TERM

echo "Downloading $archive ($version)..."
curl -fsSL --proto '=https' --retry 3 -o "$tmp/$archive" "$base/$archive" ||
    fail "download failed: $base/$archive"
curl -fsSL --proto '=https' --retry 3 -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" ||
    fail "download failed: $base/SHA256SUMS"

expected="$(awk -v f="$archive" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")"
[ -n "$expected" ] || fail "$archive is not listed in SHA256SUMS"
actual="$(sha256 "$tmp/$archive")"
[ "$actual" = "$expected" ] || fail "checksum mismatch for $archive: expected $expected, got $actual"

tar -xzf "$tmp/$archive" -C "$tmp" ./mcp-brain
mkdir -p "$dir"
# Copy next to the target, then rename: running servers keep the old file.
cp "$tmp/mcp-brain" "$dir/.mcp-brain.new"
chmod 755 "$dir/.mcp-brain.new"
mv -f "$dir/.mcp-brain.new" "$dir/mcp-brain"

echo "Installed $("$dir/mcp-brain" --version) to $dir/mcp-brain"
case ":$PATH:" in
    *":$dir:"*) ;;
    *) echo "Note: $dir is not on your PATH. Add it, or use the absolute path in client settings." ;;
esac
