package main

import (
	"net/http"

	"go-http-benchmark/config"
	"go-http-benchmark/frameworks"

	"github.com/zenazn/goji/web"
)

func main() {
	frameworks.Init(config.Goji)
	control := frameworks.StartControlServer()
	defer control.Close()

	// web.New rather than goji.DefaultMux: DefaultMux comes with goji's
	// request ID, logger and recoverer middleware, and the numbers are meant
	// to be goji's router on top of net/http rather than a line of log per
	// request. Handle routes every method.
	mux := web.New()
	mux.Handle(config.EchoPath, onEcho)

	frameworks.ServeNetHTTP(mux)
}

// onEcho is goji's own handler type, the one that gets the request's web.C.
func onEcho(c web.C, w http.ResponseWriter, r *http.Request) {
	frameworks.Echo(w, r)
}
