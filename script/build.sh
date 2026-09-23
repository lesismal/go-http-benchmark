#!/bin/bash

# Also support invoking this script directly from the repository root.
if ! declare -p frameworks >/dev/null 2>&1 || ! declare -F bench_runs_servers >/dev/null 2>&1; then
    . ./script/env.sh || { return 1 2>/dev/null || exit 1; }
fi

build_benchmark() {
    . ./script/clean.sh
    mkdir -p ./output/bin ./output/log ./output/report || return 1

    # Each half only builds what it runs.
    if bench_runs_servers; then
        for f in "${frameworks[@]}"; do
            echo "build ${f} ..."
            go build -o "./output/bin/${f}.server" "./frameworks/${f}" || return 1
            echo "build ${f} done"
            echo
        done
    else
        echo "skip building the servers: they run on ${BENCH_SERVER_HOST}"
        echo
    fi

    if bench_runs_clients; then
        echo "build client: benchcli-go ..."
        go build -o ./output/bin/bench.client ./benchcli-go || return 1
        echo "build client done"
    else
        echo "skip building the client: it runs elsewhere"
    fi
}

build_benchmark || { return 1 2>/dev/null || exit 1; }
