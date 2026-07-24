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

/// True if `buf` contains a complete Primary Device Attributes reply
/// (`ESC [ ? ... c`): the `ESC [ ?` introducer followed by its `c` terminator.
/// The probe appends a DA request after the kitty query — every terminal
/// answers DA, so this terminator is the signal that "the terminal has replied,
/// stop reading" and lets the probe finish without an arbitrary timeout even
/// when kitty is unsupported (only the DA reply comes back). Pure; unit-tested.
pub fn da_terminated(buf: &[u8]) -> bool {
    match buf.windows(3).position(|w| w == b"\x1b[?") {
        Some(pos) => buf[pos + 3..].contains(&b'c'),
        None => false,
    }
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
        // Send the kitty graphics query, then a Primary Device Attributes
        // request (`ESC [ c`). Every terminal answers DA, so a reply always
        // arrives promptly and the blocking read below never hangs — even when
        // kitty is unsupported (herdr's flag off swallows the kitty query, but
        // DA still round-trips), in which case only the DA reply comes back and
        // we report `false`. A kitty response arrives before the DA terminator
        // when kitty works.
        //
        // We deliberately do NOT use `crossterm::event::poll` here: it drains
        // the ready bytes into crossterm's own parser buffer, which would leave
        // a subsequent raw `read` with nothing to read — blocking forever. This
        // runs once at startup, before the event loop, with raw mode already
        // enabled by the caller, so reading stdin directly is safe.
        let mut out = io::stdout();
        let query = crate::kitty::probe_query(self.id);
        if out.write_all(query.as_bytes()).is_err()
            || out.write_all(b"\x1b[c").is_err()
            || out.flush().is_err()
        {
            return false;
        }
        let deadline = Instant::now() + self.timeout;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 256];
        let mut stdin = io::stdin();
        while Instant::now() < deadline {
            match stdin.read(&mut chunk) {
                Ok(0) | Err(_) => break, // EOF or read error: give up (unsupported)
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if reply_confirms(&buf, self.id) {
                        return true; // kitty response seen, before the DA reply
                    }
                    if da_terminated(&buf) {
                        return false; // terminal answered DA with no kitty reply
                    }
                }
            }
        }
        reply_confirms(&buf, self.id)
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

    #[test]
    fn da_terminated_detects_the_device_attributes_reply() {
        // The probe appends a DA request; its reply (`ESC [ ? ... c`) is the
        // "terminal has answered" signal so the read loop can stop.
        assert!(!da_terminated(b""), "empty buffer");
        assert!(
            !da_terminated(b"\x1b[?1;2"),
            "introducer but no terminator yet"
        );
        assert!(da_terminated(b"\x1b[?1;2c"), "full DA reply");
        // A kitty reply followed by the DA reply still terminates.
        assert!(da_terminated(b"\x1b_Gi=1;OK\x1b\\\x1b[?62;c"));
        // A bare 'c' with no DA introducer must not count.
        assert!(!da_terminated(b"a c in prose"), "no ESC [ ? introducer");
    }
}
