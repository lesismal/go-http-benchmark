package main

import (
	"context"
	"net"
	"net/http"
	"syscall"
	"time"

	"go-http-benchmark/config"
	"go-http-benchmark/frameworks"
	"go-http-benchmark/logging"

	"github.com/cloudwego/hertz/pkg/app"
	"github.com/cloudwego/hertz/pkg/app/server"
	"github.com/cloudwego/hertz/pkg/common/hlog"
	hertznetpoll "github.com/cloudwego/hertz/pkg/network/netpoll"
)

func main() {
	frameworks.Init(config.Hertz)
	control := frameworks.StartControlServer()
	defer control.Close()

	// Warnings and errors only: every engine logs a line when it starts
	// listening, and there are fifty of them.
	hlog.SetLevel(hlog.LevelWarn)

	// One engine per port: an engine serves one listener. They share
	// netpoll's pollers, which are global to the process, so fifty engines
	// are still one set of event loops.
	var engines []*server.Hertz
	for _, ln := range frameworks.ListenAllRaw() {
		h := server.New(
			server.WithListener(ln),
			// netpoll rather than the standard library's net: hertz's
			// default on Linux and macOS, said here since it is the point.
			server.WithTransport(hertznetpoll.NewTransporter),
			// Long enough never to close a connection mid-run, as none of
			// the other servers does: the default, 3 minutes, would close
			// the ones a million-connection run leaves waiting between
			// Connections and BenchEcho. Not 0: hertz then returns the
			// connection to netpoll after each request, and closes it as it
			// does, so it would serve one request per connection and
			// answer only the first of a pipelined batch.
			server.WithIdleTimeout(idleTimeout),
			// Room for the whole request, header and body, in one read.
			server.WithReadBufferSize(max(4096, *frameworks.Payload+1024)),
			server.WithOnAccept(onAccept),
			server.WithDisablePrintRoute(true),
		)
		h.Any(config.EchoPath, onEcho)
		engines = append(engines, h)
		go func() {
			if err := h.Run(); err != nil {
				logging.Printf("server exit: %v", err)
			}
		}()
	}

	frameworks.WaitSignal()
	for _, h := range engines {
		h.Close()
	}
}

const idleTimeout = 24 * time.Hour

// onAccept applies -nodelay=false: netpoll sets TCP_NODELAY on every
// connection it accepts, and takes the listening descriptor over from the
// net.Listener, so the wrapper frameworks.Listen puts on it for the other
// servers never sees the connection.
func onAccept(conn net.Conn) context.Context {
	if !*frameworks.Nodelay {
		if c, ok := conn.(*hertznetpoll.Conn); ok {
			if fc, ok := c.Conn.(interface{ Fd() int }); ok {
				syscall.SetsockoptInt(fc.Fd(), syscall.IPPROTO_TCP, syscall.TCP_NODELAY, 0)
			}
		}
	}
	return context.Background()
}

func onEcho(_ context.Context, c *app.RequestContext) {
	// Data copies the body into the response, as the fasthttp server's
	// SetBody does.
	c.Data(http.StatusOK, "application/octet-stream", c.Request.Body())
}
