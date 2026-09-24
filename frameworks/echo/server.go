package main

import (
	"net/http"
	"strconv"

	"go-http-benchmark/config"
	"go-http-benchmark/frameworks"

	"github.com/labstack/echo/v5"
)

func main() {
	frameworks.Init(config.Echo)
	control := frameworks.StartControlServer()
	defer control.Close()

	// echo.New comes with no middleware, so the numbers are echo's router and
	// context on top of net/http.
	e := echo.New()
	e.Any(config.EchoPath, onEcho)

	frameworks.ServeNetHTTP(e)
}

func onEcho(c *echo.Context) error {
	r := c.Request()
	body, bufp, err := frameworks.ReadBody(r.Body, r.ContentLength)
	defer frameworks.BodyPool.Put(bufp)
	if err != nil {
		return c.String(http.StatusBadRequest, err.Error())
	}
	// echo writes through net/http, which sends a body longer than its 2KB
	// chunking buffer chunked unless it is told the length.
	c.Response().Header().Set("Content-Length", strconv.Itoa(len(body)))
	return c.Blob(http.StatusOK, "application/octet-stream", body)
}
