package connections

import (
	"bufio"
	"fmt"
	"net"
	"runtime"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"go-http-benchmark/benchcli-go/protocol"
	"go-http-benchmark/benchcli-go/report"
	"go-http-benchmark/config"
	"go-http-benchmark/logging"

	"github.com/lesismal/perf"
)

// Conn is one keep-alive connection to the server, and the reader its
// responses are parsed from. Only one goroutine uses a Conn's reader at a
// time: the echo benchmark hands a Conn to one worker at a time, and the rate
// benchmark gives each one a reader goroutine of its own.
type Conn struct {
	net.Conn
	Reader *bufio.Reader
	Addr   string

	// Broken is set once a request on the connection has failed. A failed
	// request leaves the stream at an unknown point, so the connection cannot
	// carry another; Redial replaces it.
	Broken bool

	cs *Connections
}

// Redial replaces a broken connection with a new one to the same address.
func (c *Conn) Redial() error {
	c.Conn.Close()
	conn, err := c.cs.dial(c.Addr, c.Reader)
	if err != nil {
		return err
	}
	c.Conn = conn
	c.Broken = false
	return nil
}

type Connections struct {
	Framework      string
	Ip             string
	Concurrency    int
	NumConnections int
	DialTimeout    time.Duration
	RetryInterval  time.Duration
	RetryTimes     int
	EnalbeTPN      bool
	Percents       []int
	// ReadBufferSize sizes each connection's reader, which the echo
	// benchmark wants big enough for a whole response.
	ReadBufferSize int

	// All connected connections
	conns []*Conn

	Calculator *perf.Calculator

	mux         sync.Mutex
	serverIdx   uint32
	serverAddrs []string
	request     []byte
}

func New(framework, ip string, numConns int) *Connections {
	return &Connections{
		Framework:      framework,
		Ip:             ip,
		NumConnections: numConns,
	}
}

// Run dials the connections, each one a TCP connection and one GET answered
// on it: an HTTP server has no handshake of its own, and a connection the
// kernel accepted is not yet one the server is serving. What this measures is
// the rate the server takes new clients on at, the way the WebSocket
// benchmark's handshake does.
func (cs *Connections) Run() {
	cs.init()
	defer cs.clean()

	logging.Printf("Dial Connections: [%v]", cs.NumConnections)
	logging.Printf("Dial Concurrency: [%v]", cs.Concurrency)
	done := make(chan struct{})
	logDone := make(chan struct{})

	go func() {
		defer func() {
			logging.Printf("Connections done: %v Success, %v Failed",
				atomic.LoadInt64(&cs.Calculator.Success), atomic.LoadInt64(&cs.Calculator.Failed))
			close(logDone)
		}()
		ticker := time.NewTicker(time.Second)
		defer ticker.Stop()
		for i := 1; true; i++ {
			select {
			case <-done:
				return
			case <-ticker.C:
				logging.Printf("%03d seconds passed, %v Connected ...", i, atomic.LoadInt64(&cs.Calculator.Success))
			}
		}
	}()

	logging.Printf("Connections start ...")
	cs.Calculator.Benchmark(cs.Concurrency, cs.NumConnections, cs.doOnce, cs.Percents)

	close(done)
	<-logDone
}

func (cs *Connections) Conns() []*Conn {
	return cs.conns
}

func (cs *Connections) Stop() {
	for _, c := range cs.conns {
		c.Close()
	}
}

func (cs *Connections) Report() report.Report {
	r := &report.ConnectionsReport{
		BenchClient: "benchcli-go",
		Framework:   cs.Framework,
		Lang:        config.FrameworkLang(cs.Framework),
		TPS:         cs.Calculator.TPS(),
		Min:         cs.Calculator.Min,
		Avg:         cs.Calculator.Avg,
		Max:         cs.Calculator.Max,

		Used:        int64(cs.Calculator.Used),
		Total:       cs.NumConnections,
		Success:     cs.Calculator.Success,
		Failed:      cs.Calculator.Failed,
		Concurrency: cs.Concurrency,
	}
	if cs.EnalbeTPN {
		r.TP50 = cs.Calculator.TPN(50)
		r.TP75 = cs.Calculator.TPN(75)
		r.TP90 = cs.Calculator.TPN(90)
		r.TP95 = cs.Calculator.TPN(95)
		r.TP99 = cs.Calculator.TPN(99)
	}
	return r
}

