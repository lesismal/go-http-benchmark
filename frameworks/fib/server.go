package main

import (
	stdhttp "net/http"
	"syscall"

	"go-http-benchmark/config"
	"go-http-benchmark/frameworks"
	"go-http-benchmark/logging"

	fib "github.com/lesismal/fib"
	fibhttp "github.com/lesismal/fib/http"
)

func main() {
	frameworks.Init(config.Fib)
	control := frameworks.StartControlServer()
	defer control.Close()

	addrs := frameworks.ServerAddrs()

	httpConfig := fibhttp.DefaultConfig()
	// HTTP/1 only, as the other three serve it here: none of them speaks
	// cleartext HTTP/2, and the clients never ask for it.
	httpConfig.DisableHTTP2 = true
	// Recycle each request's objects once its response is finished, as
	// fasthttp recycles its RequestCtx. onRequest keeps nothing of a request
	// past its response, which is what these ask of a handler.
	httpConfig.ReuseRequests = true
	httpConfig.ReuseHeaders = true
	httpConfig.ReuseURLs = true
	httpConfig.ReuseContexts = true
	handler := &serverHandler{
		ServerHandler: fibhttp.NewHandlerWithConfig(httpConfig, fibhttp.HandlerFunc(onRequest)),
		nodelay:       *frameworks.Nodelay,
	}

	// One engine bound to every benchmark port. A server per port would give
	// each one its own event loop but also its own descriptor table, buffer
	// pools and outbound budget, none of which the ports have any reason not
	// to share.
	serverConfig := fib.DefaultConfig()
	serverConfig.Network = "tcp4"
	serverConfig.Addrs = addrs
	engine, err := fib.Bind(serverConfig, handler)
	if err != nil {
		logging.Fatalf("bind %d addresses failed: %v", len(addrs), err)
	}
	logging.Printf("%v server: listening on %d ports, %v to %v", config.Fib, len(addrs), addrs[0], addrs[len(addrs)-1])
	go func() {
		if err := engine.Run(); err != nil {
			logging.Printf("server exit: %v", err)
		}
	}()

	frameworks.WaitSignal()
	engine.Stop()
}

func onRequest(c *fibhttp.Context, r *stdhttp.Request) {
	if r.URL.Path != config.EchoPath {
		_ = c.Respond(stdhttp.StatusNotFound, "text/plain; charset=utf-8", []byte("404 page not found\n"))
		return
	}
	// The body has already been read off the connection whole, so this never
	// waits on the network; and Respond copies it into the response it
	// sends, so the buffer can go back to the pool as soon as it returns.
	body, bufp, err := frameworks.ReadBody(r.Body, r.ContentLength)
	defer frameworks.BodyPool.Put(bufp)
	if err != nil {
		_ = c.Respond(stdhttp.StatusBadRequest, "text/plain; charset=utf-8", []byte(err.Error()))
		return
	}
	_ = c.Respond(stdhttp.StatusOK, "application/octet-stream", body)
}

// serverHandler sets TCP_NODELAY on each connection before fib's HTTP handler
// sees it: fib hands the descriptor to its poller as it was accepted, and Go's
// net package, which sets the option for the other three, is not involved.
type serverHandler struct {
	*fibhttp.ServerHandler
	nodelay bool
}

func (h *serverHandler) OnOpen(c *fib.Connection) {
	if h.nodelay {
		if err := syscall.SetsockoptInt(c.FD(), syscall.IPPROTO_TCP, syscall.TCP_NODELAY, 1); err != nil {
			c.Close()
			return
		}
	}
	h.ServerHandler.OnOpen(c)
}
