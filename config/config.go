package config

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"strconv"
	"strings"
	"time"

	"go-http-benchmark/logging"

	"github.com/lesismal/perf"
)

type InitArgs struct {
	PsInterval time.Duration
}

// Every framework this benchmark knows, by the name that -f, the server binary
// and the report row all take.
//
// This list, Ports and FrameworkList below are kept in framework-name order,
// as are the framework lists in script/config.sh and
// script/1m_conns_benchmark.sh, so that a framework sits in the same place in
// all of them and a new one has one obvious place to go in each.
const (
	Axum       = "axum"
	Beego      = "beego"
	Chi        = "chi"
	Echo       = "echo"
	Fasthttp   = "fasthttp"
	Fib        = "fib"
	Fiber      = "fiber"
	Gin        = "gin"
	Goji       = "goji"
	GorillaMux = "gorillamux"
	Hertz      = "hertz"
	HTTPRouter = "httprouter"
	NBIO       = "nbio"
	NetHTTP    = "nethttp"
	Workflow   = "workflow"
)

// Ports is the range of benchmark ports each framework's server listens on.
// Fifty of them, so that a client dialing a million connections from one
// address does not run out of ephemeral ports towards any one of them. A new
// framework takes the next thousand after the last range handed out, so that
// no range moves when one is added.
//
// axum (Rust) and workflow (C++) cannot import this map, so each carries its
// range as FIRST_PORT and LAST_PORT in its own source,
// frameworks/axum/src/main.rs and frameworks/workflow/main.cc, which
// TestAxumPortsMatch and TestWorkflowPortsMatch hold to the one here.
var Ports = map[string]string{
	Axum:       "14001:14050",
	Beego:      "15001:15050",
	Chi:        "16001:16050",
	Echo:       "17001:17050",
	Fasthttp:   "10001:10050",
	Fib:        "11001:11050",
	Fiber:      "18001:18050",
	Gin:        "12001:12050",
	Goji:       "19001:19050",
	GorillaMux: "20001:20050",
	Hertz:      "22001:22050",
	HTTPRouter: "21001:21050",
	NBIO:       "24001:24050",
	NetHTTP:    "13001:13050",
	Workflow:   "23001:23050",
}

// FrameworkList is every framework, in framework-name order. It is also the
// row order of a -sort=framework report, which is what puts a framework on the
// same row in every table and across runs, whatever it scored.
var FrameworkList = []string{
	Axum,
	Beego,
	Chi,
	Echo,
	Fasthttp,
	Fib,
	Fiber,
	Gin,
	Goji,
	GorillaMux,
	Hertz,
	HTTPRouter,
	NBIO,
	NetHTTP,
	Workflow,
}

// Langs is the programming language each framework's server is written in,
// as the reports' Lang column shows it: lower case, e.g. "go", "rust" or
// "c++".
var Langs = map[string]string{
	Axum:       "rust",
	Beego:      "go",
	Chi:        "go",
	Echo:       "go",
	Fasthttp:   "go",
	Fib:        "go",
	Fiber:      "go",
	Gin:        "go",
	Goji:       "go",
	GorillaMux: "go",
	Hertz:      "go",
	HTTPRouter: "go",
	NBIO:       "go",
	NetHTTP:    "go",
	Workflow:   "c++",
}

// NoPipeline lists the frameworks whose server does not support HTTP/1.1
// pipelining, so that BenchPipeline would measure connections being closed
// rather than requests being answered. workflow's closes a connection that
// sends a request before the one in front of it is answered
// (Communicator::create_request fails with EBADMSG). The clients skip
// BenchPipeline for these and write a report marked Skipped instead, which
// the BenchPipeline table shows as a row of "-".
//
// benchcli-rust carries the same list as NO_PIPELINE in its config.rs, which
// TestRustClientNoPipelineMatches holds to this one.
var NoPipeline = []string{
	Workflow,
}

// SupportsPipeline reports whether framework's server answers pipelined
// requests, which BenchPipeline needs.
func SupportsPipeline(framework string) bool {
	for _, v := range NoPipeline {
		if v == framework {
			return false
		}
	}
	return true
}

