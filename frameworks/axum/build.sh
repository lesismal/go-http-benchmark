#!/usr/bin/env bash
# Builds the axum server to $1 (default ../../output/bin/axum.server).
#
# Needs a Rust toolchain: cargo on PATH, of the version Cargo.toml's
# rust-version asks for or later. The dependencies are pinned by Cargo.lock
# (--locked), and script/Dockerfile.benchmark fetches and compiles them into
# the image, so a build in the container needs no network. CARGO_TARGET_DIR,
# when set, is where the build keeps its objects; the image points it outside
# the workspace so that the dependencies it compiled survive COPY.
set -euo pipefail
cd "$(dirname "$0")"
if ! command -v cargo >/dev/null 2>&1; then
    echo "axum: cargo not found; install Rust (https://rustup.rs) or leave axum out with BENCH_FRAMEWORKS" >&2
    exit 1
fi
output=${1:-../../output/bin/axum.server}
cargo build --release --locked
target_dir=${CARGO_TARGET_DIR:-target}
mkdir -p "$(dirname "$output")"
cp "$target_dir/release/axum-server" "$output"
