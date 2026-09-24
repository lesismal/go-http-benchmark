package main

import (
	"net/http"

	"go-http-benchmark/config"
	"go-http-benchmark/frameworks"
)

func main() {
	frameworks.Init(config.NetHTTP)
	control := frameworks.StartControlServer()
	defer control.Close()

	mux := http.NewServeMux()
	mux.HandleFunc(config.EchoPath, frameworks.Echo)

	frameworks.ServeNetHTTP(mux)
}
