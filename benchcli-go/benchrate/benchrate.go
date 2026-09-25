package benchrate

import (
	"bytes"
	"context"
	"crypto/rand"
	"math"
	"sync"
	"sync/atomic"
	"time"

	"go-http-benchmark/benchcli-go/connections"
	"go-http-benchmark/benchcli-go/protocol"
	"go-http-benchmark/benchcli-go/report"
	"go-http-benchmark/config"
	"go-http-benchmark/logging"

	"github.com/lesismal/perf"
	"golang.org/x/time/rate"
)

// BenchRate is HTTP/1.1 pipelining at a rate the client sets: every
// connection is sent SendRate requests a second, written a batch at a time
// without waiting for the responses to the ones before, and a goroutine per
// connection reads the responses back as they come. A connection with more
// than a few batches unanswered is skipped until the server catches up, so a
// server slower than the rate is measured by what it answered rather than by
// how deep a queue the client could build in front of it.
type BenchRate struct {
	Framework   string
	Ip          string
	Duration    time.Duration
	Concurrency int
	SendRate    int
	BatchSize   int
	// Pipeline is how many requests go into one write, or 0 for as many as
	// fit in BatchSize bytes and divide SendRate.
	Pipeline   int
	Payload    int
	SendLimit  int
	PsInterval time.Duration

	ServerPid int
	PsCounter *perf.PSCounter
	// Where the CPU and MEM columns come from; see config.SetupPS.
	PsSource config.PSSource

	Conns []*connections.Conn

	wbuffer []byte

	limitFn    func()
	checkValid bool

	batch       int
	batchBuffer []byte
	tickRate    int

	sendTimes int64
	sendBytes int64
	recvTimes int64
	recvBytes int64
	// answered counts every response, whatever it said, where recvTimes
	// counts only the 200s with the body that was sent.
	answered int64

	onBenchmark  func()
	pprofDataCPU []byte
	pprofDataMEM []byte
}

type conn struct {
	*connections.Conn
	sendCnt int64
	recvCnt int64
}

// maxBatchesInFlight is how many batches a connection may have unanswered
// before it is skipped for a tick.
const maxBatchesInFlight = 4

func New(framework string, serverPid int, ip string, conns []*connections.Conn, checkValid bool) *BenchRate {
	return &BenchRate{
		Framework:  framework,
		Ip:         ip,
		Conns:      conns,
		limitFn:    func() {},
		checkValid: checkValid,
		ServerPid:  serverPid,
	}
}

func (br *BenchRate) Run() {
	br.init()
	defer br.clean()

	conns := make([]*conn, 0, len(br.Conns))
	for _, c := range br.Conns {
		if c.Broken {
			if err := c.Redial(); err != nil {
				logging.Printf("%v: redial %v failed, leaving the connection out: %v", report.BenchPipelineName, c.Addr, err)
				continue
			}
		}
		conns = append(conns, &conn{Conn: c})
	}

	connTeams := make([][]*conn, br.Concurrency)
	for i, c := range conns {
		connTeams[i%len(connTeams)] = append(connTeams[i%len(connTeams)], c)
	}

	logging.Printf("%v for %.2f seconds, %d requests pipelined per write ...", report.BenchPipelineName, br.Duration.Seconds(), br.batch)

	readers := sync.WaitGroup{}
	for _, c := range conns {
		readers.Add(1)
		go func() {
			defer readers.Done()
			br.readResponses(c)
		}()
	}

	if br.onBenchmark != nil {
		br.onBenchmark()
	}

	done := make(chan struct{})
	time.AfterFunc(br.Duration, func() {
		close(done)
	})

	writers := sync.WaitGroup{}
	for _, team := range connTeams {
		writers.Add(1)
		go func() {
			defer writers.Done()
			ticker := time.NewTicker(time.Second / time.Duration(br.tickRate))
			defer ticker.Stop()
			for {
				select {
				case <-done:
					return
				case <-ticker.C:
					br.doOnce(team)
				}
			}
		}()
	}
	writers.Wait()

	// The last batch written is still on its way back when the duration is
	// up, and a batch can be many requests. Give the server one tick more to
	// answer what it was sent, which is how long it would have had before the
	// next batch, and no longer: a server slower than the rate is measured by
	// what it answered in time, not by how long it took to drain. Then
	// snapshot the counters and unblock the readers.
	grace := time.NewTimer(time.Second / time.Duration(br.tickRate))
	for atomic.LoadInt64(&br.answered) < atomic.LoadInt64(&br.sendTimes) {
		select {
		case <-grace.C:
			goto snapshot
		case <-time.After(time.Millisecond):
		}
	}
	grace.Stop()
snapshot:
	recvTimes, recvBytes := atomic.LoadInt64(&br.recvTimes), atomic.LoadInt64(&br.recvBytes)
	for _, c := range conns {
		c.SetReadDeadline(time.Now())
	}
	readers.Wait()
	br.recvTimes, br.recvBytes = recvTimes, recvBytes

	logging.Printf("%v for %.2f seconds done", report.BenchPipelineName, br.Duration.Seconds())
}

func (br *BenchRate) Stop() {

}

func (br *BenchRate) OnBenchmark(f func()) {
	br.onBenchmark = f
}

func (br *BenchRate) SetPprofData(cpu, mem []byte) {
	br.pprofDataCPU = cpu
	br.pprofDataMEM = mem
}

