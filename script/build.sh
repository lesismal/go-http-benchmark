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
            case "${f}" in
                axum) bash ./frameworks/axum/build.sh "$(pwd)/output/bin/${f}.server" || return 1 ;;
                workflow) bash ./frameworks/workflow/build.sh "$(pwd)/output/bin/${f}.server" || return 1 ;;
                *) go build -o "./output/bin/${f}.server" "./frameworks/${f}" || return 1 ;;
            esac
            echo "build ${f} done"
            echo
        done
    else
        echo "skip building the servers: they run on ${BENCH_SERVER_HOST}"
        echo
    fi

    if bench_runs_clients; then
        # The Go client is also the report step's, whichever client measures:
        # script/report.sh runs it as bench.report.
        echo "build report: benchcli-go ..."
        go build -o ./output/bin/bench.report ./benchcli-go || return 1
        echo "build client: ${BENCH_CLIENT} ..."
        case "$BENCH_CLIENT" in
            benchcli-go) cp ./output/bin/bench.report ./output/bin/bench.client || return 1 ;;
            benchcli-rust) bash ./benchcli-rust/build.sh "$(pwd)/output/bin/bench.client" || return 1 ;;
        esac
        echo "build client done"
    else
        echo "skip building the client: it runs elsewhere"
    fi
}

build_benchmark || { return 1 2>/dev/null || exit 1; }
