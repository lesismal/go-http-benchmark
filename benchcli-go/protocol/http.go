// Package protocol is the HTTP/1.1 the benchmark client speaks: requests
// encoded once, up front, and written as they are, and a response reader that
// takes what the benchmark needs - the status and the body - off a
// bufio.Reader without building an http.Response for every one.
//
// The client is the other half of every number in the reports, and on a
// single-node run it shares the machine with the server, so what it spends on
// a request is what the server does not get. net/http's client would spend a
// goroutine, a Request, a Response and a header map on each.
package protocol

import (
	"bufio"
	"bytes"
	"errors"
	"fmt"
	"io"
	"strconv"
)

// EncodeRequest returns a whole HTTP/1.1 request: method and path, a Host
// header, and - for a body, even an empty one on a method that carries one -
// its Content-Type and Content-Length.
func EncodeRequest(method, host, path string, body []byte) []byte {
	buf := make([]byte, 0, 128+len(body))
	buf = append(buf, method...)
	buf = append(buf, ' ')
	buf = append(buf, path...)
	buf = append(buf, " HTTP/1.1\r\nHost: "...)
	buf = append(buf, host...)
	buf = append(buf, "\r\n"...)
	if body != nil || method == "POST" || method == "PUT" {
		buf = append(buf, "Content-Type: application/octet-stream\r\nContent-Length: "...)
		buf = strconv.AppendInt(buf, int64(len(body)), 10)
		buf = append(buf, "\r\n"...)
	}
	buf = append(buf, "\r\n"...)
	return append(buf, body...)
}

// BatchBuffers is buf repeated as many times as fit in maxLen, and no fewer
// than once - fewer if that many would not divide rate - with that count and
// how many times a second the batch has to be written to send rate copies of
// buf a second. The rate benchmark writes pipelined requests this way.
func BatchBuffers(buf []byte, rate, maxLen int) ([]byte, int, int) {
	batch := maxLen / len(buf)
	if batch < 1 {
		batch = 1
	}
	for (batch > 1) && (rate%batch != 0) {
		batch--
	}
	return bytes.Repeat(buf, batch), batch, rate / batch
}

// PipelineBuffers is buf repeated pipeline times, with how many times a second
// that batch has to be written to send rate copies of buf a second. pipeline
// has to divide rate; see ValidatePipeline.
func PipelineBuffers(buf []byte, rate, pipeline int) ([]byte, int, int) {
	return bytes.Repeat(buf, pipeline), pipeline, rate / pipeline
}

// ValidatePipeline checks a requested pipeline depth against the send rate:
// 0 asks for the depth BatchBuffers works out, and anything else has to divide
// rate, since the batch is written a whole number of times a second.
func ValidatePipeline(pipeline, rate int) error {
	switch {
	case pipeline < 0:
		return fmt.Errorf("pipeline %d: want 0, which fits as many requests as -rbs holds, or more", pipeline)
	case pipeline > 0 && rate%pipeline != 0:
		return fmt.Errorf("pipeline %d does not divide the send rate %d, so no whole number of"+
			" writes a second sends it; pick a divisor of %d", pipeline, rate, rate)
	}
	return nil
}

var (
	ErrMalformedStatus = errors.New("http: malformed status line")
	ErrMalformedHeader = errors.New("http: malformed header line")
	ErrMalformedChunk  = errors.New("http: malformed chunk")
	ErrNoLength        = errors.New("http: response has neither Content-Length nor chunked body")
	ErrBodyTooLarge    = errors.New("http: response body larger than the client accepts")
)

// MaxBodySize bounds a response body the reader will take, so that a broken
// Content-Length cannot make a client allocate without limit.
const MaxBodySize = 64 << 20

// ReadResponse reads one response off r. Its body is appended to dst[:0],
// which is returned grown if it had to be, so a caller that passes the same
// buffer back in reads every response without allocating.
func ReadResponse(r *bufio.Reader, dst []byte) (status int, body []byte, err error) {
	status, body, _, err = readResponse(r, dst[:0], true)
	return status, body, err
}

// DiscardResponse reads one response off r and drops its body, reporting how
// long it was.
func DiscardResponse(r *bufio.Reader) (status int, n int, err error) {
	status, _, n, err = readResponse(r, nil, false)
	return status, n, err
}

