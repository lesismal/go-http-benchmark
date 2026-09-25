#!/bin/bash

# Benchmark client: benchcli-rust (default) or benchcli-go. Both run the same
# three benchmarks with the same flags and write the same JSON report files;
# the report step turns them into tables with the Go client either way, so
# one run's tables can hold rows from both. benchcli-rust needs cargo; on a
# machine without it, use the Go one.
# Override for one run with: BENCH_CLIENT=benchcli-go bash script/benchmark.sh
BENCH_CLIENT=${BENCH_CLIENT:-benchcli-rust}
case "$BENCH_CLIENT" in
    benchcli-go|benchcli-rust) ;;
    *) echo "Unsupported BENCH_CLIENT: $BENCH_CLIENT (want benchcli-go or benchcli-rust)" >&2; return 1 ;;
esac

# Where the servers are, as the clients should reach them: an address or a
# hostname, IPv6 included. The default keeps a single-node run on loopback.
# The servers always bind every interface, so a two-node run configures only
# this side.
# Override for one run with: BENCH_SERVER_HOST=10.0.0.2 bash script/benchmark.sh
BENCH_SERVER_HOST=${BENCH_SERVER_HOST:-127.0.0.1}

# Which half of the benchmark this machine runs:
#
#   both    (default) build everything, start the servers, run the clients
#           against them and write the report - one machine
#   server  build and start the servers, then leave them running. Nothing is
#           measured here; the client node does that
#   client  build the client only, run it against BENCH_SERVER_HOST and write
#           the report. Nothing is started or stopped here
#
# A two-node run is BENCH_ROLE=server on one machine and, once it reports the
# servers are up, BENCH_ROLE=client BENCH_SERVER_HOST=<that machine> on the
# other. "client" against a loopback host is also the way to run the clients
# again without restarting servers that are already up on this machine.
#
# Two things differ from a single-node run. The client cannot stop a server it
# did not start, so every framework's server stays up for the whole run rather
# than being started right before its turn and stopped right after it, as on
# one node: stop them on the server node afterwards
# with script/killall.sh, and use BENCH_FRAMEWORKS below if the idle ones
# holding memory would disturb the framework being measured. And each node
# gives the whole machine to its own half, since there is no longer anything
# to divide it with; BENCH_SERVER_CPU_LIST and BENCH_CLIENT_CPU_LIST still
# pin it where a node shares its CPUs with something else.
BENCH_ROLE=${BENCH_ROLE:-both}
case "$BENCH_ROLE" in
    both|server|client) ;;
    *) echo "Unsupported BENCH_ROLE: $BENCH_ROLE (want both, server or client)" >&2; return 1 ;;
esac

# The order the report tables put their rows in. Both orders carry the same
# rows and the same numbers; only the order differs:
#
#   result     (default) best first, ranked by the number each benchmark
#              answers with: TPS in all three - for BenchPipeline the
#              responses the clients read back off the server per second -
#              with CPU EER, then MEM EER, breaking a tie in BenchEcho and
#              BenchPipeline (Connections samples no CPU or memory, so it has
#              neither). The rate test
#              pipelines requests at a rate the clients set rather than to
#              completion, so what came back under that load is its result
#              there the way TPS is in the other two; Req Sent is the load
#              rather than the answer
#   framework  the order FrameworkList in config/config.go lists them in,
#              which is by framework name. It is what puts a framework on the
#              same row in every table and across runs, whatever it scored, so
#              two reports can be diffed. Only the Go list reaches a report;
#              the frameworks array below decides what is built and run, and is
#              kept in the same order so that the two read alike
#
# In either order every ranked column - TPS, CPU EER and MEM EER - shows each
# row's share of the best in that column after it, the best being 100%, and
# carries [↓1], [↓2] or [↓3] after its title for which key it is. Rows that tie keep the
# framework order between them, so two frameworks that scored the same - or a
# whole table from a benchmark that did not run, which leaves every row at
# zero - come out the same way on every run.
#
# Override for one run with: BENCH_REPORT_SORT=framework bash script/benchmark.sh
# or, without re-running the benchmark, by passing the client flag straight to
# the report step: bash script/report.sh -sort=framework
BENCH_REPORT_SORT=${BENCH_REPORT_SORT:-result}
case "$BENCH_REPORT_SORT" in
    result|framework) ;;
    *) echo "Unsupported BENCH_REPORT_SORT: $BENCH_REPORT_SORT (want result or framework)" >&2; return 1 ;;
esac

# The matrix script/benchmarkN.sh runs every framework through.
Connections=(5000 50000)
BodySize=(512 1024)
BenchTime=(2000000)
# Seconds between two runs, once the last one's server has exited: none after
# the last run. A server is up when it listens on all its ports, and its client
# starts one second after that.
SleepTime=5

