#!/usr/bin/env bash
# Build Pteron from this repository on Linux (Ubuntu 24.04, or WSL2 on Windows).
# About 25 GB of disk and 8 GB of memory; the first build takes a long time.
#
#   bash pteron_build/from_repo.sh               # with the compatibility tab (WebKitGTK)
#   bash pteron_build/from_repo.sh --no-compat   # without it
#
# Servo v0.7.0 is fetched into ../servo-v0.7.0 (or $PTERON_SRC) and Pteron's files are copied over it.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${PTERON_SRC:-$REPO/../servo-v0.7.0}"
FEATURES=(--features compat_linux)
[ "${1:-}" = "--no-compat" ] && FEATURES=()
export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"

command -v uv >/dev/null || curl -LsSf https://astral.sh/uv/install.sh | sh
command -v cargo >/dev/null || { curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y; . "$HOME/.cargo/env"; }
[ -f "$SRC/mach" ] || git clone --depth 1 --branch v0.7.0 https://github.com/servo/servo.git "$SRC"

# Pteron's files go over the Servo checkout (not the repository's own housekeeping folders).
tar -C "$REPO" --exclude=./.git --exclude=./.github --exclude=./pteron_ci --exclude=./pteron_build \
    --exclude=./screenshots --exclude=./benchmarks -cf - . | tar -C "$SRC" -xmf -

# Official builds carry the author's public update key. Without it this placeholder is used:
# your build works, but it cannot install updates by itself (rebuild to update).
KEY="$SRC/ports/servoshell/update_key.pub"
[ -f "$KEY" ] || echo "AAAA" > "$KEY"

cd "$SRC"
./mach bootstrap --yes
if [ ${#FEATURES[@]} -gt 0 ] && ! pkg-config --exists webkit2gtk-4.1; then
  sudo apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev
fi
./mach build -r "${FEATURES[@]}"
echo "Built. Run it with:  cd $SRC && ./target/release/servoshell"
