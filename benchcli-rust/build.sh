#!/usr/bin/env bash
# Builds the Rust benchmark client to $1 (default ../output/bin/bench.client).
#
# Needs cargo on PATH; the dependencies are pinned by Cargo.lock (--locked),
# and script/Dockerfile.benchmark compiles them into the image, so a build in
# the container needs no network. CARGO_TARGET_DIR, when set, is where the
# build keeps its objects, as for frameworks/axum/build.sh.
set -euo pipefail
cd "$(dirname "$0")"
if ! command -v cargo >/dev/null 2>&1; then
    echo "benchcli-rust: cargo not found; install Rust (https://rustup.rs) or use BENCH_CLIENT=benchcli-go" >&2
    exit 1
fi
output=${1:-../output/bin/bench.client}
cargo build --release --locked
target_dir=${CARGO_TARGET_DIR:-target}
mkdir -p "$(dirname "$output")"
cp "$target_dir/release/benchcli-rust" "$output"
