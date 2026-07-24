use std::io::{self, Stdout, Write};

use anyhow::{Context, Result};
use crossterm::cursor::{Hide, MoveToColumn, RestorePosition, SavePosition, Show};
use crossterm::execute;
use crossterm::terminal::{
    Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

pub(crate) trait TerminalControl: Write {
    fn enable_raw(&mut self) -> io::Result<()>;
    fn disable_raw(&mut self) -> io::Result<()>;
    fn enter_alternate(&mut self) -> io::Result<()>;
    fn leave_alternate(&mut self) -> io::Result<()>;
    fn hide_cursor(&mut self) -> io::Result<()>;
    fn show_cursor(&mut self) -> io::Result<()>;
    fn save_position(&mut self) -> io::Result<()>;
    fn restore_position(&mut self) -> io::Result<()>;
    fn prepare_inline_redraw(&mut self) -> io::Result<()>;
    fn clear_from_cursor_down(&mut self) -> io::Result<()>;
    fn write_newline(&mut self) -> io::Result<()>;
}

pub(crate) struct CrosstermControl<W: Write> {
    writer: W,
}

impl CrosstermControl<Stdout> {
    pub(crate) fn stdout() -> Self {
        Self {
            writer: std::io::stdout(),
        }
    }
}

impl<W: Write> Write for CrosstermControl<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl<W: Write> TerminalControl for CrosstermControl<W> {
    fn enable_raw(&mut self) -> io::Result<()> {
        enable_raw_mode()
    }

    fn disable_raw(&mut self) -> io::Result<()> {
        disable_raw_mode()
    }

    fn enter_alternate(&mut self) -> io::Result<()> {
        execute!(&mut self.writer, EnterAlternateScreen)
    }

    fn leave_alternate(&mut self) -> io::Result<()> {
        execute!(&mut self.writer, LeaveAlternateScreen)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        execute!(&mut self.writer, Hide)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        execute!(&mut self.writer, Show)
    }

    fn save_position(&mut self) -> io::Result<()> {
        execute!(&mut self.writer, SavePosition)
    }

    fn restore_position(&mut self) -> io::Result<()> {
        execute!(&mut self.writer, RestorePosition)
    }

    fn prepare_inline_redraw(&mut self) -> io::Result<()> {
        execute!(
            &mut self.writer,
            RestorePosition,
            MoveToColumn(0),
            Clear(ClearType::FromCursorDown)
        )
    }

    fn clear_from_cursor_down(&mut self) -> io::Result<()> {
        execute!(&mut self.writer, Clear(ClearType::FromCursorDown))
    }

    fn write_newline(&mut self) -> io::Result<()> {
        writeln!(&mut self.writer)?;
        self.writer.flush()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalMode {
    Fullscreen,
    Inline,
}

pub(crate) struct TerminalSession<C: TerminalControl> {
    control: C,
    mode: TerminalMode,
    cleanup_pending: bool,
    raw_maybe_enabled: bool,
    alternate_maybe_entered: bool,
    cursor_maybe_hidden: bool,
    position_maybe_saved: bool,
    inline_area_cleared: bool,
    inline_newline_written: bool,
}

impl<C: TerminalControl> TerminalSession<C> {
    pub(crate) fn new(control: C, mode: TerminalMode) -> Self {
        Self {
            control,
            mode,
            cleanup_pending: false,
            raw_maybe_enabled: false,
            alternate_maybe_entered: false,
            cursor_maybe_hidden: false,
            position_maybe_saved: false,
            inline_area_cleared: false,
            inline_newline_written: false,
        }
    }

    pub(crate) fn enable_raw(&mut self) -> io::Result<()> {
        self.cleanup_pending = true;
        self.raw_maybe_enabled = true;
        self.cursor_maybe_hidden = true;
        self.control.enable_raw()
    }

    pub(crate) fn enter_alternate(&mut self) -> io::Result<()> {
        self.cleanup_pending = true;
        self.alternate_maybe_entered = true;
        self.control.enter_alternate()
    }

    pub(crate) fn hide_cursor(&mut self) -> io::Result<()> {
        self.cleanup_pending = true;
        self.cursor_maybe_hidden = true;
        self.control.hide_cursor()
    }

    pub(crate) fn save_position(&mut self) -> io::Result<()> {
        self.cleanup_pending = true;
        self.position_maybe_saved = true;
        self.control.save_position()
    }

    pub(crate) fn prepare_inline_redraw(&mut self) -> io::Result<()> {
        self.control.prepare_inline_redraw()
    }

    pub(crate) fn control_mut(&mut self) -> &mut C {
        &mut self.control
    }

    fn restore(&mut self) -> io::Result<()> {
        if !self.cleanup_pending {
            return Ok(());
        }

        let mut errors = Vec::new();

        if self.mode == TerminalMode::Fullscreen && self.alternate_maybe_entered {
            match self.control.leave_alternate() {
                Ok(()) => self.alternate_maybe_entered = false,
                Err(err) => errors.push(format!("leave alternate screen: {err}")),
            }
        }

        if self.mode == TerminalMode::Inline && self.position_maybe_saved {
            match self.control.restore_position() {
                Ok(()) => self.position_maybe_saved = false,
                Err(err) => errors.push(format!("restore cursor position: {err}")),
            }
        }

        if self.cursor_maybe_hidden {
            match self.control.show_cursor() {
                Ok(()) => self.cursor_maybe_hidden = false,
                Err(err) => errors.push(format!("show cursor: {err}")),
            }
        }

        if self.mode == TerminalMode::Inline && !self.inline_area_cleared {
            match self.control.clear_from_cursor_down() {
                Ok(()) => self.inline_area_cleared = true,
                Err(err) => errors.push(format!("clear inline prompt: {err}")),
            }
        }

        if self.mode == TerminalMode::Inline && !self.inline_newline_written {
            match self.control.write_newline() {
                Ok(()) => self.inline_newline_written = true,
                Err(err) => errors.push(format!("finish inline prompt line: {err}")),
            }
        }

        if self.raw_maybe_enabled {
            match self.control.disable_raw() {
                Ok(()) => self.raw_maybe_enabled = false,
                Err(err) => errors.push(format!("disable raw mode: {err}")),
            }
        }

        let inline_clean = self.mode == TerminalMode::Fullscreen
            || (self.inline_area_cleared && self.inline_newline_written);
        if !self.raw_maybe_enabled
            && !self.alternate_maybe_entered
            && !self.cursor_maybe_hidden
            && !self.position_maybe_saved
            && inline_clean
        {
            self.cleanup_pending = false;
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(errors.join("; ")))
        }
    }

    pub(crate) fn finish<T>(mut self, body: Result<T>) -> Result<T> {
        let cleanup = self
            .restore()
            .context("failed to restore terminal state after interactive session");
        match (body, cleanup) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(err), Ok(())) => Err(err),
            (Ok(_), Err(cleanup_err)) => Err(cleanup_err),
            (Err(err), Err(cleanup_err)) => {
                Err(err.context(format!("terminal cleanup also failed: {cleanup_err:#}")))
            }
        }
    }
}

