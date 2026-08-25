//! The herdr control socket client.
//!
//! Two shapes of client live here, because herdr's control socket has two:
//!
//! - a **persistent** subscription (`SocketClient`/`RealSocket`), which sends
//!   one `events.subscribe` and then reads framed event lines forever; and
//! - a **one-shot** request (`RpcClient`/`UnixRpcClient`, over `request_line`),
//!   which asks herdr a question and reads the single reply line. herdr closes
//!   the connection after answering, so each question gets its own.
//!
//! Asking over the socket is how the plugin avoids fork/exec'ing the `herdr`
//! CLI: see `RpcClient`. The older read-to-EOF `request` helper (from Phase 0
//! Spike A) is retained for simple calls such as `layout.export` /
//! `layout.apply`. (Spike A verified the wire uses newline-delimited JSON-RPC
//! with dotted method names, see the design doc §5.)

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

/// Connect, send `payload` + a newline, and read exactly one reply line (with
/// the trailing newline stripped). Unlike [`request`], this does **not** wait
/// for the server to close: the herdr control socket is persistent, so a
/// request/reply is framed by the newline, not by EOF.
pub fn request_line(path: &Path, payload: &str) -> std::io::Result<String> {
    let stream = UnixStream::connect(path)?;
    let mut writer = stream.try_clone()?;
    writer.write_all(payload.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        return Err(std::io::Error::other("socket closed before reply"));
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
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
    /// Bytes of the current line read so far. `BufRead::read_line` is not
    /// restartable: on a timeout it has already consumed and appended bytes
    /// to its buffer before `fill_buf` errors. Keeping that buffer here
    /// (instead of a fresh `String` per call) means a timeout mid-line
    /// carries the partial line forward to the next `recv_line` call instead
    /// of losing it.
    pending: String,
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
            pending: String::new(),
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
        // On a timeout, `read_line` returns `Err` with whatever it already
        // appended to `pending` left in place, so the next call resumes
        // mid-line instead of starting from a blank buffer.
        let n = self.reader.read_line(&mut self.pending)?;
        if n == 0 {
            return Err(std::io::Error::other("socket closed"));
        }
        Ok(std::mem::take(&mut self.pending)
            .trim_end_matches(['\r', '\n'])
            .to_string())
    }
}

/// A one-shot JSON-RPC call against the herdr control socket, behind a trait so
/// tests never touch a real socket.
///
/// This is the cheap way to ask herdr a question. Shelling out to the `herdr`
/// CLI forks and execs an 18 MB binary for every question (~68 ms wall, ~7.6 ms
/// of CPU before it has even parsed its arguments); a call here is a connect and
/// a round trip on a unix socket.
pub trait RpcClient {
    /// Send `payload` as one request line and return the single reply line.
    fn call(&self, payload: &str) -> std::io::Result<String>;
}

/// An [`RpcClient`] over the herdr control socket, opening a short-lived
/// connection per call.
///
/// Per call, not once: herdr closes a control connection as soon as it has
/// answered a plain request (verified live against 0.8.0: a second request on
/// the same stream gets `EPIPE`). Only an `events.subscribe` connection stays
/// open, and that one cannot be reused for requests either, because its replies
/// would interleave with the event stream the watcher is reading.
pub struct UnixRpcClient {
    path: PathBuf,
}

impl UnixRpcClient {
    /// Point a client at an explicit socket path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// From `$HERDR_SOCKET_PATH`; `None` outside a herdr session, which is the
    /// caller's signal to stay on the CLI path.
    pub fn from_env() -> Option<Self> {
        socket_path().map(Self::new)
    }
}

impl RpcClient for UnixRpcClient {
    fn call(&self, payload: &str) -> std::io::Result<String> {
        request_line(&self.path, payload)
    }
}

/// The `session.snapshot` request line: agents, workspace/tab labels, panes and
/// every tab's layout, in one reply (see [`crate::snapshot`]).
pub fn snapshot_request() -> String {
    rpc_request("herd:snapshot", "session.snapshot", serde_json::json!({}))
}

/// The `pane.process_info` request line for one pane: what the controller uses
/// to tell a live strip from a labelled corpse.
pub fn process_info_request(pane_id: &str) -> String {
    rpc_request(
        "herd:process-info",
        "pane.process_info",
        serde_json::json!({ "pane_id": pane_id }),
    )
}