func readResponse(r *bufio.Reader, dst []byte, keep bool) (status int, body []byte, n int, err error) {
	line, err := readLine(r)
	if err != nil {
		return 0, dst, 0, err
	}
	status, err = parseStatus(line)
	if err != nil {
		return 0, dst, 0, err
	}

	contentLength := -1
	chunked := false
	for {
		line, err = readLine(r)
		if err != nil {
			return status, dst, 0, err
		}
		if len(line) == 0 {
			break
		}
		colon := bytes.IndexByte(line, ':')
		if colon <= 0 {
			return status, dst, 0, ErrMalformedHeader
		}
		name, value := line[:colon], bytes.TrimSpace(line[colon+1:])
		switch {
		case asciiEqualFold(name, "Content-Length"):
			length, parseErr := strconv.Atoi(string(value))
			if parseErr != nil || length < 0 {
				return status, dst, 0, fmt.Errorf("http: bad Content-Length %q", value)
			}
			contentLength = length
		case asciiEqualFold(name, "Transfer-Encoding"):
			chunked = bytes.Contains(bytes.ToLower(value), []byte("chunked"))
		}
	}

	switch {
	case status/100 == 1 || status == 204 || status == 304:
		return status, dst, 0, nil
	case chunked:
		return readChunked(r, status, dst, keep)
	case contentLength >= 0:
		if contentLength > MaxBodySize {
			return status, dst, 0, ErrBodyTooLarge
		}
		if !keep {
			_, err = r.Discard(contentLength)
			return status, dst, contentLength, err
		}
		dst = grow(dst, contentLength)
		_, err = io.ReadFull(r, dst[len(dst)-contentLength:])
		return status, dst, contentLength, err
	default:
		// A body delimited by the connection closing would end the
		// keep-alive connection every benchmark here relies on.
		return status, dst, 0, ErrNoLength
	}
}

func readChunked(r *bufio.Reader, status int, dst []byte, keep bool) (int, []byte, int, error) {
	total := 0
	for {
		line, err := readLine(r)
		if err != nil {
			return status, dst, total, err
		}
		if semi := bytes.IndexByte(line, ';'); semi >= 0 {
			line = line[:semi]
		}
		size, err := strconv.ParseInt(string(bytes.TrimSpace(line)), 16, 64)
		if err != nil || size < 0 {
			return status, dst, total, ErrMalformedChunk
		}
		if size == 0 {
			// The trailer, if any, up to the blank line that ends it.
			for {
				line, err = readLine(r)
				if err != nil {
					return status, dst, total, err
				}
				if len(line) == 0 {
					return status, dst, total, nil
				}
			}
		}
		if int64(total)+size > MaxBodySize {
			return status, dst, total, ErrBodyTooLarge
		}
		chunk := int(size)
		if keep {
			dst = grow(dst, chunk)
			if _, err = io.ReadFull(r, dst[len(dst)-chunk:]); err != nil {
				return status, dst, total, err
			}
		} else if _, err = r.Discard(chunk); err != nil {
			return status, dst, total, err
		}
		total += chunk
		if line, err = readLine(r); err != nil {
			return status, dst, total, err
		}
		if len(line) != 0 {
			return status, dst, total, ErrMalformedChunk
		}
	}
}

// readLine is one line without its CRLF, valid until the next read of r.
func readLine(r *bufio.Reader) ([]byte, error) {
	line, err := r.ReadSlice('\n')
	if err != nil {
		if err == bufio.ErrBufferFull {
			return nil, ErrMalformedHeader
		}
		return nil, err
	}
	line = line[:len(line)-1]
	if n := len(line); n > 0 && line[n-1] == '\r' {
		line = line[:n-1]
	}
	return line, nil
}

// parseStatus reads the code off "HTTP/1.1 200 OK".
func parseStatus(line []byte) (int, error) {
	if len(line) < 12 || !bytes.HasPrefix(line, []byte("HTTP/1.")) || line[8] != ' ' {
		return 0, ErrMalformedStatus
	}
	status := 0
	for _, c := range line[9:12] {
		if c < '0' || c > '9' {
			return 0, ErrMalformedStatus
		}
		status = status*10 + int(c-'0')
	}
	if len(line) > 12 && line[12] != ' ' {
		return 0, ErrMalformedStatus
	}
	return status, nil
}

func asciiEqualFold(b []byte, s string) bool {
	if len(b) != len(s) {
		return false
	}
	for i := 0; i < len(b); i++ {
		c, d := b[i], s[i]
		if 'A' <= c && c <= 'Z' {
			c += 'a' - 'A'
		}
		if 'A' <= d && d <= 'Z' {
			d += 'a' - 'A'
		}
		if c != d {
			return false
		}
	}
	return true
}

// grow extends b by n bytes, reallocating only when it has no room for them.
func grow(b []byte, n int) []byte {
	if cap(b)-len(b) < n {
		grown := make([]byte, len(b), len(b)+n)
		copy(grown, b)
		b = grown
	}
	return b[:len(b)+n]
}
