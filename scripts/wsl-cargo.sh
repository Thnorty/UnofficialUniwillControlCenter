#!/usr/bin/env bash
# Runs cargo for the Linux build in WSL: `wsl -e bash scripts/wsl-cargo.sh test` from the repository on Windows.
# The Linux build keeps its own target folder in the WSL home, so Windows and Linux artifacts never mix.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="$HOME/uucc-target"
. "$HOME/.cargo/env"
cargo "$@"
