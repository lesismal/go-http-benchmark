package main

import (
	"go-http-benchmark/config"
	"go-http-benchmark/frameworks"

	"github.com/gorilla/mux"
)

func main() {
	frameworks.Init(config.GorillaMux)
	control := frameworks.StartControlServer()
	defer control.Close()

	// A route with no .Methods() matches every method.
	router := mux.NewRouter()
	router.HandleFunc(config.EchoPath, frameworks.Echo)

	frameworks.ServeNetHTTP(router)
}
