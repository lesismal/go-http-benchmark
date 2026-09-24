package main

import (
	"net/http"

	"go-http-benchmark/config"
	"go-http-benchmark/frameworks"

	"github.com/beego/beego/v2/server/web"
	beecontext "github.com/beego/beego/v2/server/web/context"
)

func main() {
	frameworks.Init(config.Beego)
	control := frameworks.StartControlServer()
	defer control.Close()

	// beego's router on beego's default config - prod mode, no sessions, no
	// gzip, and CopyRequestBody off, so that onEcho reads the body into a
	// pooled buffer as the other net/http based servers do rather than beego
	// reading it into one of its own first - served by net/http like the
	// others, not by web.Run and its admin and graceful-restart machinery.
	router := web.NewControllerRegister()
	router.Any(config.EchoPath, onEcho)

	frameworks.ServeNetHTTP(router)
}

func onEcho(ctx *beecontext.Context) {
	r := ctx.Request
	body, bufp, err := frameworks.ReadBody(r.Body, r.ContentLength)
	defer frameworks.BodyPool.Put(bufp)
	if err != nil {
		ctx.Output.SetStatus(http.StatusBadRequest)
		ctx.Output.Body([]byte(err.Error()))
		return
	}
	// Output.Body sets the Content-Length itself.
	ctx.Output.Header("Content-Type", "application/octet-stream")
	ctx.Output.Body(body)
}
