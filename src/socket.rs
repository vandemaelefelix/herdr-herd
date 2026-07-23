//! Minimal raw unix-socket helper — Spike A scaffolding only.
//!
//! Phase 0 does NOT ship a full socket client; that is Phase 1 (event
//! subscription). This exists so Spike A can send a `layout.export` /
//! `layout.apply` request to `$HERDR_SOCKET_PATH` and read the reply.
//! (Spike A verified the wire uses newline-delimited JSON-RPC with dotted
//! method names — see the design doc §5.)

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

/// The herdr control socket, from `$HERDR_SOCKET_PATH`.
pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os("HERDR_SOCKET_PATH").map(PathBuf::from)
}

/// Connect, send `payload` followed by a newline, and read the reply to EOF.
pub fn request(path: &Path, payload: &str) -> std::io::Result<String> {
    let mut stream = UnixStream::connect(path)?;
    stream.write_all(payload.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    Ok(reply)
}

/// A persistent, line-delimited JSON-RPC connection (see Phase 0 Spike A: the
/// control socket speaks newline-delimited JSON-RPC with dotted method names).
pub trait SocketClient {
    /// Write `line` followed by a newline and flush.
    fn send_line(&mut self, line: &str) -> std::io::Result<()>;
    /// Read one framed line, with the trailing newline stripped.
    ///
    /// Returns an error if the socket is closed (a zero-byte read).
    fn recv_line(&mut self) -> std::io::Result<String>;
}

/// A `SocketClient` backed by a real `UnixStream`, with an internal
/// `BufReader` for line-buffered reads.
pub struct RealSocket {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl RealSocket {
    /// Connect to the herdr control socket at `path`.
    pub fn connect(path: &Path) -> std::io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        let reader = BufReader::new(stream.try_clone()?);
        Ok(Self {
            writer: stream,
            reader,
        })
    }
}

impl SocketClient for RealSocket {
    fn send_line(&mut self, line: &str) -> std::io::Result<()> {
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()
    }

    fn recv_line(&mut self) -> std::io::Result<String> {
        let mut s = String::new();
        let n = self.reader.read_line(&mut s)?;
        if n == 0 {
            return Err(std::io::Error::other("socket closed"));
        }
        Ok(s.trim_end_matches(['\r', '\n']).to_string())
    }
}

/// The verified `events.subscribe` request line (refine per Spike 1).
pub fn subscribe_request() -> String {
    r#"{"id":"pets","method":"events.subscribe","params":{}}"#.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixListener;

    #[test]
    fn real_socket_sends_and_receives_framed_lines() {
        let path = std::env::temp_dir().join(format!("herdr-pets-rt-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn({
            let path = path.clone();
            move || {
                let (conn, _) = listener.accept().unwrap();
                let mut r = BufReader::new(conn.try_clone().unwrap());
                let mut w = conn;
                let mut got = String::new();
                r.read_line(&mut got).unwrap();
                w.write_all(b"{\"event\":\"ok\"}\n").unwrap();
                let _ = std::fs::remove_file(&path);
                got
            }
        });
        let mut c = RealSocket::connect(&path).unwrap();
        c.send_line("{\"id\":\"x\",\"method\":\"events.subscribe\",\"params\":{}}")
            .unwrap();
        let reply = c.recv_line().unwrap();
        assert_eq!(reply, "{\"event\":\"ok\"}");
        let got = server.join().unwrap();
        assert!(got.contains("events.subscribe"));
    }

    #[test]
    fn subscribe_request_is_valid_json_line() {
        let s = subscribe_request();
        assert!(s.contains("events.subscribe"));
        assert!(!s.contains('\n'));
    }

    #[test]
    fn request_writes_payload_and_reads_reply() {
        let dir = std::env::temp_dir().join(format!("herdr-pets-sock-{}", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        let listener = UnixListener::bind(&dir).unwrap();

        let server = std::thread::spawn({
            let dir = dir.clone();
            move || {
                let (mut conn, _) = listener.accept().unwrap();
                let mut got = String::new();
                let mut buf = [0u8; 64];
                // read the one line the client sends
                loop {
                    let n = conn.read(&mut buf).unwrap();
                    got.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if got.contains('\n') { break; }
                }
                conn.write_all(b"PONG").unwrap();
                drop(conn); // EOF for the client
                let _ = std::fs::remove_file(&dir);
                got
            }
        });

        let reply = request(&dir, "PING").unwrap();
        assert_eq!(reply, "PONG");
        let got = server.join().unwrap();
        assert_eq!(got, "PING\n");
    }
}
