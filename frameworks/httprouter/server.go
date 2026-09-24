package main

import (
	"net/http"

	"go-http-benchmark/config"
	"go-http-benchmark/frameworks"

	"github.com/julienschmidt/httprouter"
)

func main() {
	frameworks.Init(config.HTTPRouter)
	control := frameworks.StartControlServer()
	defer control.Close()

	// httprouter routes by method and has no catch-all one, so /echo is
	// registered for the two the clients send: GET for Connections, POST for
	// the rest.
	router := httprouter.New()
	router.Handle(http.MethodGet, config.EchoPath, onEcho)
	router.Handle(http.MethodPost, config.EchoPath, onEcho)

	frameworks.ServeNetHTTP(router)
}

// onEcho is httprouter's own handler type, the one that gets the route's
// Params.
func onEcho(w http.ResponseWriter, r *http.Request, _ httprouter.Params) {
	frameworks.Echo(w, r)
}
