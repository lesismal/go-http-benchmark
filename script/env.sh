#!/bin/bash

# Guarded, so that config.sh's checks on BENCH_ROLE, BENCH_REPORT_SORT and
# BENCH_FRAMEWORKS actually stop a run. They each print and "return 1", but a
# sourced script's return only sets $? in its caller, so without this an entry
# point would get 0 back from the function definition at the end of this file
# and run the whole benchmark on a value it had already rejected. Every caller
# of env.sh guards it the same way; docker_benchmark.sh guards config.sh
# directly.
. ./script/config.sh || return 1

# Which half of the benchmark this machine runs; see BENCH_ROLE in config.sh.
# The drivers ask through these rather than each testing the variable.
bench_runs_servers() { [ "$BENCH_ROLE" != client ]; }
bench_runs_clients() { [ "$BENCH_ROLE" != server ]; }
# The servers are ours to stop only when we are the machine that started them.
bench_owns_servers() { [ "$BENCH_ROLE" = both ]; }

# Only a single-node run has two halves to divide the CPUs between. On a node
# that runs one of them, pinning it to half a machine nothing else is using
# would leave the other half idle, so the default there is the whole node; an
# explicit list still pins it, for a node that shares its CPUs.
split_cpus=true
if ! bench_runs_servers || ! bench_runs_clients; then
    if [ -z "${BENCH_SERVER_CPU_LIST:-}" ] && [ -z "${BENCH_CLIENT_CPU_LIST:-}" ]; then
        split_cpus=false
    fi
fi

