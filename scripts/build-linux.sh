#!/usr/bin/env bash
# Builds the Linux packages: `wsl -e bash scripts/build-linux.sh` from the repository on Windows (or run it on Linux).
# It builds the app, uucc and the helper in release, then the .deb (cargo-deb) and the .rpm (cargo-generate-rpm), and
# copies both to dist/. `--check` lists what's in them and fails unless every file the packages must install is there.
set -euo pipefail
cd "$(dirname "$0")/.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/uucc-target}"
if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi

# Cargo.toml has CRLF line endings in a Windows checkout.
version=$(tr -d '\r' < Cargo.toml | sed -n 's/^version = "\(.*\)"$/\1/p' | head -1)
deb="dist/unofficial-uniwill-control-center_${version}-1_amd64.deb"
rpm="dist/unofficial-uniwill-control-center-${version}-1.x86_64.rpm"

# What the packages install (the spec's Packaging table).
paths=(
    /usr/bin/unofficial-uniwill-control-center
    /usr/bin/uucc
    /usr/libexec/uucc-helper
    /usr/lib/systemd/system/uucc-helper.socket
    /usr/lib/systemd/system/uucc-helper.service
    /usr/share/polkit-1/actions/io.github.thnorty.uucc.policy
    /usr/lib/udev/rules.d/70-uucc-lighting.rules
    /usr/share/applications/unofficial-uniwill-control-center.desktop
    /usr/share/icons/hicolor/256x256/apps/unofficial-uniwill-control-center.png
)

check() {
    local missing=0
    for package in "$deb" "$rpm"; do
        if [ ! -f "$package" ]; then
            echo "No $package: build it first."
            missing=1
        fi
    done
    [ "$missing" = 0 ] || exit 1
    local deb_files rpm_files
    deb_files=$(dpkg-deb -c "$deb" | awk '{print $6}' | sed 's|^\./|/|')
    rpm_files=$(rpm -qlp "$rpm" 2>/dev/null)
    for path in "${paths[@]}"; do
        grep -qx "$path" <<<"$deb_files" || { echo "The .deb lacks $path"; missing=1; }
        grep -qx "$path" <<<"$rpm_files" || { echo "The .rpm lacks $path"; missing=1; }
    done
    echo "$deb: depends on $(dpkg-deb -f "$deb" Depends)"
    echo "$rpm: requires $(rpm -qp --requires "$rpm" 2>/dev/null | grep -v '^rpmlib' | tr '\n' ' ')"
    if [ "$missing" = 0 ]; then echo "Both packages have all ${#paths[@]} files."; fi
    exit "$missing"
}

if [ "${1:-}" = "--check" ]; then
    check
fi

cargo build --release --bin UnofficialUniwillControlCenter --bin uucc --bin uucc-helper
mkdir -p dist
cargo deb --no-build --output "$deb"
cargo generate-rpm --target-dir "$CARGO_TARGET_DIR" --output "$rpm"
echo "Built $deb and $rpm"
