#!/usr/bin/env bash
# Builds the workflow server to $1 (default ../../output/bin/workflow.server).
#
# Needs git, cmake, a C++ compiler and OpenSSL's headers and libraries, which
# workflow links against even for plain HTTP (on Debian: git cmake g++
# libssl-dev; on macOS: brew install cmake openssl). workflow itself is built
# from source, as a static library, at the release pinned below: cloned from
# WORKFLOW_REPO (default GitHub; script/docker_benchmark_cn.sh points it at
# the Gitee mirror), checked against the pinned commit, and built once into
# WORKFLOW_BUILD_DIR (default ./target). Later builds reuse it, which is how
# script/Dockerfile.benchmark builds the library into the image so that a
# build in the container needs no network.
set -euo pipefail
cd "$(dirname "$0")"

WORKFLOW_VERSION=v1.1.0
WORKFLOW_COMMIT=3d5739960ac34d6164d5fec0abf268e83d72fe93
WORKFLOW_REPO=${WORKFLOW_REPO:-https://github.com/sogou/workflow.git}

for tool in git cmake; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "workflow: $tool not found; install it or leave workflow out with BENCH_FRAMEWORKS" >&2
        exit 1
    fi
done
cxx=${CXX:-c++}
if ! command -v "$cxx" >/dev/null 2>&1; then
    echo "workflow: C++ compiler $cxx not found; install one or leave workflow out with BENCH_FRAMEWORKS" >&2
    exit 1
fi

output=${1:-../../output/bin/workflow.server}
build_dir=${WORKFLOW_BUILD_DIR:-target}
mkdir -p "$build_dir"
build_dir=$(cd "$build_dir" && pwd)
src_dir="$build_dir/workflow-$WORKFLOW_VERSION"
inc_dir="$build_dir/include"
lib_dir="$build_dir/lib"

# OpenSSL where Homebrew keeps it, since macOS has no headers of its own for
# it; elsewhere cmake and the compiler find the system's.
openssl_root=${OPENSSL_ROOT_DIR:-}
if [ -z "$openssl_root" ] && command -v brew >/dev/null 2>&1; then
    openssl_root=$(brew --prefix openssl@3 2>/dev/null || true)
fi

if [ ! -f "$lib_dir/libworkflow.a" ]; then
    if [ ! -d "$src_dir/.git" ]; then
        rm -rf "$src_dir"
        echo "workflow: cloning $WORKFLOW_VERSION from $WORKFLOW_REPO"
        git -c advice.detachedHead=false clone --quiet --depth 1 \
            --branch "$WORKFLOW_VERSION" "$WORKFLOW_REPO" "$src_dir"
    fi
    commit=$(git -C "$src_dir" rev-parse HEAD)
    if [ "$commit" != "$WORKFLOW_COMMIT" ]; then
        echo "workflow: $WORKFLOW_REPO $WORKFLOW_VERSION is $commit, want $WORKFLOW_COMMIT" >&2
        exit 1
    fi
    cmake_args=(
        -S "$src_dir" -B "$build_dir/cmake"
        -DCMAKE_BUILD_TYPE=Release
        -DCMAKE_CXX_COMPILER="$cxx"
        -DINC_DIR="$inc_dir"
        -DLIB_DIR="$lib_dir"
    )
    if [ -n "$openssl_root" ]; then cmake_args+=(-DOPENSSL_ROOT_DIR="$openssl_root"); fi
    cmake "${cmake_args[@]}" >/dev/null
    jobs=$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)
    cmake --build "$build_dir/cmake" --target workflow-static -j "$jobs"
fi

link_flags=(-lssl -lcrypto -lpthread)
cxx_flags=(-std=c++11 -O3 -DNDEBUG -Wall -I"$inc_dir")
# As workflow's own CMakeLists.txt does on macOS, whose SDK marks the
# sprintf in workflow's headers deprecated.
if [ "$(uname -s)" = Darwin ]; then cxx_flags+=(-Wno-deprecated-declarations); fi
if [ -n "$openssl_root" ]; then
    cxx_flags+=(-I"$openssl_root/include")
    link_flags=(-L"$openssl_root/lib" "${link_flags[@]}")
fi
mkdir -p "$(dirname "$output")"
"$cxx" "${cxx_flags[@]}" -o "$output" main.cc "$lib_dir/libworkflow.a" "${link_flags[@]}"
