#!/bin/bash

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
# than being killed after its turn: stop them on the server node afterwards
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
#              answers with: TPS in all three - for BenchRate the responses the
#              clients read back off the server per second - with EER breaking
#              a tie in BenchEcho and BenchRate (Connections samples no CPU, so
#              it has none). The rate test pipelines requests at a rate the
#              clients set rather than to completion, so what came back under
#              that load is its result there the way TPS is in the other two;
#              Req Sent is the load rather than the answer
#   framework  the order FrameworkList in config/config.go lists them in,
#              which is by framework name. It is what puts a framework on the
#              same row in every table and across runs, whatever it scored, so
#              two reports can be diffed. Only the Go list reaches a report;
#              the frameworks array below decides what is built and run, and is
#              kept in the same order so that the two read alike
#
# In either order every ranked column - TPS, and EER - shows each row's share
# of the best in that column after it, the best being 100%, and carries [↓1]
# or [↓2] after its title for which key it is. Rows that tie keep the
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
SleepTime=5

# Which frameworks a run measures, and the order the servers are started and
# the clients run in. In framework-name order, like config.FrameworkList, so
# that a framework is in the same place in every list and a new one has one
# obvious place to go.
#
#   fasthttp  github.com/valyala/fasthttp
#   fib       github.com/lesismal/fib/go, its HTTP/1 server (fib/go/http)
#   gin       github.com/gin-gonic/gin, served by net/http
#   nethttp   the standard library's net/http
frameworks=(
    "fasthttp"
    "fib"
    "gin"
    "nethttp"
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
