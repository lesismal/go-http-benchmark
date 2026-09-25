#!/bin/bash

# . ./script/env.sh

clients_status=0

if bench_owns_servers; then
    check_server_ports_reserved
else
    echo "servers are on ${BENCH_SERVER_HOST} and stay up for the whole run;"
    echo "stop them there with script/killall.sh when it is done"
fi

first_framework=true
for f in ${frameworks[@]}; do
    if [ "$first_framework" != true ]; then
        sleep_between_runs
    fi
    first_framework=false
    echo
    # One server at a time, and only for its own turn: started right before
    # its client, stopped right after, so that no other framework's server
    # holds CPU, memory or sockets while this one is measured. Only the
    # machine that starts a server can stop it, and only it should: killing
    # by process name on a shared client node would reach whatever else is
    # running there.
    if bench_owns_servers; then
        if ! start_server "$f"; then
            clients_status=1
            continue
        fi
    fi
    # echo "start bench ${f}" "$@"
    echo "run client to ${f} at ${BENCH_SERVER_HOST}, on cpu ${client_cpu_list:-unbound}"
    # -ip first, so a host given on the command line still wins: both clients
    # take the last value of a repeated flag.
    . ./script/client.sh -f=$f -ip=${BENCH_SERVER_HOST} "$@" || clients_status=$?
    if bench_owns_servers; then
        stop_server "$f"
    fi
done

return "$clients_status" 2>/dev/null || exit "$clients_status"