func (br *BenchRate) Report() *report.BenchRateReport {
	r := &report.BenchRateReport{
		BenchClient: "benchcli-go",
		Framework:   br.Framework,
		Lang:        config.FrameworkLang(br.Framework),
		Duration:    br.Duration.Nanoseconds(),
		Connections: len(br.Conns),
		Concurrency: br.Concurrency,
		SendRate:    br.SendRate,
		Pipeline:    br.batch,
		Payload:     br.Payload,
		SendTimes:   br.sendTimes,
		SendBytes:   br.sendBytes,
		RecvTimes:   br.recvTimes,
		RecvBytes:   br.recvBytes,
	}
	r.SetPprofData(br.pprofDataCPU, br.pprofDataMEM)
	var psErr error
	br.PsCounter, psErr = br.psInfo()
	if psErr != nil {
		logging.Printf("%v: resource statistics for %v incomplete, CPU EER and MEM EER will read 0: %v",
			report.BenchPipelineName, br.Framework, psErr)
	}
	if br.PsCounter != nil {
		r.CPUMin = br.PsCounter.CPUMin()
		r.CPUAvg = br.PsCounter.CPUAvg()
		r.CPUMax = br.PsCounter.CPUMax()
		r.MEMRSSMin = br.PsCounter.MEMRSSMin()
		r.MEMRSSAvg = br.PsCounter.MEMRSSAvg()
		r.MEMRSSMax = br.PsCounter.MEMRSSMax()
		tps := report.RateTPS(r.RecvTimes, r.Duration)
		r.CPUEER = report.CPUEER(tps, r.CPUAvg)
		r.MEMEER = report.MEMEER(tps, r.MEMRSSAvg)
	}
	r.TPS = int64(math.Floor(report.RateTPS(r.RecvTimes, r.Duration)))
	return r
}

// psInfo reads the server's resource samples from wherever this run takes
// them; see BenchEcho.psInfo.
func (br *BenchRate) psInfo() (*perf.PSCounter, error) {
	if br.PsSource != nil {
		return br.PsSource.PsInfo()
	}
	return config.GetFrameworkPsInfo(br.Framework, br.Ip)
}

func (br *BenchRate) init() {
	if br.Duration <= 0 {
		br.Duration = time.Second * 10
	}
	if br.Concurrency <= 0 {
		br.Concurrency = 50000
	}
	if br.Concurrency > len(br.Conns) {
		br.Concurrency = len(br.Conns)
	}
	if br.Concurrency <= 0 {
		logging.Fatalf("%v: no connections to run on", report.BenchPipelineName)
	}
	if br.SendRate <= 0 {
		br.SendRate = 1
	}
	if br.Payload <= 0 {
		br.Payload = 1024
	}

	br.wbuffer = make([]byte, br.Payload)
	rand.Read(br.wbuffer)
	request := protocol.EncodeRequest("POST", connections.Host(br.Ip), config.EchoPath, br.wbuffer)
	if err := protocol.ValidatePipeline(br.Pipeline, br.SendRate); err != nil {
		logging.Fatalf("%v: %v", report.BenchPipelineName, err)
	}
	if br.Pipeline > 0 {
		br.batchBuffer, br.batch, br.tickRate = protocol.PipelineBuffers(request, br.SendRate, br.Pipeline)
	} else {
		br.batchBuffer, br.batch, br.tickRate = protocol.BatchBuffers(request, br.SendRate, br.BatchSize)
	}
	if br.tickRate <= 0 || len(br.batchBuffer) == 0 {
		logging.Fatalf("%v got a wrong tickRate: %v, or batchBuffer: %v", report.BenchPipelineName, br.tickRate, len(br.batchBuffer))
	}

	if br.PsInterval <= 0 {
		br.PsInterval = time.Second
	}

	if br.SendLimit > 0 {
		limiter := rate.NewLimiter(rate.Every(1*time.Second), br.SendLimit)
		br.limitFn = func() {
			limiter.WaitN(context.Background(), br.batch)
		}
	}
}

func (br *BenchRate) clean() {
	br.limitFn = func() {}
}

func (br *BenchRate) doOnce(conns []*conn) {
	for _, c := range conns {
		if atomic.LoadInt64(&c.sendCnt)-atomic.LoadInt64(&c.recvCnt) >= int64(br.batch*maxBatchesInFlight) {
			continue
		}
		br.limitFn()
		if _, err := c.Write(br.batchBuffer); err == nil {
			atomic.AddInt64(&br.sendTimes, int64(br.batch))
			atomic.AddInt64(&br.sendBytes, int64(br.batch*br.Payload))
			atomic.AddInt64(&c.sendCnt, int64(br.batch))
		}
	}
}

// readResponses counts the responses on one connection until it fails, which
// is how the run ends it: Run sets a read deadline once the duration is up.
func (br *BenchRate) readResponses(c *conn) {
	var body []byte
	for {
		var status, n int
		var err error
		if br.checkValid {
			status, body, err = protocol.ReadResponse(c.Reader, body)
			n = len(body)
			if err == nil && !bytes.Equal(body, br.wbuffer) {
				status = 0
			}
		} else {
			status, n, err = protocol.DiscardResponse(c.Reader)
		}
		if err != nil {
			c.Broken = true
			return
		}
		// Unanswered is unanswered whatever the response said, so the
		// in-flight count goes down either way; only a 200 with the body
		// that was sent counts as a response in the report.
		atomic.AddInt64(&c.recvCnt, 1)
		atomic.AddInt64(&br.answered, 1)
		if status == 200 {
			atomic.AddInt64(&br.recvTimes, 1)
			atomic.AddInt64(&br.recvBytes, int64(n))
		}
	}
}
