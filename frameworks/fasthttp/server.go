package main

import (
	"go-http-benchmark/config"
	"go-http-benchmark/frameworks"
	"go-http-benchmark/logging"

	"github.com/valyala/fasthttp"
)

func main() {
	frameworks.Init(config.Fasthttp)
	control := frameworks.StartControlServer()
	defer control.Close()

	server := &fasthttp.Server{
		Handler: onRequest,
		// Concurrency bounds the connections a server serves at once, and
		// its default of 256K would refuse the rest of a million-connection
		// run.
		Concurrency: 4 * 1024 * 1024,
		// Room for the whole request, header and body, in one read.
		ReadBufferSize: max(4096, *frameworks.Payload+1024),
	}

	// One fasthttp.Server serving every port: Serve takes one listener per
	// call, and the calls share the server's worker pool settings and its
	// connection count.
	for _, ln := range frameworks.ListenAll() {
		go func() {
			if err := server.Serve(ln); err != nil {
				logging.Printf("server exit: %v", err)
			}
		}()
	}

	frameworks.WaitSignal()
	server.Shutdown()
}

var echoPath = []byte(config.EchoPath)

func onRequest(ctx *fasthttp.RequestCtx) {
	if string(ctx.Path()) != string(echoPath) {
		ctx.NotFound()
		return
	}
	ctx.SetContentType("application/octet-stream")
	// SetBody copies, so the response does not hold on to the request's
	// buffer.
	ctx.SetBody(ctx.PostBody())
}
