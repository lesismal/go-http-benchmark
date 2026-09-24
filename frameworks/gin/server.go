package main

import (
	"net/http"
	"strconv"

	"go-http-benchmark/config"
	"go-http-benchmark/frameworks"

	"github.com/gin-gonic/gin"
)

func main() {
	frameworks.Init(config.Gin)
	control := frameworks.StartControlServer()
	defer control.Close()

	// gin.New rather than gin.Default: no logger and no recovery middleware,
	// so the numbers are gin's router and context on top of net/http rather
	// than a line of log per request.
	gin.SetMode(gin.ReleaseMode)
	router := gin.New()
	router.Any(config.EchoPath, onEcho)

	frameworks.ServeNetHTTP(router)
}

func onEcho(c *gin.Context) {
	body, bufp, err := frameworks.ReadBody(c.Request.Body, c.Request.ContentLength)
	defer frameworks.BodyPool.Put(bufp)
	if err != nil {
		c.String(http.StatusBadRequest, err.Error())
		return
	}
	// gin writes through net/http, which sends a body longer than its 2KB
	// chunking buffer chunked unless it is told the length.
	c.Header("Content-Length", strconv.Itoa(len(body)))
	c.Data(http.StatusOK, "application/octet-stream", body)
}
