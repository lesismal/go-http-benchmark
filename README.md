# go-http-benchmark

HTTP/1.1 server benchmark for Go frameworks, with Rust's axum as a
reference, built the same way as
[go-websocket-benchmark](https://github.com/lesismal/go-websocket-benchmark):
the same scripts, the same Go client structure, and the same report format.

| Framework | Package | Server |
| --- | --- | --- |
| `axum` | [axum](https://crates.io/crates/axum) (Rust) | `axum::serve` on tokio's multi-threaded runtime, one listener per port, one worker thread per CPU the process may run on |
| `fasthttp` | [github.com/valyala/fasthttp](https://github.com/valyala/fasthttp) | one `fasthttp.Server` serving every port |
| `fib` | [github.com/lesismal/fib/go](https://github.com/lesismal/fib) | one fib engine bound to every port, HTTP/1 handler from `fib/go/http` |
| `gin` | [github.com/gin-gonic/gin](https://github.com/gin-gonic/gin) | `gin.New()` (no logger or recovery middleware) on `net/http` |
| `nethttp` | `net/http` | one `http.Server` per port, all sharing one `ServeMux` |

Every server answers `POST /echo` with the request body, byte for byte, with a
`Content-Length`. Each one listens on 50 ports (see `config.Ports`) so that a
client dialing a million connections from one address does not run out of
ephemeral ports toward any one port. `/init`, `/ps` and `/debug/pprof/` are on
a separate `net/http` server on the port after the last benchmark port. That
way the framework being measured serves nothing but `/echo`, and control
requests never wait behind benchmark requests.

`axum` is a Rust program in [`frameworks/axum`](frameworks/axum), so it cannot
share the Go servers' control server. It serves `/init` and `/ps` itself, on
its own control port, in the same JSON shape (only the `cpu` and `mem[].rss`
fields, which are all the clients read). It has no `/debug/pprof/`, so the
client's pprof fetch fails for axum and logs it. It takes the same `-nodelay`,
`-reuseport` and `-b` flags; `-m` is accepted and ignored, since Rust has no
GC to limit. Its port range is a constant in `src/main.rs`, which a test in
`config` holds to `config.Ports`.

## What is measured

The Go client, [`benchcli-go`](benchcli-go), runs three benchmarks one after
another on the same keep-alive connections:

| Benchmark | What it does | TPS is |
| --- | --- | --- |
| `Connections` | dials `-c` TCP connections, `-dc` at a time, and sends one `GET /echo` on each | connections established and answered per second |
| `BenchEcho` | `-en` request/response round trips: a `POST /echo` with `-b` random bytes, then the response is read back, with one request in flight per connection and `-ec` connections busy at once | round trips per second |
| `BenchPipeline` | HTTP/1.1 pipelining for `-rd` seconds: each connection gets `-rr` requests a second, written in batches of `-rpl` requests (or, with `-rpl=0`, as many as fit in `-rbs` bytes) without waiting for earlier responses; a goroutine per connection reads the responses | responses read back per second |

`Connections` sends a request on each connection because an HTTP server has no
handshake of its own, and a connection the kernel accepted is not yet one the
server is serving. That GET is the equivalent of the WebSocket upgrade.

`BenchPipeline` limits each connection to four unanswered batches. When the server
falls behind the rate, the client skips that connection for a tick instead of
queueing more requests, so a slow server is measured by what it answered, not
by how deep a backlog the client built. When the duration is up, the client
waits up to one more tick for the last batch, then counts. `Rate Pipeline` in
the Summary table is the number of requests merged into one write. Set it with
`-rpl`, which has to divide `-rr` so that the batch goes out a whole number of
times a second. The default, `-rpl=0`, uses the most requests that fit in
`-rbs` bytes (16KB) and divide `-rr`, which is 10 for the default 1KB payload
and 200 requests a second:

```sh
bash script/benchmark.sh -rr=200 -rpl=50   # 50 requests per write, 4 writes a second
```

The client writes pre-encoded requests and parses responses with a small
reader ([`protocol`](benchcli-go/protocol/http.go)) instead of `net/http`'s
client. On a single-node run, whatever the client spends on each request
comes out of the machine the server runs on. `-check=true` compares every
response body with the request that was sent.

`EER` is throughput per percent of a CPU core: `TPS / CPU Avg`. The server's
CPU and memory are sampled every `-pi` ms. `-ps=auto` (the default) samples the
server process from the client side when it runs on the same machine, and asks
the server's `/ps` route when it does not; `local` and `remote` force one or
the other. A phase shorter than one sampling interval has no samples, and its
CPU, MEM and EER columns read 0. The client logs a message when that happens.

## Run

Go 1.27 or later, and for `axum` a Rust toolchain (cargo 1.85 or later; see
[rustup.rs](https://rustup.rs)). Without cargo, leave axum out with
`BENCH_FRAMEWORKS`, e.g. `BENCH_FRAMEWORKS=fasthttp,fib,gin,nethttp`. From the
repository root:

```sh
# all frameworks, 10k connections, 1k payload
bash script/benchmark.sh

# a subset, with client flags
BENCH_FRAMEWORKS=fasthttp,fib bash script/benchmark.sh -c=10000 -en=2000000 -b=1024

# every framework through the Connections x BodySize x BenchTime matrix in script/config.sh
bash script/benchmarkN.sh

# 1m connections (needs the system settings below)
bash script/1m_conns_benchmark.sh
```

Change the defaults in [`script/config.sh`](script/config.sh). Reports are
written to `output/report`: one JSON file per framework and benchmark, plus
`Summary.md`, `Connections.md`, `BenchEcho.md` and `BenchPipeline.md`. Server logs
are in `output/log`. `benchmark.sh` forwards only `-nodelay`, `-reuseport`,
`-b` and `-m` to the servers. Every other flag goes to the client; run
`go run ./benchcli-go -h` for the list.

On Linux, `script/env.sh` pins the servers and the client to separate halves
of the CPUs with `taskset`, and splits by socket, NUMA node or core when
`lscpu` can tell them apart. Without `taskset` (macOS, for example), nothing is
pinned.

### Docker

The Docker runner reads the CPU set and memory exposed by the Docker daemon,
uses about 75% of its CPUs and 80% of its memory, pins separate CPU groups for
the servers and the client, and copies reports, logs, console output and the
resource plan to `output/docker/<timestamp>`.

```sh
# short nethttp-only validation
bash script/docker_benchmark.sh --smoke

# full benchmark
bash script/docker_benchmark.sh

# focused run with explicit resource limits
BENCH_FRAMEWORKS=fasthttp,fib \
DOCKER_BENCH_CPUS=8 DOCKER_BENCH_MEMORY=12g \
bash script/docker_benchmark.sh -c=10000 -en=2000000 -b=1024
```

Run `bash script/docker_benchmark.sh --help` for all overrides. From mainland
China, use `script/docker_benchmark_cn.sh` instead. It takes the same options
and builds the image from mirrors (DaoCloud for Docker Hub, Aliyun for apt,
goproxy.cn for Go modules, rsproxy.cn for crates.io). Only the build downloads
anything, and the benchmark itself runs with `--network none`: the image
carries the Rust toolchain and has axum's dependencies compiled, so building
axum in the container needs no network.

The [Docker benchmark workflow](.github/workflows/docker-benchmark.yml) runs
the same script on every push to `main`, or by hand from the Actions tab, and
writes the tables to the job summary. The container gets 8 CPUs when the
runner has at least 8, and 4 otherwise, so every CI run is one of those two
sizes. The standard GitHub-hosted runner has 4 CPUs; to get 8, set the
repository variable `DOCKER_BENCH_RUNNER` to the label of a larger runner. A
runner with fewer than 4 CPUs fails the job.

## Report format

The reports have the same layout as go-websocket-benchmark's: a Summary table
of the run's parameters, each with a description of what it means and the
flag that sets it, then one table per benchmark.

- Rows are ranked best first by `TPS`. In `BenchEcho` and `BenchPipeline`, a tie
  is broken by `EER`. The ranked columns carry `[↓1]` and `[↓2]` in their
  titles.
- Every ranked column shows each row's share of the best value in that
  column, floored so that only the best row reads `100%`.
- Parameters shared by every row (`Client`, `Conns`, `Payload`, each
  benchmark's concurrency, `Echo Total`, `Rate Duration`, `Rate SendRate`,
  `Rate Pipeline`) are in the Summary table instead of the columns. A
  parameter the rows disagree on lists each value with its frameworks, for
  example `20000 (fib); 19998 (fasthttp)`.
- The JSON files keep every field, including `TP50`, `TP75`, `TP90`,
  `CPU Min` and `MEM Min`, which the tables leave out.

`BENCH_REPORT_SORT=framework` (or `-sort=framework`) keeps
`config.FrameworkList` order instead, so that a framework is on the same row
in every table and across runs. Neither order changes the numbers, so
re-running the report step alone is enough:

```sh
bash script/report.sh -sort=framework
```

## Two nodes

To run the servers and the client on separate machines, set `BENCH_ROLE` on
each and give the client side the server's address. The servers bind every
interface, so nothing needs configuring on their side.

```sh
# On the server node: builds the servers, starts them, leaves them running.
BENCH_ROLE=server bash script/benchmark.sh

# On the client node: builds the client, runs it against the servers, reports.
BENCH_ROLE=client BENCH_SERVER_HOST=10.0.0.2 bash script/benchmark.sh

# Back on the server node, when the run is over.
bash script/killall.sh
```

A node that runs one half gets the whole machine. The client cannot stop
servers it did not start, so every server stays up for the whole run. Use
`BENCH_FRAMEWORKS` to run a subset if the idle servers would disturb the one
being measured. The CPU and MEM columns come from the servers' own `/ps`
route, because the client cannot see the server process from another machine.

## Before running the test

Set the system limits on every machine the benchmark runs on. The client node
needs the port range and file descriptor limits as much as the server does:

```sh
sysctl -w net.ipv4.ip_local_port_range="1024 65535"
sysctl -w fs.file-max=2000500
sysctl -w fs.nr_open=2000500
sysctl -w net.nf_conntrack_max=2000500
ulimit -n 2000500
sysctl -w net.ipv4.tcp_mem='131072  262144  524288'
sysctl -w net.ipv4.tcp_rmem='8760  256960  4088000'
sysctl -w net.ipv4.tcp_wmem='8760  256960  4088000'
sysctl -w net.core.rmem_max=16384
sysctl -w net.core.wmem_max=16384
sysctl -w net.core.somaxconn=2048
sysctl -w net.ipv4.tcp_max_syn_backlog=2048
sysctl -w /proc/sys/net/core/netdev_max_backlog=2048
sysctl -w net.ipv4.tcp_tw_reuse=1
```

## Sample results

These numbers are from `bash script/docker_benchmark.sh` with the defaults, on
Docker Desktop on an Apple M4 Pro. Docker had 10 CPUs; the run allocated 7:
3 for the servers and 4 for the client. They show what the report looks like.
They are not a reference measurement: re-run on your own hardware.

| Parameter        | Value   |
| ---              | ---     |
| Client           | go      |
| Conns            | 10000   |
| Payload          | 1024    |
| Dial Concurrency | 2000    |
| Echo Concurrency | 10000   |
| Echo Total       | 2000000 |
| Rate Concurrency | 10000   |
| Rate Duration    | 10.00s  |
| Rate SendRate    | 200     |
| Rate Pipeline    | 10      |

| Framework |  TPS [↓1]  |   Min    |   Avg   |   Max    |  TP95   |   TP99   |   Used   | Total | Success | Failed |
|   ---     |    ---     |   ---    |   ---   |   ---    |   ---   |   ---    |   ---    |  ---  |   ---   |  ---   |
| fasthttp  | 48778 100% | 194.79us | 30.01ms | 121.61ms | 88.05ms | 109.95ms | 205.01ms | 10000 |  10000  |   0    |
|   fib     | 48027  98% |  1.02ms  | 31.85ms | 136.02ms | 77.17ms | 122.74ms | 208.21ms | 10000 |  10000  |   0    |
|   gin     | 46409  95% | 35.04us  | 33.00ms | 138.23ms | 71.22ms | 93.95ms  | 215.47ms | 10000 |  10000  |   0    |
| nethttp   | 42989  88% | 51.17us  | 33.67ms | 173.39ms | 87.98ms | 102.75ms | 232.61ms | 10000 |  10000  |   0    |

| Framework |  TPS [↓1]   |   EER [↓2]   |   Min   |   Avg   |   Max    |  TP95   |  TP99   | Used  | Success | Failed | CPU Avg | CPU Max | MEM Avg | MEM Max |
|   ---     |     ---     |     ---      |   ---   |   ---   |   ---    |   ---   |   ---   |  ---  |   ---   |  ---   |   ---   |   ---   |   ---   |   ---   |
| fasthttp  | 457150 100% | 1534.21 100% | 9.04us  | 21.78ms | 46.02ms  | 23.72ms | 30.96ms | 4.37s | 2000000 |   0    | 297.97  | 298.97  | 193.60M | 194.32M |
|   fib     | 364725  79% | 1319.49  86% | 20.88us | 27.30ms | 363.22ms | 45.21ms | 54.31ms | 5.48s | 2000000 |   0    | 276.41  | 277.98  | 89.22M  | 94.27M  |
| nethttp   | 298872  65% | 1039.90  67% | 7.50us  | 33.32ms | 71.90ms  | 43.67ms | 48.26ms | 6.69s | 2000000 |   0    | 287.40  | 295.97  | 315.14M | 315.88M |
|   gin     | 294209  64% | 1005.20  65% | 7.92us  | 33.86ms | 66.20ms  | 43.68ms | 47.18ms | 6.80s | 2000000 |   0    | 292.69  | 296.97  | 318.62M | 318.99M |

| Framework |   TPS [↓1]   |   EER [↓2]   | Req Sent | Bytes Sent | Resp Recv | Bytes Recv | CPU Avg | CPU Max | MEM Avg | MEM Max |
|   ---     |     ---      |     ---      |   ---    |    ---     |    ---    |    ---     |   ---   |   ---   |   ---   |   ---   |
| fasthttp  | 1990000 100% | 6826.59 100% | 19900000 |   18.98G   | 19900000  |   18.98G   | 291.51  | 298.97  | 195.42M | 196.68M |
|   fib     | 1143488  57% | 4165.12  61% | 11626730 |   11.09G   | 11434880  |   10.91G   | 274.54  | 285.96  | 630.26M | 934.54M |
| nethttp   |  470849  23% | 1612.81  23% | 5031820  |   4.80G    |  4708491  |   4.49G    | 291.94  | 298.97  | 338.23M | 353.10M |
|   gin     |  467244  23% | 1600.57  23% | 4991820  |   4.76G    |  4672446  |   4.46G    | 291.92  | 298.97  | 343.14M | 358.48M |
