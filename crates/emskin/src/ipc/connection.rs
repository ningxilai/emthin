use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;

/// Maximum allowed IPC message payload size (1 MiB).
const MAX_MSG_SIZE: usize = 1024 * 1024;

/// A single active IPC connection (one Emacs client).
pub struct IpcConn {
    pub(super) stream: UnixStream,
    /// Incomplete incoming bytes waiting to form a full message.
    read_buf: Vec<u8>,
    /// Serialized bytes queued for writing.
    write_buf: VecDeque<u8>,
}

impl IpcConn {
    pub fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            read_buf: Vec::new(),
            write_buf: VecDeque::new(),
        })
    }

    /// Drain available bytes from the stream into `read_buf`.
    /// Returns `true` if the peer closed the connection.
    pub fn fill_read_buf(&mut self) -> io::Result<bool> {
        let mut tmp = [0u8; 4096];
        loop {
            match self.stream.read(&mut tmp) {
                Ok(0) => return Ok(true), // EOF — peer closed
                Ok(n) => self.read_buf.extend_from_slice(&tmp[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(e) => return Err(e),
            }
        }
    }

    /// Attempt to decode and return the next complete message, if available.
    /// Returns `Err` if the framed length exceeds `MAX_MSG_SIZE`.
    pub fn try_recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        let header_end = self.read_buf.windows(4).position(|w| w == b"\r\n\r\n");
        let Some(header_end) = header_end else {
            return Ok(None);
        };
        let header = &self.read_buf[..header_end];
        let header_str = std::str::from_utf8(header)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-utf8 header"))?;
        let len = header_str
            .strip_prefix("Content-Length:")
            .and_then(|s| s.trim().parse::<usize>().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length"))?;
        if len > MAX_MSG_SIZE {
            self.read_buf.clear();
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Content-Length {len} exceeds maximum {MAX_MSG_SIZE}"),
            ));
        }
        let body_start = header_end + 4;
        let body_end = body_start + len;
        if self.read_buf.len() < body_end {
            return Ok(None);
        }
        let payload = self.read_buf[body_start..body_end].to_vec();
        self.read_buf.drain(..body_end);
        Ok(Some(payload))
    }

    /// Enqueue a raw byte payload with a `Content-Length` header.
    pub fn enqueue_raw(&mut self, data: &[u8]) {
        let header = format!("Content-Length: {}\r\n\r\n", data.len());
        self.write_buf.extend(header.as_bytes());
        self.write_buf.extend(data);
    }

    /// Flush as many bytes as possible from `write_buf` without blocking.
    /// Returns `true` if there is still data remaining to write.
    pub fn try_flush(&mut self) -> io::Result<bool> {
        while !self.write_buf.is_empty() {
            // Collect contiguous bytes for a single write call.
            let (front, back) = self.write_buf.as_slices();
            let slice = if !front.is_empty() { front } else { back };
            match self.stream.write(slice) {
                Ok(0) => return Ok(!self.write_buf.is_empty()),
                Ok(n) => {
                    self.write_buf.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    return Ok(true);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(false)
    }

    #[cfg(test)]
    pub fn has_pending_writes(&self) -> bool {
        !self.write_buf.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    fn make_pair() -> (IpcConn, UnixStream) {
        let (a, b) = UnixStream::pair().unwrap();
        (IpcConn::new(a).unwrap(), b)
    }

    fn write_content_length(stream: &mut UnixStream, payload: &[u8]) {
        let header = format!("Content-Length: {}\r\n\r\n", payload.len());
        stream.write_all(header.as_bytes()).unwrap();
        stream.write_all(payload).unwrap();
    }

    #[test]
    fn try_recv_returns_none_on_empty_buffer() {
        let (mut conn, _peer) = make_pair();
        assert!(conn.try_recv().unwrap().is_none());
    }

    #[test]
    fn try_recv_returns_none_on_incomplete_header() {
        let (mut conn, mut peer) = make_pair();
        peer.write_all(b"Content-L").unwrap();
        conn.fill_read_buf().ok();
        assert!(conn.try_recv().unwrap().is_none());
    }

    #[test]
    fn try_recv_decodes_single_message() {
        let (mut conn, mut peer) = make_pair();
        let payload = b"hello world";
        write_content_length(&mut peer, payload);
        conn.fill_read_buf().ok();
        let msg = conn.try_recv().unwrap().unwrap();
        assert_eq!(msg, payload);
    }

    #[test]
    fn try_recv_returns_none_on_incomplete_payload() {
        let (mut conn, mut peer) = make_pair();
        let header = b"Content-Length: 10\r\n\r\n";
        peer.write_all(header).unwrap();
        peer.write_all(b"hello").unwrap();
        conn.fill_read_buf().ok();
        assert!(conn.try_recv().unwrap().is_none());
    }

    #[test]
    fn try_recv_handles_multiple_messages_in_one_read() {
        let (mut conn, mut peer) = make_pair();
        write_content_length(&mut peer, b"msg1");
        write_content_length(&mut peer, b"msg2");
        write_content_length(&mut peer, b"msg3");
        conn.fill_read_buf().ok();

        assert_eq!(conn.try_recv().unwrap().unwrap(), b"msg1");
        assert_eq!(conn.try_recv().unwrap().unwrap(), b"msg2");
        assert_eq!(conn.try_recv().unwrap().unwrap(), b"msg3");
        assert!(conn.try_recv().unwrap().is_none());
    }

    #[test]
    fn try_recv_handles_empty_payload() {
        let (mut conn, mut peer) = make_pair();
        write_content_length(&mut peer, b"");
        conn.fill_read_buf().ok();
        let msg = conn.try_recv().unwrap().unwrap();
        assert!(msg.is_empty());
    }

    #[test]
    fn try_recv_rejects_oversized_message() {
        let (mut conn, mut peer) = make_pair();
        let header = b"Content-Length: 2097152\r\n\r\n";
        peer.write_all(header).unwrap();
        conn.fill_read_buf().ok();
        let result = conn.try_recv();
        assert!(result.is_err());
    }

    #[test]
    fn enqueue_and_flush_roundtrip() {
        let (mut conn, mut peer) = make_pair();
        peer.set_nonblocking(true).unwrap();

        let payload = b"{\"jsonrpc\":\"2.0\",\"method\":\"test\"}";
        conn.enqueue_raw(payload);
        assert!(conn.has_pending_writes());

        conn.try_flush().unwrap();
        assert!(!conn.has_pending_writes());

        // Read the Content-Length framed message from the peer side.
        peer.set_nonblocking(false).unwrap();

        // Read until we see \r\n\r\n
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1];
        loop {
            peer.read_exact(&mut tmp).unwrap();
            buf.push(tmp[0]);
            if buf.len() >= 4 && buf[buf.len() - 4..] == [b'\r', b'\n', b'\r', b'\n'] {
                break;
            }
        }
        let header_str = std::str::from_utf8(&buf[..buf.len() - 4]).unwrap();
        let len: usize = header_str
            .strip_prefix("Content-Length: ")
            .and_then(|s| s.trim().parse().ok())
            .unwrap();
        let mut body = vec![0u8; len];
        peer.read_exact(&mut body).unwrap();
        assert_eq!(body, payload);
    }

    #[test]
    fn fill_read_buf_detects_eof() {
        let (mut conn, peer) = make_pair();
        drop(peer); // Close the peer end.
        let eof = conn.fill_read_buf().unwrap();
        assert!(eof);
    }
}
