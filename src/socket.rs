//! Minimal raw unix-socket helper — Spike A scaffolding only.
//!
//! Phase 0 does NOT ship a full socket client; that is Phase 1 (event
//! subscription). This exists so Spike A can send a `layout.export` /
//! `layout.apply` request to `$HERDR_SOCKET_PATH` and read the reply.
//! (Spike A verified the wire uses newline-delimited JSON-RPC with dotted
//! method names — see the design doc §5.)

use std::io::{Read, Write};
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;

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
