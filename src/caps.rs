//! Runtime detection of whether the outer terminal (through herdr) will render
//! kitty graphics. Self-correcting: if herdr's experimental flag is off, the
//! query is swallowed and no reply arrives, so we report unsupported and the
//! caller falls back to half-blocks. The tty I/O lives behind a trait so tests
//! never touch a real terminal, and the probe itself is bounded from the
//! outside so a terminal that never answers costs a few hundred milliseconds
//! rather than the whole process.

use std::io::{self, Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Extra time allowed on top of the probe's own read budget before the caller
/// stops waiting for the worker thread. Covers thread spawn and the last read
/// returning just as the deadline passes.
const JOIN_GRACE: Duration = Duration::from_millis(100);

/// The image id the probe transmits and looks for in the reply. Arbitrary, but
/// high enough to be unlikely to collide with anything else on the terminal.
const PROBE_IMAGE_ID: u32 = 0x7E51;

/// Whether the current terminal supports the kitty graphics protocol.
///
/// Note: we intentionally do NOT expose the terminal cell size here. herdr
/// swallows the `CSI 14 t`/`CSI 16 t` pixel-size queries (only the DA reply
/// comes back) and its API reports no pixel dimensions, so the cell size is
/// unobtainable through herdr. The kitty backend instead places images with an
/// explicit cell footprint (`c=`/`r=`), which needs no cell-size query.
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
/// stop reading" and lets the probe finish early when kitty is unsupported
/// (only the DA reply comes back). Pure; unit-tested.
pub fn da_terminated(buf: &[u8]) -> bool {
    match buf.windows(3).position(|w| w == b"\x1b[?") {
        Some(pos) => buf[pos + 3..].contains(&b'c'),
        None => false,
    }
}

/// The tty round-trip the probe needs, behind a seam so tests can supply a
/// terminal that answers, answers partially, or never answers at all.
///
/// `read` is allowed — expected, even — to block indefinitely: under raw mode
/// (`VMIN=1, VTIME=0`) that is exactly what a real stdin does when no bytes
/// arrive. Callers must therefore run implementations on a thread they are
/// willing to abandon.
pub trait ProbeIo {
    /// Write the probe bytes to the terminal and flush them.
    fn write_query(&mut self, query: &[u8]) -> io::Result<()>;
    /// Read the next chunk of the terminal's reply. May block forever.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
}

/// Production seam: query out on stdout, reply in on stdin.
///
/// We deliberately do NOT use `crossterm::event::poll` here: it drains the
/// ready bytes into crossterm's own parser buffer, which would leave a
/// subsequent raw `read` with nothing to read. This runs once at startup,
/// before the event loop, with raw mode already enabled by the caller, so
/// reading stdin directly is safe.
pub struct TtyProbeIo;

impl ProbeIo for TtyProbeIo {
    fn write_query(&mut self, query: &[u8]) -> io::Result<()> {
        let mut out = io::stdout();
        out.write_all(query)?;
        out.flush()
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        io::stdin().read(buf)
    }
}

/// Write the kitty query followed by a Primary Device Attributes request
/// (`ESC [ c`) and read until the DA reply terminates the response, `timeout`
/// elapses, or the stream ends; true if image `id` was confirmed.
///
/// The deadline can only be checked *between* reads, so this function may
/// outlive `timeout` by however long a blocking `read` takes to return. That
/// is why it is never called directly — see
/// [`supports_kitty_graphics`](TerminalCaps::supports_kitty_graphics).
fn probe<I: ProbeIo>(io: &mut I, id: u32, timeout: Duration) -> bool {
    // Every terminal answers DA, so appending it guarantees a reply even when
    // the kitty query itself elicits none.
    let mut query = crate::kitty::probe_query(id).into_bytes();
    query.extend_from_slice(b"\x1b[c");
    if io.write_query(&query).is_err() {
        return false;
    }
    let deadline = Instant::now() + timeout;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 256];
    while Instant::now() < deadline {
        match io.read(&mut chunk) {
            Ok(0) | Err(_) => break, // EOF or read error: stop
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if da_terminated(&buf) {
                    break; // terminal has answered; nothing more is coming
                }
            }
        }
    }
    reply_confirms(&buf, id)
}

/// Probes the real tty. Assumes raw mode is already enabled by the caller
/// (the render loop enables it before selecting a renderer).
pub struct RealCaps<I: ProbeIo = TtyProbeIo> {
    id: u32,
    timeout: Duration,
    /// Taken by the first probe and never returned: the worker thread that owns
    /// it may still be parked in `read`, so it cannot be handed back.
    io: Option<I>,
}

impl RealCaps<TtyProbeIo> {
    pub fn new() -> Self {
        Self::with_io(TtyProbeIo, Duration::from_millis(150))
    }
}

impl Default for RealCaps<TtyProbeIo> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I: ProbeIo> RealCaps<I> {
    /// Construct over an injected tty seam with an explicit read budget.
    pub fn with_io(io: I, timeout: Duration) -> Self {
        Self {
            id: PROBE_IMAGE_ID,
            timeout,
            io: Some(io),
        }
    }
}