# Which frameworks a run measures, and the order the servers are started and
# the clients run in. In framework-name order, like config.FrameworkList, so
# that a framework is in the same place in every list and a new one has one
# obvious place to go.
#
#   axum        github.com/tokio-rs/axum, a Rust server on tokio; building
#               it needs cargo (see frameworks/axum/build.sh)
#   beego       github.com/beego/beego/v2 (formerly github.com/astaxie/beego),
#               its router, served by net/http
#   chi         github.com/go-chi/chi/v5, served by net/http
#   echo        github.com/labstack/echo/v5, served by net/http
#   fasthttp    github.com/valyala/fasthttp
#   fib         github.com/lesismal/fib, its HTTP/1 server (fib/http)
#   fiber       github.com/gofiber/fiber/v3, served by fasthttp
#   gin         github.com/gin-gonic/gin, served by net/http
#   goji        github.com/zenazn/goji, its web.Mux, served by net/http
#   gorillamux  github.com/gorilla/mux, served by net/http
#   hertz       github.com/cloudwego/hertz, on its netpoll transport
#               (github.com/cloudwego/netpoll)
#   httprouter  github.com/julienschmidt/httprouter, served by net/http
#   nbio        github.com/lesismal/nbio, its nbhttp server in
#               IOModNonBlocking, serving nethttp's net/http handler
#   nethttp     the standard library's net/http
#   workflow    github.com/sogou/workflow, a C++ server: its HTTP server
#               benchmark answering with the request body; building it needs
#               git, cmake, a C++ compiler and OpenSSL (see
#               frameworks/workflow/build.sh)
frameworks=(
    "axum"
    "beego"
    "chi"
    "echo"
    "fasthttp"
    "fib"
    "fiber"
    "gin"
    "goji"
    "gorillamux"
    "hertz"
    "httprouter"
    "nbio"
    "nethttp"
    "workflow"
)

# Optional comma-separated subset, used by the Docker smoke test and useful for
# focused local runs. Reject unknown names before they reach build paths.
if [ -n "${BENCH_FRAMEWORKS:-}" ]; then
    all_frameworks=("${frameworks[@]}")
    IFS=',' read -r -a requested_frameworks <<< "$BENCH_FRAMEWORKS"
    frameworks=()
    for requested_framework in "${requested_frameworks[@]}"; do
        framework_found=false
        for available_framework in "${all_frameworks[@]}"; do
            if [ "$requested_framework" = "$available_framework" ]; then
                framework_found=true
                break
            fi
        done
        if [ "$framework_found" != true ]; then
            echo "Unsupported framework in BENCH_FRAMEWORKS: $requested_framework" >&2
            return 1
        fi
        frameworks+=("$requested_framework")
    done
    if [ "${#frameworks[@]}" -eq 0 ]; then
        echo "BENCH_FRAMEWORKS must select at least one framework" >&2
        return 1
    fi
fi

# The first and last benchmark port of framework $1, "10401 10450", read from
# config.Ports in config/config.go rather than copied here: first the constant
# whose value is the name, then that constant's range in the map.
# TestScriptServerPorts holds this to config.Ports. Here, with
# server_port_range below, rather than in env.sh: docker_benchmark.sh reads
# only this file.
server_ports() {
    local name
    name=$(sed -n "s/^[[:space:]]*\([A-Za-z0-9_]*\)[[:space:]]*=[[:space:]]*\"$1\"[[:space:]]*\$/\1/p" ./config/config.go | head -n 1)
    [ -n "$name" ] || return 1
    sed -n "s/^[[:space:]]*${name}:[[:space:]]*\"\([0-9]*\):\([0-9]*\)\",\{0,1\}[[:space:]]*\$/\1 \2/p" ./config/config.go | head -n 1
}

# Every port a server listens on, of every framework in config.Ports - its
# benchmark ports and the control port after them - as the one range
# "10001-11451" that covers them all.
#
# A single-node run starts each server only for its own turn, after other
# frameworks' clients have already dialed tens of thousands of connections.
# With an ephemeral port range that takes these in, as the "1024 65535" the
# README and docker_benchmark.sh set does, those connections' local ports -
# and the TIME_WAIT sockets they leave for a minute after them - sit on ports
# a later server needs, and it exits with "address already in use". Reserving
# the range, with net.ipv4.ip_local_reserved_ports, keeps the kernel from
# handing them out as ephemeral ports.
server_port_range() {
    sed -n '/^var Ports = map/,/^}/p' ./config/config.go |
        sed -n 's/^[[:space:]]*[A-Za-z0-9_]*:[[:space:]]*"\([0-9]*\):\([0-9]*\)",\{0,1\}[[:space:]]*$/\1 \2/p' |
        awk 'NR == 1 || $1 < min { min = $1 } NR == 1 || $2 > max { max = $2 } END { if (NR) print min "-" max + 1 }'
}
