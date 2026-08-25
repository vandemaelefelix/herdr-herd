//! Terminal lifecycle: one guard that owns every global terminal mutation the
//! strip makes (raw mode, alternate screen, mouse capture, cursor visibility)
//! and undoes it on `Drop`, plus a panic hook that undoes it before the panic
//! message is printed.
//!
//! Why a guard rather than a straight-line teardown: `enable_raw_mode` acts on
//! `/dev/tty`, which crossterm opens independently of stdout, so an stdout
//! failure after that point used to return `Err` with the user's shell still
//! raw — no echo, no line editing, dead Ctrl-C until `stty sane`. `Drop` runs
//! on every path (`?`, panic, early return), so the terminal is always handed
//! back.

use std::io;
use std::sync::Once;

use crossterm::cursor::Show;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

/// The global terminal mutations the guard performs, behind a seam so tests can
/// watch it restore without a tty.
pub trait TerminalControl {
    fn enable_raw(&mut self) -> io::Result<()>;
    fn disable_raw(&mut self) -> io::Result<()>;
    /// Enter the alternate screen and start reporting mouse events.
    fn enter_screen(&mut self) -> io::Result<()>;
    /// Leave the alternate screen, stop reporting mouse events, show the cursor.
    fn leave_screen(&mut self) -> io::Result<()>;
}

/// Production impl: crossterm against the real terminal.
pub struct CrosstermControl;

impl TerminalControl for CrosstermControl {
    fn enable_raw(&mut self) -> io::Result<()> {
        enable_raw_mode()
    }

    fn disable_raw(&mut self) -> io::Result<()> {
        disable_raw_mode()
    }

    fn enter_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)
    }

    fn leave_screen(&mut self) -> io::Result<()> {
        execute!(
            io::stdout(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            Show
        )
    }
}

/// Owns whatever terminal state has been entered and gives it back exactly
/// once — on [`restore`](TerminalGuard::restore) or on `Drop`, whichever
/// happens first.
pub struct TerminalGuard<C: TerminalControl = CrosstermControl> {
    control: C,
    raw: bool,
    screen: bool,
}

impl TerminalGuard<CrosstermControl> {
    pub fn new() -> Self {
        Self::with_control(CrosstermControl)
    }
}

impl Default for TerminalGuard<CrosstermControl> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: TerminalControl> TerminalGuard<C> {
    /// Construct over an injected control seam (tests).
    pub fn with_control(control: C) -> Self {
        Self {
            control,
            raw: false,
            screen: false,
        }
    }

    /// Enable raw mode. Only marked for restoration if it actually took, so a
    /// failure here does not make the guard emit a spurious `disable_raw_mode`.
    pub fn enter_raw(&mut self) -> io::Result<()> {
        self.control.enable_raw()?;
        self.raw = true;
        Ok(())
    }

    /// Enter the alternate screen and enable mouse capture. Marked entered
    /// *before* the call: the two commands are applied in order, so a failure
    /// may still have switched the screen, and the guard has to undo that.
    pub fn enter_screen(&mut self) -> io::Result<()> {
        self.screen = true;
        self.control.enter_screen()
    }

    /// Give back whatever is still entered. Idempotent — the `Drop` that
    /// follows an explicit call does nothing.
    ///
    /// Raw mode goes first because it is the state that breaks the user's
    /// shell, and the screen is restored even when that fails: a half-restored
    /// terminal is exactly the outcome this guard exists to prevent.
    pub fn restore(&mut self) -> io::Result<()> {
        let mut result = Ok(());
        if std::mem::take(&mut self.raw) {
            result = result.and(self.control.disable_raw());
        }
        if std::mem::take(&mut self.screen) {
            result = result.and(self.control.leave_screen());
        }
        result
    }
}

impl<C: TerminalControl> Drop for TerminalGuard<C> {
    fn drop(&mut self) {
        // Best-effort: on the panic and `?` paths there is no caller left to
        // hand an error to, and failing to restore is not worth a double panic.
        let _ = self.restore();
    }
}

/// Undo every terminal mutation the strip can make, ignoring errors.
///
/// Used from the panic hook, where the guard's own state is out of reach, so
/// each command is issued unconditionally — all of them are no-ops when the
/// state was never entered.
pub fn restore_terminal_best_effort() {
    let _ = disable_raw_mode();
    let out = io::stdout();
    // Deliberately NO kitty image cleanup here. The only delete reachable
    // without the renderer's state is the terminal-global `a=d,d=A`, and that
    // is exactly issue #28: every strip pane forwards into one outer terminal,
    // so a global delete issued by this pane blanks every *other* pane's sheep
    // permanently. `KittyRenderer::teardown` frees this pane's own id block on
    // the normal exit path; on the panic path the images are simply left, which
    // is cosmetic and clears on the next redraw. Pane-scoped cleanup here would
    // need the live id set reachable from the hook — tracked separately.
    let _ = execute!(&out, LeaveAlternateScreen, DisableMouseCapture, Show);
}

