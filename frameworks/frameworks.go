// Package frameworks is what every benchmark server shares: the flags
// script/benchmark.sh forwards to them, the listeners on the framework's
// benchmark ports, and the control server that the clients read the server's
// pid, CPU and memory from.
package frameworks

import (
	"errors"
	"flag"
	"net"
	"net/http"
	"os"
	"os/signal"
	"runtime/debug"
	"syscall"

	"go-http-benchmark/config"
	"go-http-benchmark/logging"

	"github.com/libp2p/go-reuseport"
)

// The flags script/benchmark.sh forwards to every server. Each server defines
// all of them, whether or not it has a use for each, since one it did not
// define would stop it with "flag provided but not defined".
var (
	Nodelay   = flag.Bool("nodelay", true, `tcp nodelay`)
	reuse     = flag.Bool("reuseport", true, `reuse port`)
	Payload   = flag.Int("b", 1024, `expected request body size, which sizes the servers' read buffers`)
	memLimit  = flag.Int64("m", 1024*1024*1024*2, `memory limit`)
	framework string
)

// Init parses the flags and applies the ones every server shares. framework
// is the server's name as config knows it.
func Init(name string) {
	flag.Parse()
	framework = name
	debug.SetMemoryLimit(*memLimit)
	logging.Printf("%v server: nodelay=%v, reuseport=%v, payload=%v, memory limit=%v",
		name, *Nodelay, *reuse, *Payload, *memLimit)
}

// ServerAddrs is the addresses the framework's server listens on for the
// benchmark.
func ServerAddrs() []string {
	addrs, err := config.GetFrameworkServerAddrs(framework)
	if err != nil {
		logging.Fatalf("GetFrameworkServerAddrs(%v) failed: %v", framework, err)
	}
	return addrs
}

// Listen listens on addr, with SO_REUSEPORT unless -reuseport=false, and sets
// -nodelay on every connection it accepts. Go sets TCP_NODELAY on every TCP
// connection itself, so the wrapper only has anything to do for
// -nodelay=false; it is there for every framework that serves a net.Listener
// so that the flag means the same thing to all of them.
func Listen(network, addr string) (net.Listener, error) {
	var ln net.Listener
	var err error
	if *reuse {
		ln, err = reuseport.Listen(network, addr)
	} else {
		ln, err = net.Listen(network, addr)
	}
	if err != nil {
		return nil, err
	}
	if *Nodelay {
		return ln, nil
	}
	return &noDelayListener{Listener: ln}, nil
}

// ListenAll listens on every one of the framework's benchmark ports.
func ListenAll() []net.Listener {
	addrs := ServerAddrs()
	lns := make([]net.Listener, 0, len(addrs))
	for _, addr := range addrs {
		ln, err := Listen("tcp", addr)
		if err != nil {
			logging.Fatalf("listen %v failed: %v", addr, err)
		}
		lns = append(lns, ln)
	}
	logging.Printf("%v server: listening on %d ports, %v to %v",
		framework, len(addrs), addrs[0], addrs[len(addrs)-1])
	return lns
}

type noDelayListener struct {
	net.Listener
}

func (l *noDelayListener) Accept() (net.Conn, error) {
	c, err := l.Listener.Accept()
	if err != nil {
		return nil, err
	}
	SetNoDelay(c, false)
	return c, nil
}

func SetNoDelay(c net.Conn, nodelay bool) {
	cc, ok := c.(interface{ SetNoDelay(bool) error })
	if ok {
		cc.SetNoDelay(nodelay)
	}
}

// StartControlServer serves the control routes on the framework's control
// port, on a net/http server of its own; see
// config.GetFrameworkControlServerAddr.
func StartControlServer() *http.Server {
	addr, err := config.GetFrameworkControlServerAddr(framework)
	if err != nil {
		logging.Fatalf("GetFrameworkControlServerAddr(%v) failed: %v", framework, err)
	}
	mux := http.NewServeMux()
	HandleCommon(mux)
	server := &http.Server{Addr: addr, Handler: mux}
	go func() {
		if err := server.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			logging.Fatalf("control server on %v exit: %v", addr, err)
		}
	}()
	return server
}

// WaitSignal blocks until the server is told to stop: SIGINT, which
// script/killone.sh sends, or SIGTERM.
func WaitSignal() {
	interrupt := make(chan os.Signal, 1)
	signal.Notify(interrupt, os.Interrupt, syscall.SIGTERM)
	<-interrupt
	logging.Printf("%v server: exit", framework)
}
