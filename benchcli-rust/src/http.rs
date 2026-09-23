//! benchcli-go/protocol in Rust: requests encoded once and written as they
//! are, and a response reader that takes the status and the body off a
//! buffered connection without building a response object for each one.

use std::io::{Error, ErrorKind, Result};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// protocol.EncodeRequest.
pub fn encode_request(method: &str, host: &str, path: &str, body: Option<&[u8]>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(128 + body.map_or(0, <[u8]>::len));
    buf.extend_from_slice(format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\n").as_bytes());
    if body.is_some() || method == "POST" || method == "PUT" {
        let len = body.map_or(0, <[u8]>::len);
        buf.extend_from_slice(
            format!("Content-Type: application/octet-stream\r\nContent-Length: {len}\r\n")
                .as_bytes(),
        );
    }
    buf.extend_from_slice(b"\r\n");
    if let Some(body) = body {
        buf.extend_from_slice(body);
    }
    buf
}

/// protocol.BatchBuffers: as many copies of buf as fit in max_len, and at
/// least one - fewer if that many would not divide rate - with that count and
/// how many writes a second send rate copies a second.
pub fn batch_buffers(buf: &[u8], rate: usize, max_len: usize) -> (Vec<u8>, usize, usize) {
    let mut batch = (max_len / buf.len()).max(1);
    while batch > 1 && rate % batch != 0 {
        batch -= 1;
    }
    (buf.repeat(batch), batch, rate / batch)
}

/// protocol.PipelineBuffers.
pub fn pipeline_buffers(buf: &[u8], rate: usize, pipeline: usize) -> (Vec<u8>, usize, usize) {
    (buf.repeat(pipeline), pipeline, rate / pipeline)
}

/// protocol.ValidatePipeline.
pub fn validate_pipeline(pipeline: i64, rate: usize) -> std::result::Result<(), String> {
    if pipeline < 0 {
        return Err(format!(
            "pipeline {pipeline}: want 0, which fits as many requests as -rbs holds, or more"
        ));
    }
    if pipeline > 0 && rate % pipeline as usize != 0 {
        return Err(format!(
            "pipeline {pipeline} does not divide the send rate {rate}, so no whole number of writes a second sends it; pick a divisor of {rate}"
        ));
    }
    Ok(())
}

/// protocol.MaxBodySize.
const MAX_BODY_SIZE: usize = 64 << 20;

fn invalid(msg: &str) -> Error {
    Error::new(ErrorKind::InvalidData, msg.to_string())
}

/// The read side of a connection's buffer: what has been read off the socket
/// and not yet parsed, between start and end.
pub struct RBuf {
    buf: Vec<u8>,
    start: usize,
    end: usize,
}

/// How a response body with neither a Content-Length nor chunked framing is
/// taken: an error on a keep-alive benchmark connection, whose next response
/// it would swallow, and the rest of the stream on a control request, which
/// the server closes after answering.
#[derive(Clone, Copy, PartialEq)]
pub enum NoLength {
    Error,
    UntilEof,
}

impl RBuf {
    pub fn new(capacity: usize) -> Self {
        RBuf {
            buf: vec![0; capacity.max(512)],
            start: 0,
            end: 0,
        }
    }

    async fn fill<R: AsyncRead + Unpin>(&mut self, r: &mut R) -> Result<()> {
        if self.start == self.end {
            self.start = 0;
            self.end = 0;
        } else if self.end == self.buf.len() {
            self.buf.copy_within(self.start..self.end, 0);
            self.end -= self.start;
            self.start = 0;
            if self.end == self.buf.len() {
                return Err(invalid("http: malformed header line"));
            }
        }
        let n = r.read(&mut self.buf[self.end..]).await?;
        if n == 0 {
            return Err(Error::new(ErrorKind::UnexpectedEof, "EOF"));
        }
        self.end += n;
        Ok(())
    }

    /// One line without its CRLF, as a range of buf that stays valid until
    /// the next read.
    async fn read_line<R: AsyncRead + Unpin>(&mut self, r: &mut R) -> Result<(usize, usize)> {
        let mut scanned = self.start;
        loop {
            if let Some(i) = self.buf[scanned..self.end].iter().position(|&b| b == b'\n') {
                let nl = scanned + i;
                let begin = self.start;
                let mut end = nl;
                if end > begin && self.buf[end - 1] == b'\r' {
                    end -= 1;
                }
                self.start = nl + 1;
                return Ok((begin, end));
            }
            let offset = scanned - self.start;
            self.fill(r).await?;
            scanned = self.start + offset;
        }
    }

    /// n bytes of body, appended to dst when there is one and dropped when
    /// there is not.
    async fn take<R: AsyncRead + Unpin>(
        &mut self,
        r: &mut R,
        mut n: usize,
        mut dst: Option<&mut Vec<u8>>,
    ) -> Result<()> {
        while n > 0 {
            if self.start == self.end {
                // A body bigger than what is buffered goes straight into dst.
                if let Some(dst) = dst.as_deref_mut() {
                    if n >= self.buf.len() {
                        let at = dst.len();
                        dst.resize(at + n, 0);
                        r.read_exact(&mut dst[at..]).await?;
                        return Ok(());
                    }
                }
                self.fill(r).await?;
            }
            let k = n.min(self.end - self.start);
            if let Some(dst) = dst.as_deref_mut() {
                dst.extend_from_slice(&self.buf[self.start..self.start + k]);
            }
            self.start += k;
            n -= k;
        }
        Ok(())
    }

    /// protocol.ReadResponse and DiscardResponse: one response's status and
    /// the length of its body, which goes into dst[..] when dst is given.
    pub async fn read_response<R: AsyncRead + Unpin>(
        &mut self,
        r: &mut R,
        mut dst: Option<&mut Vec<u8>>,
        no_length: NoLength,
    ) -> Result<(u16, usize)> {
        if let Some(dst) = dst.as_deref_mut() {
            dst.clear();
        }
        let (b, e) = self.read_line(r).await?;
        let status = parse_status(&self.buf[b..e])?;

        let mut content_length: Option<usize> = None;
        let mut chunked = false;
        loop {
            let (b, e) = self.read_line(r).await?;
            let line = &self.buf[b..e];
            if line.is_empty() {
                break;
            }
            let colon = line
                .iter()
                .position(|&c| c == b':')
                .filter(|&i| i > 0)
                .ok_or_else(|| invalid("http: malformed header line"))?;
            let name = &line[..colon];
            let value = line[colon + 1..].trim_ascii();
            if name.eq_ignore_ascii_case(b"Content-Length") {
                let len = std::str::from_utf8(value)
                    .ok()
                    .and_then(|v| v.parse::<usize>().ok())
                    .ok_or_else(|| invalid("http: bad Content-Length"))?;
                content_length = Some(len);
            } else if name.eq_ignore_ascii_case(b"Transfer-Encoding") {
                chunked = value
                    .to_ascii_lowercase()
                    .windows(7)
                    .any(|w| w == b"chunked");
            }
        }

        if status / 100 == 1 || status == 204 || status == 304 {
            return Ok((status, 0));
        }
        if chunked {
            return self.read_chunked(r, status, dst).await;
        }
        if let Some(n) = content_length {
            if n > MAX_BODY_SIZE {
                return Err(invalid(
                    "http: response body larger than the client accepts",
                ));
            }
            self.take(r, n, dst).await?;
            return Ok((status, n));
        }
        match no_length {
            NoLength::Error => Err(invalid(
                "http: response has neither Content-Length nor chunked body",
            )),
            NoLength::UntilEof => {
                let mut n = 0;
                loop {
                    if self.start == self.end {
                        match self.fill(r).await {
                            Ok(()) => {}
                            Err(err) if err.kind() == ErrorKind::UnexpectedEof => {
                                return Ok((status, n));
                            }
                            Err(err) => return Err(err),
                        }
                    }
                    let k = self.end - self.start;
                    if let Some(dst) = dst.as_deref_mut() {
                        dst.extend_from_slice(&self.buf[self.start..self.end]);
                    }
                    self.start = self.end;
                    n += k;
                }
            }
        }
    }

    async fn read_chunked<R: AsyncRead + Unpin>(
        &mut self,
        r: &mut R,
        status: u16,
        mut dst: Option<&mut Vec<u8>>,
    ) -> Result<(u16, usize)> {
        let mut total = 0;
        loop {
            let (b, e) = self.read_line(r).await?;
            let mut line = &self.buf[b..e];
            if let Some(semi) = line.iter().position(|&c| c == b';') {
                line = &line[..semi];
            }
            let size = std::str::from_utf8(line.trim_ascii())
                .ok()
                .and_then(|s| usize::from_str_radix(s, 16).ok())
                .ok_or_else(|| invalid("http: malformed chunk"))?;
            if size == 0 {
                // The trailer, if any, up to the blank line that ends it.
                loop {
                    let (b, e) = self.read_line(r).await?;
                    if b == e {
                        return Ok((status, total));
                    }
                }
            }
            if total + size > MAX_BODY_SIZE {
                return Err(invalid(
                    "http: response body larger than the client accepts",
                ));
            }
            self.take(r, size, dst.as_deref_mut()).await?;
            total += size;
            let (b, e) = self.read_line(r).await?;
            if b != e {
                return Err(invalid("http: malformed chunk"));
            }
        }
    }
}

/// The code off "HTTP/1.1 200 OK".
fn parse_status(line: &[u8]) -> Result<u16> {
    if line.len() < 12 || !line.starts_with(b"HTTP/1.") || line[8] != b' ' {
        return Err(invalid("http: malformed status line"));
    }
    let mut status = 0u16;
    for &c in &line[9..12] {
        if !c.is_ascii_digit() {
            return Err(invalid("http: malformed status line"));
        }
        status = status * 10 + u16::from(c - b'0');
    }
    if line.len() > 12 && line[12] != b' ' {
        return Err(invalid("http: malformed status line"));
    }
    Ok(status)
}

/// One control request - /init, /ps, pprof - on a connection of its own,
/// closed after the answer: status and body.
pub async fn control_request(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> Result<(u16, Vec<u8>)> {
    let fut = async {
        let mut stream =
            TcpStream::connect(format!("{}:{port}", super::config::url_host(host))).await?;
        let mut request = encode_request(method, &super::config::url_host(host), path, body);
        // Connection: close, before the blank line that ends the header.
        let at = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 2;
        request.splice(at..at, b"Connection: close\r\n".iter().copied());
        stream.write_all(&request).await?;
        let mut rbuf = RBuf::new(16 * 1024);
        let mut out = Vec::new();
        let (status, _) = rbuf
            .read_response(&mut stream, Some(&mut out), NoLength::UntilEof)
            .await?;
        Ok((status, out))
    };
    tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| Error::new(ErrorKind::TimedOut, "timed out"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
    }

    #[test]
    fn reads_responses() {
        rt().block_on(async {
            let stream = b"HTTP/1.1 200 OK\r\nContent-Type: x\r\ncontent-length: 5\r\n\r\nhello\
HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3;ext=1\r\nabc\r\n2\r\nde\r\n0\r\nX-Trailer: 1\r\n\r\n\
HTTP/1.1 204 No Content\r\n\r\n\
HTTP/1.0 404 Not Found\r\nContent-Length: 3\r\n\r\nno!\
HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nskip";
            let mut body = Vec::new();
            let mut r: &[u8] = stream;
            let mut rb = RBuf::new(512);
            for (status, want) in [(200, "hello"), (200, "abcde"), (204, ""), (404, "no!")] {
                let (s, n) = rb.read_response(&mut r, Some(&mut body), NoLength::Error).await.unwrap();
                assert_eq!((s, n, body.as_slice()), (status, want.len(), want.as_bytes()));
            }
            assert_eq!(rb.read_response(&mut r, None, NoLength::Error).await.unwrap(), (200, 4));
            assert!(rb.read_response(&mut r, None, NoLength::Error).await.is_err());
        });
    }

    #[test]
    fn rejects() {
        rt().block_on(async {
            for raw in [
                &b"HTTP/2 200 OK\r\n\r\n"[..],
                b"HTTP/1.1 2x0 OK\r\n\r\n",
                b"HTTP/1.1 200 OK\r\nno colon here\r\n\r\n",
                b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n",
                b"HTTP/1.1 200 OK\r\n\r\nread until close",
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
            ] {
                let mut r = raw;
                assert!(
                    RBuf::new(512)
                        .read_response(&mut r, None, NoLength::Error)
                        .await
                        .is_err()
                );
            }
            let mut r: &[u8] = b"HTTP/1.1 200 OK\r\n\r\nread until close";
            let mut body = Vec::new();
            let got = RBuf::new(512)
                .read_response(&mut r, Some(&mut body), NoLength::UntilEof)
                .await
                .unwrap();
            assert_eq!(
                (got, body.as_slice()),
                ((200, 16), &b"read until close"[..])
            );
        });
    }

    #[test]
    fn big_body_skips_the_buffer() {
        rt().block_on(async {
            let mut raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5000\r\n\r\n".to_vec();
            raw.extend(std::iter::repeat_n(b'x', 5000));
            let mut r = raw.as_slice();
            let mut body = Vec::new();
            let got = RBuf::new(512)
                .read_response(&mut r, Some(&mut body), NoLength::Error)
                .await
                .unwrap();
            assert_eq!(got, (200, 5000));
            assert!(body.len() == 5000 && body.iter().all(|&b| b == b'x'));
        });
    }

    #[test]
    fn batches() {
        let buf = vec![0u8; 1100];
        let (b, n, tick) = batch_buffers(&buf, 200, 16 * 1024);
        assert_eq!((b.len(), n, tick), (11000, 10, 20));
        let (_, n, tick) = batch_buffers(&vec![0u8; 32 * 1024], 7, 16 * 1024);
        assert_eq!((n, tick), (1, 7));
        assert!(validate_pipeline(25, 200).is_ok());
        assert!(validate_pipeline(3, 200).is_err());
        assert!(validate_pipeline(-1, 200).is_err());
        let get = encode_request("GET", "[::1]", "/echo", None);
        assert_eq!(get, b"GET /echo HTTP/1.1\r\nHost: [::1]\r\n\r\n");
    }
}
