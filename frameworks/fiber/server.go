package main

import (
	"go-http-benchmark/config"
	"go-http-benchmark/frameworks"
	"go-http-benchmark/logging"

	"github.com/gofiber/fiber/v3"
)

func main() {
	frameworks.Init(config.Fiber)
	control := frameworks.StartControlServer()
	defer control.Close()

	app := fiber.New(fiber.Config{
		// The same two settings as the fasthttp server, which fiber is built
		// on and passes them to: a Concurrency default of 256K would refuse
		// the rest of a million-connection run, and the read buffer has room
		// for the whole request in one read.
		Concurrency:    4 * 1024 * 1024,
		ReadBufferSize: max(4096, *frameworks.Payload+1024),
	})
	app.All(config.EchoPath, onEcho)

	// Handler builds the route tree, which app.Listener would otherwise
	// rebuild, print a banner and run the listen hooks for once per port.
	// After it, the fasthttp.Server fiber configured serves every port, as
	// the fasthttp server's one Server does.
	app.Handler()
	server := app.Server()
	for _, ln := range frameworks.ListenAll() {
		go func() {
			if err := server.Serve(ln); err != nil {
				logging.Printf("server exit: %v", err)
			}
		}()
	}

	frameworks.WaitSignal()
	app.Shutdown()
}

func onEcho(c fiber.Ctx) error {
	c.Set(fiber.HeaderContentType, "application/octet-stream")
	// Send does not copy: the response points at the request's body, which
	// fasthttp keeps until the response has been written.
	return c.Send(c.Body())
}