/// Build one newline-free JSON-RPC request line. `params` is always sent, even
/// when empty: herdr's request schema requires the key.
fn rpc_request(id: &str, method: &str, params: serde_json::Value) -> String {
    serde_json::json!({ "id": id, "method": method, "params": params }).to_string()
}

/// The `events.subscribe` request line (verified live — Spike 1, herdr 0.7.0).
///
/// `params.subscriptions` is required and each entry is an internally-tagged
/// enum keyed by `type` (dotted names). We subscribe to the **global**
/// structural events that signal the herd changed — panes/agents appearing,
/// disappearing, or being detected — plus the **focus** events
/// (`pane.focused`/`tab.focused`/`workspace.focused`) so the "active" hat
/// tracks the focused agent promptly instead of lagging up to a slow-poll
/// interval behind it. Pure status transitions
/// (`idle`↔`working`↔`blocked`↔`done`) arrive only via the per-pane
/// `pane.agent_status_changed` subscription (which requires a `pane_id`), so
/// those are still covered by the watcher's slow poll rather than by an event.
/// On connect herdr also replays current state, giving an immediate structural
/// snapshot. (Stream event names use underscores, e.g. `pane_created`; the
/// watcher ignores event contents and just refetches.)
///
/// The global focus subscriptions wake every render process on one pane
/// switch (#73). Checked herdr 0.8.0's schema (`herdr api schema --json`) for
/// a narrower alternative: `pane.focused`/`tab.focused`/`workspace.focused`
/// take no target filter at all, only `pane.output_matched`/
/// `pane.agent_status_changed`/`pane.scroll_changed` accept one `pane_id`.
/// Since the herd strip renders the whole session, not just its own pane
/// (#31), a per-pane subscription would mean one subscription per pane that
/// exists, re-issued as panes come and go, not a straight swap for the global
/// one. Narrowing this needs either a herdr API change or a decision on what
/// a session-wide strip should actually watch, so it stays global here.
pub fn subscribe_request() -> String {
    r#"{"id":"members","method":"events.subscribe","params":{"subscriptions":[{"type":"pane.created"},{"type":"pane.closed"},{"type":"pane.exited"},{"type":"pane.focused"},{"type":"pane.agent_detected"},{"type":"tab.created"},{"type":"tab.closed"},{"type":"tab.focused"},{"type":"workspace.focused"}]}}"#.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixListener;

    #[test]
    fn real_socket_sends_and_receives_framed_lines() {
        let path = std::env::temp_dir().join(format!("herdr-herd-rt-{}", std::process::id()));
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

    /// A line that crosses the 400ms read-timeout boundary must not lose its
    /// head: the byte-timeout `recv_line` must resume mid-line rather than
    /// discarding what it already read and later returning just the tail.
    #[test]
    fn recv_line_resumes_a_line_split_across_a_read_timeout() {
        let path = std::env::temp_dir().join(format!("herdr-herd-split-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn({
            let path = path.clone();
            move || {
                let (mut conn, _) = listener.accept().unwrap();
                conn.write_all(b"{\"first_half\":").unwrap();
                conn.flush().unwrap();
                // Long enough to clear the client's 400ms read timeout at
                // least once before the rest of the line arrives.
                std::thread::sleep(std::time::Duration::from_millis(700));
                conn.write_all(b"\"second_half\"}\n").unwrap();
                conn.flush().unwrap();
                let _ = std::fs::remove_file(&path);
            }
        });

        let mut c = RealSocket::connect(&path).unwrap();
        let mut timed_out = false;
        let line = loop {
            match c.recv_line() {
                Ok(line) => break line,
                Err(e) => {
                    assert!(
                        matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ),
                        "unexpected error: {e}"
                    );
                    timed_out = true;
                }
            }
        };
        assert!(timed_out, "the test is only meaningful if a timeout fired");
        assert_eq!(line, r#"{"first_half":"second_half"}"#);
        server.join().unwrap();
    }

    #[test]
    fn subscribe_request_is_valid_json_line() {
        let s = subscribe_request();
        assert!(s.contains("events.subscribe"));
        assert!(!s.contains('\n'));
        // The focus event is what keeps the "active" hat in sync; without it,
        // focus changes only surface on the slow poll (up to seconds late).
        assert!(
            s.contains(r#"{"type":"pane.focused"}"#),
            "must subscribe to pane.focused so the active hat updates promptly"
        );
    }

    #[test]
    fn snapshot_request_is_one_json_line_asking_for_the_session_snapshot() {
        let s = snapshot_request();
        assert!(!s.contains('\n'), "the wire is newline-framed");
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid JSON-RPC");
        assert_eq!(v["method"], "session.snapshot");
        assert!(
            v.get("params").is_some(),
            "herdr's request schema requires params, even empty"
        );
    }

    #[test]
    fn process_info_request_names_the_pane_and_escapes_it() {
        let v: serde_json::Value =
            serde_json::from_str(&process_info_request("w1:p1")).expect("valid JSON-RPC");
        assert_eq!(v["method"], "pane.process_info");
        assert_eq!(v["params"]["pane_id"], "w1:p1");

        // A pane id is herdr's to shape, so it is escaped rather than pasted in.
        let odd = process_info_request(r#"we"ird"#);
        let v: serde_json::Value = serde_json::from_str(&odd).expect("still valid JSON");
        assert_eq!(v["params"]["pane_id"], r#"we"ird"#);
    }

    #[test]
    fn unix_rpc_client_sends_one_request_and_returns_one_reply_line() {
        let path = std::env::temp_dir().join(format!("herdr-herd-rpc-{}", std::process::id()));
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
                w.write_all(b"{\"result\":{\"snapshot\":{}}}\n").unwrap();
                let _ = std::fs::remove_file(&path);
                got
            }
        });

        let client = UnixRpcClient::new(&path);
        let reply = client.call(&snapshot_request()).unwrap();
        assert_eq!(reply, r#"{"result":{"snapshot":{}}}"#);
        let got = server.join().unwrap();
        assert!(got.contains("session.snapshot"));
    }

    #[test]
    fn unix_rpc_client_errors_when_there_is_no_socket_to_talk_to() {
        let client = UnixRpcClient::new("/nonexistent/herdr-herd-no-such.sock");
        assert!(
            client.call(&snapshot_request()).is_err(),
            "a dead socket must surface as an error so the caller can fall back"
        );
    }

    #[test]
    fn request_writes_payload_and_reads_reply() {
        let dir = std::env::temp_dir().join(format!("herdr-herd-sock-{}", std::process::id()));
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
                    if got.contains('\n') {
                        break;
                    }
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

    #[test]
    fn request_line_reads_one_reply_line_without_needing_eof() {
        let path = std::env::temp_dir().join(format!("herdr-herd-rl-{}", std::process::id()));
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
                w.write_all(b"{\"reply\":1}\n").unwrap();
                w.flush().unwrap();
                // Hold the connection open (a persistent socket does not EOF):
                // block on a second read, which only returns once the client
                // drops its stream. A read-to-EOF implementation would still
                // be blocked here when the client-side timeout below fires.
                let mut trailing = String::new();
                let _ = r.read_line(&mut trailing);
                let _ = std::fs::remove_file(&path);
                got
            }
        });

        // Run the client call on a worker thread so a hung (read-to-EOF)
        // implementation can't block this test forever: we bound the wait
        // with recv_timeout instead.
        let (tx, rx) = std::sync::mpsc::channel();
        let client = std::thread::spawn({
            let path = path.clone();
            move || {
                let result = request_line(&path, "{\"ping\":1}");
                let _ = tx.send(result);
            }
        });

        let result = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("request_line did not return promptly after one reply line");
        assert_eq!(result.unwrap(), "{\"reply\":1}");

        client.join().unwrap();
        let got = server.join().unwrap();
        assert_eq!(got, "{\"ping\":1}\n");
    }

    #[test]
    fn request_line_errors_when_the_socket_closes_before_a_reply() {
        let path = std::env::temp_dir().join(format!("herdr-herd-rl-close-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn({
            let path = path.clone();
            move || {
                let (conn, _) = listener.accept().unwrap();
                // Accept, then close without replying.
                drop(conn);
                let _ = std::fs::remove_file(&path);
            }
        });

        let result = request_line(&path, "{\"ping\":1}");
        assert!(result.is_err());

        server.join().unwrap();
    }
}
