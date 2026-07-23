//! The herdr control socket client.
//!
//! This is the persistent, line-delimited JSON-RPC client
//! (`SocketClient`/`RealSocket`) used to subscribe to socket events and
//! receive framed lines, one per newline-delimited JSON-RPC message. It also
//! retains the one-shot `request` helper (from Phase 0 Spike A) for simple
//! request/reply calls such as `layout.export` / `layout.apply` against
//! `$HERDR_SOCKET_PATH`. (Spike A verified the wire uses newline-delimited
//! JSON-RPC with dotted method names — see the design doc §5.)

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
    /// Returns an error if the socket is closed (a zero-byte read). On
    /// `RealSocket`, an `Err` whose `kind()` is `WouldBlock` or `TimedOut`
    /// means no line arrived within the read timeout — a normal idle tick,
    /// not a dead connection. `Ok(0)`/EOF is still a real close, surfaced as
    /// `io::Error::other("socket closed")`.
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
    ///
    /// The read half gets a short read timeout so `recv_line` cannot block
    /// forever, letting the watcher's slow-poll safety net run even while
    /// connected (see `recv_line`).
    pub fn connect(path: &Path) -> std::io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        let read_half = stream.try_clone()?;
        read_half.set_read_timeout(Some(std::time::Duration::from_millis(400)))?;
        let reader = BufReader::new(read_half);
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

/// The `events.subscribe` request line (verified live — Spike 1, herdr 0.7.0).
///
/// `params.subscriptions` is required and each entry is an internally-tagged
/// enum keyed by `type` (dotted names). We subscribe to the **global**
/// structural events that signal the herd changed — panes/agents appearing,
/// disappearing, or being detected — so the watcher refetches promptly. Pure
/// status transitions (`idle`↔`working`↔`blocked`↔`done`) arrive only via the
/// per-pane `pane.agent_status_changed` subscription (which requires a
/// `pane_id`), so those are covered here by the watcher's slow poll rather than
/// by an event. On connect herdr also replays current state, giving an
/// immediate structural snapshot. (Stream event names use underscores, e.g.
/// `pane_created`; the watcher ignores event contents and just refetches.)
pub fn subscribe_request() -> String {
    r#"{"id":"pets","method":"events.subscribe","params":{"subscriptions":[{"type":"pane.created"},{"type":"pane.closed"},{"type":"pane.exited"},{"type":"pane.agent_detected"},{"type":"tab.created"},{"type":"tab.closed"}]}}"#.to_string()
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
