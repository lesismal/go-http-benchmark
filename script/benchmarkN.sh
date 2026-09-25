#!/bin/bash

# Every framework through the Connections x BodySize x BenchTime matrix in
# script/config.sh, one report per combination, each named by its suffix:
# output/report/BenchEcho_<conns>_<payload>_<times>.md and so on.

. ./script/env.sh || { return 1 2>/dev/null || exit 1; }

echo $line
. ./script/clean.sh

echo $line

print_env

echo $line

. ./script/build.sh || { return 1 2>/dev/null || exit 1; }

echo $line

. ./script/killall.sh
# The servers and the benchmark client take different flags, and this script
# takes the client's. Forward only what a server actually defines.
server_flags=""
for arg in "$@"; do
    case "$arg" in
        -nodelay=*|-reuseport=*|-b=*|-m=*) server_flags="${server_flags} ${arg}" ;;
    esac
done

# A server node starts every server now and leaves them up for the client
# node. On a single node, each one is started below for its own turn only.
if bench_runs_servers && ! bench_owns_servers; then
    . ./script/servers.sh
fi
echo $line

if ! bench_runs_clients; then
    echo "servers are up and left running. On the client node:"
    echo "  BENCH_ROLE=client BENCH_SERVER_HOST=<this host> bash script/benchmarkN.sh"
    echo "Stop them here afterwards with: bash script/killall.sh"
    return 0 2>/dev/null || exit 0
fi

if bench_owns_servers; then
    check_server_ports_reserved
fi

# A pause before every run but the first, so none after the last. Between two
# frameworks it falls after the one's server has exited and before the next
# one's starts.
first_run=true
for f in ${frameworks[@]}; do
    if [ "$first_run" != true ]; then
        sleep_between_runs
    fi
    if bench_owns_servers; then
        start_server "$f" || { return 1 2>/dev/null || exit 1; }
    fi
    first_combination=true
    for c in ${Connections[@]}; do
        for b in ${BodySize[@]}; do
            for n in ${BenchTime[@]}; do
                if [ "$first_combination" != true ]; then
                    sleep_between_runs
                fi
                first_combination=false
                first_run=false
                suffix="_${c}_${b}_${n}"
                echo "run client to ${f} at ${BENCH_SERVER_HOST}: ${c} connections, ${b} payload, ${n} times"
                . ./script/client.sh -f=$f -ip=${BENCH_SERVER_HOST} -c=$c -b=$b -en=$n -suffix=${suffix} -rate=true "$@" || { return 1 2>/dev/null || exit 1; }
            done
        done
    done
    suffix=""
    if bench_owns_servers; then
        stop_server "$f"
    fi
done

# The report step reads BENCH_REPORT_SORT for the row order of its tables; see
# script/config.sh.
for c in ${Connections[@]}; do
    for b in ${BodySize[@]}; do
        for n in ${BenchTime[@]}; do
            suffix="_${c}_${b}_${n}"
            . ./script/report.sh -suffix=${suffix} "$@"
        done
    done
done
