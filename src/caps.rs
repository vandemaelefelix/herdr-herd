//! Runtime detection of whether the outer terminal (through herdr) will render
//! kitty graphics. Self-correcting: if herdr's experimental flag is off, the
//! query is swallowed and no reply arrives, so we report unsupported and the
//! caller falls back to half-blocks. The tty I/O lives behind this trait so
//! tests never touch a real terminal.

use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

/// Whether the current terminal supports the kitty graphics protocol.
pub trait TerminalCaps {
    fn supports_kitty_graphics(&mut self) -> bool;
}

/// True if `buf` contains a kitty graphics reply naming image id `id`
/// (`\x1b_G...i=<id>...\x1b\`). Pure; unit-tested.
pub fn reply_confirms(buf: &[u8], id: u32) -> bool {
    let text = String::from_utf8_lossy(buf);
    text.split("\x1b_G").skip(1).any(|seg| {
        seg.split(['\x1b', ';', ','])
            .any(|tok| tok == format!("i={id}"))
    })
}

/// Reads the real tty. Assumes raw mode is already enabled by the caller
/// (the render loop enables it); writes the probe and polls stdin briefly.
pub struct RealCaps {
    id: u32,
    timeout: Duration,
}

impl RealCaps {
    pub fn new() -> Self {
        Self {
            id: 0x7E51,
            timeout: Duration::from_millis(150),
        }
    }
}

impl Default for RealCaps {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalCaps for RealCaps {
    fn supports_kitty_graphics(&mut self) -> bool {
        let query = crate::kitty::probe_query(self.id);
        if io::stdout().write_all(query.as_bytes()).is_err() || io::stdout().flush().is_err() {
            return false;
        }
        // Poll stdin for a reply until the deadline. crossterm's event stream
        // is already in use by the caller after this returns, so read raw here
        // before the loop starts.
        let deadline = Instant::now() + self.timeout;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 256];
        let mut stdin = io::stdin();
        while Instant::now() < deadline {
            // Non-blocking-ish: rely on crossterm::event::poll for readiness.
            if crossterm::event::poll(Duration::from_millis(20)).unwrap_or(false) {
                // Drain available bytes via a raw read.
                if let Ok(n) = stdin.read(&mut chunk) {
                    buf.extend_from_slice(&chunk[..n]);
                    if reply_confirms(&buf, self.id) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Test double: reports a fixed, configured support value with no tty I/O.
#[cfg(test)]
pub struct FakeCaps {
    pub supported: bool,
}

#[cfg(test)]
impl TerminalCaps for FakeCaps {
    fn supports_kitty_graphics(&mut self) -> bool {
        self.supported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_reports_configured_support() {
        assert!(FakeCaps { supported: true }.supports_kitty_graphics());
        assert!(!FakeCaps { supported: false }.supports_kitty_graphics());
    }

    #[test]
    fn reply_matcher_accepts_only_matching_image_id() {
        // Pure parser used by RealCaps, unit-tested without a terminal.
        assert!(reply_confirms(b"\x1b_Gi=31,OK\x1b\\", 31));
        assert!(!reply_confirms(b"\x1b_Gi=99;OK\x1b\\", 31));
        assert!(!reply_confirms(b"garbage", 31));
    }
}