impl<C: TerminalControl> Drop for TerminalSession<C> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io::{self, Write};
    use std::rc::Rc;

    use anyhow::{Result, anyhow};

    use super::{TerminalControl, TerminalMode, TerminalSession};

    #[derive(Clone, Default)]
    struct Calls(Rc<RefCell<Vec<&'static str>>>);

    impl Calls {
        fn record(&self, action: &'static str) {
            self.0.borrow_mut().push(action);
        }

        fn get(&self) -> Vec<&'static str> {
            self.0.borrow().clone()
        }
    }

    struct MockControl {
        calls: Calls,
        fail_once: Option<&'static str>,
        failed: bool,
    }

    impl MockControl {
        fn new(calls: Calls, fail_once: Option<&'static str>) -> Self {
            Self {
                calls,
                fail_once,
                failed: false,
            }
        }

        fn action(&mut self, name: &'static str) -> io::Result<()> {
            self.calls.record(name);
            if self.fail_once == Some(name) && !self.failed {
                self.failed = true;
                return Err(io::Error::other(format!("{name} failed")));
            }
            Ok(())
        }
    }

    impl Write for MockControl {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl TerminalControl for MockControl {
        fn enable_raw(&mut self) -> io::Result<()> {
            self.action("enable_raw")
        }

        fn disable_raw(&mut self) -> io::Result<()> {
            self.action("disable_raw")
        }

        fn enter_alternate(&mut self) -> io::Result<()> {
            self.action("enter_alternate")
        }

        fn leave_alternate(&mut self) -> io::Result<()> {
            self.action("leave_alternate")
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            self.action("hide_cursor")
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            self.action("show_cursor")
        }

        fn save_position(&mut self) -> io::Result<()> {
            self.action("save_position")
        }

        fn restore_position(&mut self) -> io::Result<()> {
            self.action("restore_position")
        }

        fn prepare_inline_redraw(&mut self) -> io::Result<()> {
            self.action("prepare_inline_redraw")
        }

        fn clear_from_cursor_down(&mut self) -> io::Result<()> {
            self.action("clear_from_cursor_down")
        }

        fn write_newline(&mut self) -> io::Result<()> {
            self.action("write_newline")
        }
    }

    #[test]
    fn fullscreen_partial_setup_failure_restores_every_possible_state() {
        let calls = Calls::default();
        let control = MockControl::new(calls.clone(), Some("enter_alternate"));
        let mut session = TerminalSession::new(control, TerminalMode::Fullscreen);
        let body = (|| -> Result<()> {
            session.enable_raw()?;
            session.enter_alternate()?;
            Ok(())
        })();

        assert!(session.finish(body).is_err());
        assert_eq!(
            calls.get(),
            [
                "enable_raw",
                "enter_alternate",
                "leave_alternate",
                "show_cursor",
                "disable_raw",
            ]
        );
    }

    #[test]
    fn fullscreen_body_error_is_preserved_after_complete_cleanup() {
        let calls = Calls::default();
        let control = MockControl::new(calls.clone(), None);
        let mut session = TerminalSession::new(control, TerminalMode::Fullscreen);
        session.enable_raw().expect("enable raw");
        session.enter_alternate().expect("enter alternate");

        let err = session
            .finish::<()>(Err(anyhow!("draw failed")))
            .expect_err("body error");
        assert!(err.to_string().contains("draw failed"));
        assert_eq!(
            calls.get(),
            [
                "enable_raw",
                "enter_alternate",
                "leave_alternate",
                "show_cursor",
                "disable_raw",
            ]
        );
    }

    #[test]
    fn inline_save_failure_restores_cursor_area_and_raw_mode() {
        let calls = Calls::default();
        let control = MockControl::new(calls.clone(), Some("save_position"));
        let mut session = TerminalSession::new(control, TerminalMode::Inline);
        let body = (|| -> Result<()> {
            session.enable_raw()?;
            session.hide_cursor()?;
            session.save_position()?;
            Ok(())
        })();

        assert!(session.finish(body).is_err());
        assert_eq!(
            calls.get(),
            [
                "enable_raw",
                "hide_cursor",
                "save_position",
                "restore_position",
                "show_cursor",
                "clear_from_cursor_down",
                "write_newline",
                "disable_raw",
            ]
        );
    }

    #[test]
    fn failed_cleanup_does_not_skip_later_actions_and_drop_retries_it() {
        let calls = Calls::default();
        let control = MockControl::new(calls.clone(), Some("leave_alternate"));
        let mut session = TerminalSession::new(control, TerminalMode::Fullscreen);
        session.enable_raw().expect("enable raw");
        session.enter_alternate().expect("enter alternate");

        assert!(session.finish(Ok(())).is_err());
        assert_eq!(
            calls.get(),
            [
                "enable_raw",
                "enter_alternate",
                "leave_alternate",
                "show_cursor",
                "disable_raw",
                "leave_alternate",
            ]
        );
    }
}
