package protocol

import (
	"bufio"
	"bytes"
	"io"
	"net/http"
	"strings"
	"testing"
)

// TestEncodeRequestParses holds EncodeRequest to what net/http's own server
// parses, since that is what every framework here parses it with or against.
func TestEncodeRequestParses(t *testing.T) {
	body := []byte("hello, benchmark")
	raw := EncodeRequest("POST", "127.0.0.1", "/echo", body)
	req, err := http.ReadRequest(bufio.NewReader(bytes.NewReader(raw)))
	if err != nil {
		t.Fatalf("ReadRequest: %v\n%s", err, raw)
	}
	got, _ := io.ReadAll(req.Body)
	if req.Method != "POST" || req.URL.Path != "/echo" || req.Host != "127.0.0.1" ||
		req.ContentLength != int64(len(body)) || !bytes.Equal(got, body) {
		t.Errorf("parsed %v %v host=%v len=%v body=%q", req.Method, req.URL.Path, req.Host, req.ContentLength, got)
	}

	get := EncodeRequest("GET", "[::1]", "/echo", nil)
	if strings.Contains(string(get), "Content-Length") {
		t.Errorf("GET without a body carries a Content-Length:\n%s", get)
	}
	if _, err := http.ReadRequest(bufio.NewReader(bytes.NewReader(get))); err != nil {
		t.Errorf("ReadRequest(GET): %v", err)
	}
}

func TestReadResponse(t *testing.T) {
	stream := "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\ncontent-length: 5\r\n\r\nhello" +
		"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3;ext=1\r\nabc\r\n2\r\nde\r\n0\r\nX-Trailer: 1\r\n\r\n" +
		"HTTP/1.1 204 No Content\r\n\r\n" +
		"HTTP/1.0 404 Not Found\r\nContent-Length: 3\r\n\r\nno!" +
		"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nskip"
	r := bufio.NewReader(strings.NewReader(stream))
	buf := make([]byte, 0, 2)
	for _, want := range []struct {
		status int
		body   string
	}{{200, "hello"}, {200, "abcde"}, {204, ""}, {404, "no!"}} {
		status, body, err := ReadResponse(r, buf)
		if err != nil || status != want.status || string(body) != want.body {
			t.Fatalf("ReadResponse = %v %q %v, want %v %q", status, body, err, want.status, want.body)
		}
		buf = body
	}
	if status, n, err := DiscardResponse(r); err != nil || status != 200 || n != 4 {
		t.Fatalf("DiscardResponse = %v %v %v", status, n, err)
	}
	if _, _, err := ReadResponse(r, buf); err != io.EOF {
		t.Errorf("ReadResponse at the end = %v, want EOF", err)
	}
}

func TestReadResponseRejects(t *testing.T) {
	for _, raw := range []string{
		"HTTP/2 200 OK\r\n\r\n",
		"HTTP/1.1 2x0 OK\r\n\r\n",
		"HTTP/1.1 200 OK\r\nno colon here\r\n\r\n",
		"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n",
		"HTTP/1.1 200 OK\r\n\r\nread until close",
		"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
	} {
		if _, _, err := ReadResponse(bufio.NewReader(strings.NewReader(raw)), nil); err == nil {
			t.Errorf("ReadResponse(%q) = nil error", raw)
		}
	}
}

func TestBatchBuffers(t *testing.T) {
	buf := make([]byte, 1100)
	batch, n, tick := BatchBuffers(buf, 200, 16*1024)
	if n != 10 || tick != 20 || len(batch) != 10*len(buf) {
		t.Errorf("BatchBuffers = %d bytes, %d per batch, %d a second", len(batch), n, tick)
	}
	if _, n, tick := BatchBuffers(make([]byte, 32*1024), 7, 16*1024); n != 1 || tick != 7 {
		t.Errorf("a request bigger than the batch: %d per batch, %d a second", n, tick)
	}
}