// FrameworkLang is framework's language, or "-" for a framework Langs does
// not know, such as one a report file names that this build has dropped.
func FrameworkLang(framework string) string {
	if lang, ok := Langs[framework]; ok {
		return lang
	}
	return "-"
}

// HasPprof reports whether framework's server serves /debug/pprof/: only the
// Go ones do, on the net/http control server they share. The clients do not
// ask any other server for a profile.
func HasPprof(framework string) bool {
	return FrameworkLang(framework) == "go"
}

// EchoPath is the route every server answers the benchmark on: the response
// body is the request body, byte for byte, with a Content-Length.
const EchoPath = "/echo"

func GetFrameworkBenchmarkPorts(framework string) ([]int, error) {
	portRange, ok := Ports[framework]
	if !ok {
		return nil, fmt.Errorf("unknown framework %q", framework)
	}
	bounds := strings.Split(portRange, ":")
	minPort, err := strconv.Atoi(bounds[0])
	if err != nil {
		return nil, err
	}
	maxPort, err := strconv.Atoi(bounds[1])
	if err != nil {
		return nil, err
	}
	ports := []int{}
	for i := minPort; i <= maxPort; i++ {
		ports = append(ports, i)
	}
	return ports, nil
}

// GetFrameworkServerAddrs is the addresses a server listens on for the
// benchmark: every interface, one address per port.
func GetFrameworkServerAddrs(framework string) ([]string, error) {
	ports, err := GetFrameworkBenchmarkPorts(framework)
	if err != nil {
		return nil, err
	}
	addrs := make([]string, 0, len(ports))
	for _, port := range ports {
		addrs = append(addrs, fmt.Sprintf(":%d", port))
	}
	return addrs, nil
}

// GetFrameworkControlServerAddr is the address a server's control routes -
// /init, /ps and the pprof ones - listen on: the port after its last
// benchmark port. Every framework serves them there on a net/http server of
// its own, so that the routes a client reads its resource columns from are
// the same code for all of them and never queue behind benchmark requests,
// and so that the framework being measured serves nothing but /echo.
func GetFrameworkControlServerAddr(framework string) (string, error) {
	port, err := frameworkControlPort(framework)
	if err != nil {
		return "", err
	}
	return fmt.Sprintf(":%d", port), nil
}

// urlHost brackets a bare IPv6 literal so that it can carry a port in a URL.
// BENCH_SERVER_HOST may be an address or a hostname, and an IPv6 address
// without this comes out as http://fe80::1:13001/echo, which parses as neither
// host nor port.
func urlHost(ip string) string {
	if strings.Contains(ip, ":") && !strings.HasPrefix(ip, "[") {
		return "[" + ip + "]"
	}
	return ip
}

// GetFrameworkBenchmarkAddrs is the host:port a client dials for each of the
// framework's benchmark ports.
func GetFrameworkBenchmarkAddrs(framework, ip string) ([]string, error) {
	ports, err := GetFrameworkBenchmarkPorts(framework)
	if err != nil {
		return nil, err
	}
	host := strings.Trim(ip, "[]")
	addrs := make([]string, 0, len(ports))
	for _, port := range ports {
		addrs = append(addrs, net.JoinHostPort(host, strconv.Itoa(port)))
	}
	return addrs, nil
}

// Control requests - /init and /ps - go to a port of their own, but they
// still arrive at a process that may be working through the backlog of a
// just-finished rate test with a hundred thousand connections, which can make
// it slow to accept or to answer. One attempt is not enough for that, and the
// resource columns that silently read 0 when it failed took CPU EER down
// with them. So retry, patiently, and say what failed when it still does.
const (
	controlAttempts = 4
	controlTimeout  = 30 * time.Second
	controlBackoff  = 2 * time.Second
)

// One client for every control request, so a retry can reuse a connection the
// server has already accepted.
var controlClient = &http.Client{Timeout: controlTimeout}