/// Install a process-wide panic hook that hands the terminal back *before* the
/// default hook prints the message. Without it the message is written into the
/// alternate screen and disappears along with it, and a panic anywhere outside
/// a guard's reach leaves the shell raw.
///
/// Chains to the previously installed hook. Safe to call repeatedly; only the
/// first call installs.
pub fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal_best_effort();
            previous(info);
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{self, AssertUnwindSafe};
    use std::sync::{Arc, Mutex};

    /// Records the calls the guard makes instead of touching a terminal, and
    /// can be told to fail a given step.
    struct Recorder {
        calls: Arc<Mutex<Vec<&'static str>>>,
        failing: Option<&'static str>,
    }

    impl Recorder {
        fn new(calls: &Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                calls: Arc::clone(calls),
                failing: None,
            }
        }

        fn failing_at(calls: &Arc<Mutex<Vec<&'static str>>>, step: &'static str) -> Self {
            Self {
                calls: Arc::clone(calls),
                failing: Some(step),
            }
        }

        fn record(&self, step: &'static str) -> io::Result<()> {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push(step);
            }
            if self.failing == Some(step) {
                return Err(io::Error::other(step));
            }
            Ok(())
        }
    }

    impl TerminalControl for Recorder {
        fn enable_raw(&mut self) -> io::Result<()> {
            self.record("enable_raw")
        }

        fn disable_raw(&mut self) -> io::Result<()> {
            self.record("disable_raw")
        }

        fn enter_screen(&mut self) -> io::Result<()> {
            self.record("enter_screen")
        }

        fn leave_screen(&mut self) -> io::Result<()> {
            self.record("leave_screen")
        }
    }

    fn log() -> Arc<Mutex<Vec<&'static str>>> {
        Arc::new(Mutex::new(Vec::new()))
    }

    fn calls(log: &Arc<Mutex<Vec<&'static str>>>) -> Vec<&'static str> {
        log.lock()
            .expect("recorder mutex is never poisoned")
            .clone()
    }

    #[test]
    fn guard_restores_only_what_was_entered() {
        let log = log();
        {
            let mut guard = TerminalGuard::with_control(Recorder::new(&log));
            guard.enter_raw().expect("recorder accepts enable_raw");
        }
        assert_eq!(calls(&log), ["enable_raw", "disable_raw"]);
    }

    #[test]
    fn guard_restores_raw_mode_when_entering_the_alternate_screen_fails() {
        // Issue #35's repro: `herdr-herd render > /dev/full`. The stdout write
        // fails after raw mode is already on, and the shell must still come
        // back usable.
        let log = log();
        let mut guard = TerminalGuard::with_control(Recorder::failing_at(&log, "enter_screen"));
        guard.enter_raw().expect("recorder accepts enable_raw");
        assert!(guard.enter_screen().is_err());
        drop(guard);
        assert_eq!(
            calls(&log),
            ["enable_raw", "enter_screen", "disable_raw", "leave_screen"]
        );
    }

    #[test]
    fn guard_restores_the_screen_even_when_leaving_raw_mode_fails() {
        let log = log();
        let mut guard = TerminalGuard::with_control(Recorder::failing_at(&log, "disable_raw"));
        guard.enter_raw().expect("recorder accepts enable_raw");
        guard.enter_screen().expect("recorder accepts enter_screen");
        assert!(guard.restore().is_err(), "the failure is reported");
        assert_eq!(
            calls(&log),
            [
                "enable_raw",
                "enter_screen",
                "disable_raw",
                "leave_screen" // not skipped by the earlier failure
            ]
        );
    }

    #[test]
    fn guard_restores_on_unwind() {
        let log = log();
        let hook = panic::take_hook();
        panic::set_hook(Box::new(|_| {})); // the panic below is expected; stay quiet
        let unwound = panic::catch_unwind(AssertUnwindSafe(|| {
            let mut guard = TerminalGuard::with_control(Recorder::new(&log));
            guard.enter_raw().expect("recorder accepts enable_raw");
            guard.enter_screen().expect("recorder accepts enter_screen");
            panic!("the render loop blew up");
        }))
        .is_err();
        panic::set_hook(hook);
        assert!(unwound, "the panic propagated");
        assert_eq!(
            calls(&log),
            ["enable_raw", "enter_screen", "disable_raw", "leave_screen"]
        );
    }

    #[test]
    fn restoring_twice_restores_once() {
        let log = log();
        let mut guard = TerminalGuard::with_control(Recorder::new(&log));
        guard.enter_raw().expect("recorder accepts enable_raw");
        guard.restore().expect("recorder accepts disable_raw");
        drop(guard);
        assert_eq!(calls(&log), ["enable_raw", "disable_raw"]);
    }

    #[test]
    fn a_guard_that_entered_nothing_restores_nothing() {
        let log = log();
        drop(TerminalGuard::with_control(Recorder::new(&log)));
        assert!(calls(&log).is_empty());
    }

    #[test]
    fn failing_to_enable_raw_mode_leaves_nothing_to_restore() {
        let log = log();
        let mut guard = TerminalGuard::with_control(Recorder::failing_at(&log, "enable_raw"));
        assert!(guard.enter_raw().is_err());
        drop(guard);
        assert_eq!(calls(&log), ["enable_raw"], "no spurious disable_raw");
    }
}
