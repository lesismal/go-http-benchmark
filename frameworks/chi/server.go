package main

import (
	"go-http-benchmark/config"
	"go-http-benchmark/frameworks"

	"github.com/go-chi/chi/v5"
)

func main() {
	frameworks.Init(config.Chi)
	control := frameworks.StartControlServer()
	defer control.Close()

	// chi.NewRouter with no middleware: the numbers are chi's router on top
	// of net/http. HandleFunc routes every method.
	router := chi.NewRouter()
	router.HandleFunc(config.EchoPath, frameworks.Echo)

	frameworks.ServeNetHTTP(router)
}