// controlRequest sends one control request, retrying a transport failure up to
// attempts times. A reply the server actually produced is returned as it is,
// including a 404: the route is not there and waiting will not put it there.
func controlRequest(url string, body []byte, attempts int) ([]byte, error) {
	var lastErr error
	for attempt := 1; attempt <= attempts; attempt++ {
		if attempt > 1 {
			time.Sleep(time.Duration(attempt-1) * controlBackoff)
		}
		data, answered, err := controlOnce(url, body)
		if err == nil {
			return data, nil
		}
		lastErr = fmt.Errorf("%v: %w", url, err)
		if answered {
			break
		}
		if attempt < attempts {
			logging.Printf("control request failed, retrying (%d/%d): %v", attempt, attempts, lastErr)
		}
	}
	return nil, lastErr
}

// controlOnce reports whether the server answered at all, so that the caller
// can tell a route that is missing from a server that is too busy to reply.
func controlOnce(url string, body []byte) (data []byte, answered bool, err error) {
	var res *http.Response
	if body == nil {
		res, err = controlClient.Get(url)
	} else {
		res, err = controlClient.Post(url, "", bytes.NewReader(body))
	}
	if err != nil {
		return nil, false, err
	}
	defer res.Body.Close()
	data, err = io.ReadAll(res.Body)
	if err != nil {
		return nil, false, err
	}
	if res.StatusCode != http.StatusOK {
		return nil, true, fmt.Errorf("%v: %s", res.Status, bytes.TrimSpace(data))
	}
	return data, true, nil
}

// frameworkControlPort is the port a framework's control routes listen on:
// the one after its last benchmark port.
func frameworkControlPort(framework string) (int, error) {
	ports, err := GetFrameworkBenchmarkPorts(framework)
	if err != nil {
		return 0, err
	}
	return ports[len(ports)-1] + 1, nil
}

// FrameworkControlAddr is the base URL of those routes, as a client reaches
// them.
func FrameworkControlAddr(framework, ip string) (string, error) {
	port, err := frameworkControlPort(framework)
	if err != nil {
		return "", err
	}
	return fmt.Sprintf("http://%v:%v", urlHost(ip), port), nil
}

func InitAndGetFrameworkPid(framework, ip string, args *InitArgs) (int, string, error) {
	pprofAddr, err := FrameworkControlAddr(framework, ip)
	if err != nil {
		return -1, "", err
	}
	serverAddr := pprofAddr + "/init"

	data, _ := json.Marshal(args)
	// A failed /init is not just a missing pid: it is a server that never
	// started sampling, so every CPU and MEM column of the run would be 0.
	body, err := controlRequest(serverAddr, data, controlAttempts)
	if err != nil {
		return -1, "", err
	}
	pid, err := strconv.Atoi(strings.TrimSpace(string(body)))

	return pid, pprofAddr, err
}

// GetFrameworkPsInfo reads the server's CPU and memory samples. It returns
// the counter it managed to read alongside an error as well as instead of
// one, so that samples which did arrive are still reported: an error here
// means the resource columns are incomplete, not that they are all missing.
func GetFrameworkPsInfo(framework, ip string) (*perf.PSCounter, error) {
	controlAddr, err := FrameworkControlAddr(framework, ip)
	if err != nil {
		return nil, err
	}
	serverAddr := controlAddr + "/ps"

	body, err := controlRequest(serverAddr, nil, controlAttempts)
	if err != nil {
		return nil, err
	}

	psCounter := &perf.PSCounter{}
	err = json.Unmarshal(body, psCounter)
	if err != nil {
		return nil, fmt.Errorf("%v: %w", serverAddr, err)
	}
	if psCounter.CPUAvg() <= 0 {
		// The request went through, so the sampler is what did not: either
		// /init never reached this server, or nothing has been sampled yet
		// because the phase was shorter than one -pi interval. Say so rather
		// than letting the columns quietly read 0.
		return psCounter, fmt.Errorf("%v: answered with no CPU samples, so either /init did not"+
			" reach it or the phase was shorter than the -pi sampling interval", serverAddr)
	}

	return psCounter, nil
}