impl<I: ProbeIo + Send + 'static> TerminalCaps for RealCaps<I> {
    /// Bounded from the outside, because it cannot be bounded from the inside:
    /// under raw mode `read` blocks indefinitely, so the probe's own deadline
    /// never fires if the reply is lost (herdr forwarding, a nested
    /// multiplexer, redirected stdin). Running it on a worker thread and
    /// waiting with `recv_timeout` puts a hard ceiling on the cost.
    ///
    /// Unknown means unsupported: we fall back to half-blocks, which GOAL.md
    /// names the universal baseline. A blank strip is never the answer.
    fn supports_kitty_graphics(&mut self) -> bool {
        let Some(mut io) = self.io.take() else {
            return false; // seam already consumed by an earlier probe
        };
        let (id, timeout) = (self.id, self.timeout);
        let (tx, rx) = mpsc::channel();
        // Detached on purpose: an abandoned worker stays parked in `read` until
        // one more byte arrives, then sees the deadline has passed and exits.
        // It can swallow at most that one chunk of input before the event loop
        // starts, which is the price of not hanging the strip forever.
        thread::spawn(move || {
            let _ = tx.send(probe(&mut io, id, timeout));
        });
        rx.recv_timeout(timeout + JOIN_GRACE).unwrap_or(false)
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

    /// A tty that accepts the write and then never says anything — herdr
    /// swallowing the reply, a nested multiplexer eating it, stdin redirected
    /// from something that never produces bytes. This is issue #27's terminal.
    struct SilentIo;

    impl ProbeIo for SilentIo {
        fn write_query(&mut self, _query: &[u8]) -> io::Result<()> {
            Ok(())
        }

        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            // Block like a raw-mode stdin with no bytes available. `park` can
            // return spuriously, hence the loop; nothing ever unparks us.
            loop {
                thread::park();
            }
        }
    }

    /// A tty that answers once with `reply`, then goes silent like [`SilentIo`].
    struct ScriptedIo {
        reply: Vec<u8>,
        answered: bool,
    }

    impl ProbeIo for ScriptedIo {
        fn write_query(&mut self, _query: &[u8]) -> io::Result<()> {
            Ok(())
        }

        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.answered {
                loop {
                    thread::park();
                }
            }
            self.answered = true;
            let n = self.reply.len().min(buf.len());
            buf[..n].copy_from_slice(&self.reply[..n]);
            Ok(n)
        }
    }

    /// A tty whose write fails, e.g. stdout redirected to a full device.
    struct UnwritableIo;

    impl ProbeIo for UnwritableIo {
        fn write_query(&mut self, _query: &[u8]) -> io::Result<()> {
            Err(io::Error::other("stdout is gone"))
        }

        fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
            unreachable!("no read happens after a failed write")
        }
    }

    /// Run the probe on its own thread and fail — rather than wedge
    /// `cargo test` — if it has not answered well after its budget. A
    /// regression of the #27 hang must show up as a red test.
    fn probe_within<I: ProbeIo + Send + 'static>(mut caps: RealCaps<I>) -> bool {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(caps.supports_kitty_graphics());
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("probe never returned; the bounded wait has regressed")
    }

    fn silent_caps() -> RealCaps<SilentIo> {
        RealCaps::with_io(SilentIo, Duration::from_millis(50))
    }

    fn scripted_caps(reply: &[u8]) -> RealCaps<ScriptedIo> {
        RealCaps::with_io(
            ScriptedIo {
                reply: reply.to_vec(),
                answered: false,
            },
            Duration::from_millis(50),
        )
    }

    #[test]
    fn probe_reports_unsupported_when_the_terminal_never_answers() {
        // Issue #27: the read blocks forever, so the probe's own deadline can
        // never fire. The caller must give up and degrade to half-blocks.
        let started = Instant::now();
        assert!(!probe_within(silent_caps()));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "gave up in {:?}, which is not a bounded wait",
            started.elapsed()
        );
    }

    #[test]
    fn probe_confirms_support_when_the_kitty_reply_arrives() {
        let reply = format!("\x1b_Gi={PROBE_IMAGE_ID};OK\x1b\\\x1b[?62;c");
        assert!(probe_within(scripted_caps(reply.as_bytes())));
    }

    #[test]
    fn probe_reports_unsupported_when_only_the_device_attributes_reply_arrives() {
        // herdr's experimental kitty flag off: the query is swallowed, DA still
        // comes back, and the DA terminator lets the probe finish immediately.
        assert!(!probe_within(scripted_caps(b"\x1b[?62;c")));
    }

    #[test]
    fn probe_reports_unsupported_when_the_query_cannot_be_written() {
        assert!(!probe_within(RealCaps::with_io(
            UnwritableIo,
            Duration::from_millis(50)
        )));
    }

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
