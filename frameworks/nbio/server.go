package main

import (
	"net/http"
	"time"

	"go-http-benchmark/config"
	"go-http-benchmark/frameworks"
	"go-http-benchmark/logging"

	nblog "github.com/lesismal/nbio/logging"
	"github.com/lesismal/nbio/nbhttp"
)

func main() {
	frameworks.Init(config.NBIO)
	control := frameworks.StartControlServer()
	defer control.Close()

	// Warnings and errors only: the engine logs a line for every listener it
	// starts, and there are fifty of them.
	nblog.SetLevel(nblog.LevelWarn)

	addrs := frameworks.ServerAddrs()

	// nethttp's mux and handler: nbhttp calls a standard http.Handler, so
	// the two differ by the server underneath alone.
	mux := http.NewServeMux()
	mux.HandleFunc(config.EchoPath, frameworks.Echo)

	// One engine bound to every benchmark port, all served by its one set of
	// pollers.
	engine := nbhttp.NewEngine(nbhttp.Config{
		Network: "tcp",
		// A copy: Start writes each listener's resolved address back into
		// the slice, which would turn ":11401" into "[::]:11401" in the log
		// line below.
		Addrs: append([]string(nil), addrs...),
		// Every connection on nbio's poller goroutines, read as its events
		// arrive, rather than a goroutine per connection. The default, said
		// here since it is the point.
		IOMod: nbhttp.IOModNonBlocking,
		// frameworks.Listen, for -reuseport and -nodelay: nbhttp accepts
		// with the listener's Accept before handing the connection to its
		// pollers, so the wrapper applies here as it does for net/http.
		Listen:  frameworks.Listen,
		Handler: mux,
		// Long enough never to close a connection mid-run, as none of the
		// other servers does: the default, 120s, would close the ones a
		// million-connection run leaves waiting between Connections and
		// BenchEcho.
		KeepaliveTime: 24 * time.Hour,
		// No client connections, so none of the client executor it would
		// otherwise start.
		SupportServerOnly: true,
	})
	if err := engine.Start(); err != nil {
		logging.Fatalf("start %d addresses failed: %v", len(addrs), err)
	}
	logging.Printf("%v server: listening on %d ports, %v to %v", config.NBIO, len(addrs), addrs[0], addrs[len(addrs)-1])

	frameworks.WaitSignal()
	engine.Stop()
}