if [ "$split_cpus" = true ] && command -v taskset >/dev/null 2>&1; then
    if [ -n "${BENCH_SERVER_CPU_LIST:-}" ] && [ -n "${BENCH_CLIENT_CPU_LIST:-}" ]; then
        # Docker supplies lists from the daemon's effective cpuset. This avoids
        # selecting host CPUs that are not available inside the container.
        server_cpu_list=$BENCH_SERVER_CPU_LIST
        client_cpu_list=$BENCH_CLIENT_CPU_LIST
    else
        total_cpu_num=$(getconf _NPROCESSORS_ONLN)
        server_cpu_num=$((total_cpu_num / 2 - 1))
        client_cpu_num=$((server_cpu_num + 1))
        server_cpu_list="0-${server_cpu_num}"
        client_cpu_list="${client_cpu_num}-$((total_cpu_num - 1))"

        if command -v lscpu >/dev/null 2>&1; then
            topology=$(lscpu -p=CPU,CORE,SOCKET,NODE | awk -F, '$1 !~ /^#/ {print $1 "," $2 "," $3 "," $4}')
            socket_count=$(printf '%s\n' "$topology" | awk -F, '$3 >= 0 {seen[$3] = 1} END {print length(seen)}')
            node_count=$(printf '%s\n' "$topology" | awk -F, '$4 >= 0 {seen[$4] = 1} END {print length(seen)}')
            server_topology_cpus=""
            client_topology_cpus=""
            server_topology_count=0
            client_topology_count=0

            append_cpu_group() {
                target=$1
                cpus=$2
                count=$(printf '%s\n' "$cpus" | awk -F, '{print NF}')
                if [ "$target" = server ]; then
                    server_topology_cpus="${server_topology_cpus}${server_topology_cpus:+,}${cpus}"
                    server_topology_count=$((server_topology_count + count))
                else
                    client_topology_cpus="${client_topology_cpus}${client_topology_cpus:+,}${cpus}"
                    client_topology_count=$((client_topology_count + count))
                fi
            }

            if [ "$socket_count" -ge 2 ]; then
                mapfile -t groups < <(printf '%s\n' "$topology" | awk -F, '$3 >= 0 {print $3}' | sort -n -u)
                group_column=3
            elif [ "$node_count" -ge 2 ]; then
                mapfile -t groups < <(printf '%s\n' "$topology" | awk -F, '$4 >= 0 {print $4}' | sort -n -u)
                group_column=4
            else
                mapfile -t groups < <(printf '%s\n' "$topology" | awk -F, '{print $3 ":" $2}' | sort -t: -k1,1n -k2,2n -u)
                group_column=core
            fi

            for group in "${groups[@]}"; do
                if [ "$group_column" = core ]; then
                    socket=${group%%:*}
                    core=${group#*:}
                    group_cpus=$(printf '%s\n' "$topology" | awk -F, -v socket="$socket" -v core="$core" '$3 == socket && $2 == core {print $1}' | paste -sd, -)
                else
                    group_cpus=$(printf '%s\n' "$topology" | awk -F, -v column="$group_column" -v group="$group" '$column == group {print $1}' | paste -sd, -)
                fi
                if [ "$server_topology_count" -le "$client_topology_count" ]; then
                    append_cpu_group server "$group_cpus"
                else
                    append_cpu_group client "$group_cpus"
                fi
            done

            if [ -n "$server_topology_cpus" ] && [ -n "$client_topology_cpus" ]; then
                server_cpu_list=$server_topology_cpus
                client_cpu_list=$client_topology_cpus
            fi
        fi
    fi

    limit_cpu_server="taskset -c ${server_cpu_list}"
    limit_cpu_client="taskset -c ${client_cpu_list}"
fi

# debug
# echo "limit_cpu_server: ${server_cpu_list}, ${limit_cpu_server}"
# echo "limit_cpu_client: ${client_cpu_list}, ${limit_cpu_client}"

line=$(printf "%0.s-" {1..62})

# Warns when this machine can hand a later server's ports out as ephemeral
# ones, which stops that server with "address already in use"; see
# server_port_range in config.sh. Linux only: elsewhere the default ephemeral
# range starts above every server port.
check_server_ports_reserved() {
    [ -r /proc/sys/net/ipv4/ip_local_port_range ] || return 0
    local range first last low high entry a b
    range=$(server_port_range)
    [ -n "$range" ] || return 0
    first=${range%-*}
    last=${range#*-}
    read -r low high < /proc/sys/net/ipv4/ip_local_port_range
    if [ "$high" -lt "$first" ] || [ "$low" -gt "$last" ]; then
        return 0
    fi
    for entry in $(tr ',' ' ' < /proc/sys/net/ipv4/ip_local_reserved_ports 2>/dev/null); do
        a=${entry%-*}
        b=${entry#*-}
        if [ "$a" -le "$first" ] && [ "$b" -ge "$last" ]; then
            return 0
        fi
    done
    echo "warning: the ephemeral port range ${low}-${high} takes in the servers' ports ${range};"
    echo "a server started after other frameworks' runs may find its ports taken. Reserve them with:"
    echo "  sysctl -w net.ipv4.ip_local_reserved_ports=${range}"
}

# Whether framework $1's server is running on this machine, matched the way
# killone.sh's pkill matches it.
server_running() {
    pgrep -f "[/]output/bin/${1}\.server([[:space:]]|\$)" >/dev/null 2>&1
}

# Whether something accepts a TCP connection on local port $1.
port_open() {
    (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null
}

# How long start_server waits for a server to listen, and stop_server for one
# to exit before it is killed, in seconds.
ServerStartTimeout=${ServerStartTimeout:-60}
ServerStopTimeout=${ServerStopTimeout:-10}

# Starts framework $1's server with $server_flags and returns once it listens
# on every benchmark port and on the control port after them, plus one second
# for it to settle. Fails, with the end of its log, if it exits first or is
# not up within ServerStartTimeout seconds, when it is stopped.
start_server() {
    local f=$1 ports first last port waited=0
    if ! ports=$(server_ports "$f") || [ -z "$ports" ]; then
        echo "no ports for ${f} in config.Ports in config/config.go" >&2
        return 1
    fi
    read -r first last <<< "$ports"
    ./script/server.sh "$f" $server_flags
    # The control port is last + 1; see config.GetFrameworkControlServerAddr.
    for ((port = first; port <= last + 1; port++)); do
        while ! port_open "$port"; do
            # Checked from the second second on: until server.sh's fork has
            # exec'd, there is no process by the server's name to find.
            if [ "$waited" -ge 10 ] && ! server_running "$f"; then
                echo "${f} server: exited before listening on :${port}" >&2
                tail -n 20 "./output/log/${preffix}${f}${suffix}.log" >&2 2>/dev/null
                return 1
            fi
            if [ "$waited" -ge $((ServerStartTimeout * 10)) ]; then
                echo "${f} server: not listening on :${port} after ${ServerStartTimeout}s" >&2
                tail -n 20 "./output/log/${preffix}${f}${suffix}.log" >&2 2>/dev/null
                stop_server "$f"
                return 1
            fi
            sleep 0.1
            waited=$((waited + 1))
        done
    done
    echo "${f} server: listening on :${first} to :$((last + 1))"
    sleep 1
}

# Stops framework $1's server with killone.sh's SIGINT, which lets it flush
# its last statistics, and waits for it to exit, so that the next framework
# does not start while this one still holds CPU, memory or sockets. SIGKILL
# after ServerStopTimeout seconds.
stop_server() {
    local f=$1 waited=0
    . ./script/killone.sh "${f}.server"
    while server_running "$f"; do
        if [ "$waited" -ge $((ServerStopTimeout * 10)) ]; then
            echo "${f} server: still running after ${ServerStopTimeout}s, kill -9"
            pkill -9 -f "[/]output/bin/${f}\.server([[:space:]]|\$)"
            break
        fi
        sleep 0.1
        waited=$((waited + 1))
    done
}

# The pause between two frameworks, or two runs of one, so that the system
# settles from the last run - sockets in TIME_WAIT, memory given back - before
# the next. Never after the last run: nothing follows it.
sleep_between_runs() {
    local i
    for ((i = 1; i <= SleepTime; i++)); do
        echo "sleep $i ..."
        sleep 1
    done
}

clean() {
    rm -rf ./output
    for f in ${frameworks[@]}; do
        killall -9 "${f}.server" 1>/dev/null 2>&1
    done
}

print_env() {
    echo "os:"
    echo
    if [ -r /etc/issue ]; then cat /etc/issue; else uname -srm; fi
    echo $line
    echo "cpu model:"
    echo
    if [ -r /proc/cpuinfo ]; then
        grep "model name" /proc/cpuinfo | uniq
    else
        sysctl -n machdep.cpu.brand_string 2>/dev/null || echo unknown
    fi
    echo $line
    echo "processors: $(getconf _NPROCESSORS_ONLN)"
    echo $line
    if command -v free >/dev/null 2>&1; then free; else echo "memory: $(sysctl -n hw.memsize 2>/dev/null || echo unknown) bytes"; fi
    echo $line
    echo "server cpus: ${server_cpu_list:-unbound}"
    echo "client cpus: ${client_cpu_list:-unbound}"
    echo $line
    echo "role: ${BENCH_ROLE} (servers: $(bench_runs_servers && echo here || echo elsewhere), clients: $(bench_runs_clients && echo here || echo elsewhere))"
    echo "server host: ${BENCH_SERVER_HOST}"
    echo $line
    echo "benchmark client: ${BENCH_CLIENT}"
    echo "frameworks: ${frameworks[*]}"
    echo $line
    echo "report sort: ${BENCH_REPORT_SORT} (result = best first, framework = config.FrameworkList order)"
    echo $line
    echo "go env:"
    echo
    go env
}