func (cs *Connections) init() {
	if cs.NumConnections <= 0 {
		cs.NumConnections = 1000
	}
	if cs.Concurrency <= 0 {
		cs.Concurrency = runtime.NumCPU() * 1000
	}
	if cs.Concurrency > cs.NumConnections {
		cs.Concurrency = cs.NumConnections
	}
	if cs.DialTimeout <= 0 {
		cs.DialTimeout = time.Second * 1
	}
	if cs.RetryInterval <= 0 {
		cs.RetryInterval = time.Second / 10
	}
	if cs.RetryTimes <= 0 {
		cs.RetryTimes = 3
	}
	if cs.ReadBufferSize <= 0 {
		cs.ReadBufferSize = 4096
	}
	if cs.EnalbeTPN {
		cs.Percents = []int{50, 75, 90, 95, 99}
	}

	cs.Calculator = perf.NewCalculator(fmt.Sprintf("%v-Connect", cs.Framework))

	addrs, err := config.GetFrameworkBenchmarkAddrs(cs.Framework, cs.Ip)
	if err != nil {
		logging.Fatalf("GetFrameworkBenchmarkAddrs(%v) failed: %v", cs.Framework, err)
	}
	cs.serverAddrs = addrs
	cs.request = protocol.EncodeRequest("GET", Host(cs.Ip), config.EchoPath, nil)
	cs.conns = make([]*Conn, 0, cs.NumConnections)
}

func (cs *Connections) clean() {
	cs.serverAddrs = nil
}

// Host is the Host header the clients send, for a server reached as ip: an
// IPv6 address in brackets, as it would be in a URL.
func Host(ip string) string {
	if strings.Contains(ip, ":") && !strings.HasPrefix(ip, "[") {
		return "[" + ip + "]"
	}
	return ip
}

// dial connects to addr and has one request answered on the connection,
// which is what makes it one the server is serving. r is reset to read the
// new connection, and is what reads its responses from then on.
func (cs *Connections) dial(addr string, r *bufio.Reader) (net.Conn, error) {
	conn, err := net.DialTimeout("tcp", addr, cs.DialTimeout)
	if err != nil {
		return nil, err
	}
	conn.SetDeadline(time.Now().Add(cs.DialTimeout))
	if _, err = conn.Write(cs.request); err == nil {
		var status int
		r.Reset(conn)
		status, _, err = protocol.DiscardResponse(r)
		if err == nil && status != 200 {
			err = fmt.Errorf("GET %v: status %d", config.EchoPath, status)
		}
	}
	if err != nil {
		conn.Close()
		return nil, err
	}
	conn.SetDeadline(time.Time{})
	return conn, nil
}

// doOnce dials one connection, retrying on failure.
func (cs *Connections) doOnce() error {
	var err error
	reader := bufio.NewReaderSize(nil, cs.ReadBufferSize)
	for i := 0; i < cs.RetryTimes; i++ {
		if i > 0 {
			time.Sleep(cs.RetryInterval)
		}
		addr := cs.serverAddrs[atomic.AddUint32(&cs.serverIdx, 1)%uint32(len(cs.serverAddrs))]
		var conn net.Conn
		conn, err = cs.dial(addr, reader)
		if err == nil {
			c := &Conn{Conn: conn, Reader: reader, Addr: addr, cs: cs}
			cs.mux.Lock()
			cs.conns = append(cs.conns, c)
			cs.mux.Unlock()
			return nil
		}
	}
	return err
}
