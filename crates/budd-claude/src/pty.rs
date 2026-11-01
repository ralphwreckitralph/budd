//! `claude attach` in a hidden pseudo-terminal: wait for the prompt, paste, press Return, close.
//!
//! Claude Code has no command that puts a message into a running background session, so Budd does what
//! a person would: it opens `claude attach <short id>` in a terminal nobody sees, waits for the input
//! prompt, pastes the message, presses Return and closes the client again; the session keeps running.
//!
//! The terminal is a real pty (openpty on Unix, ConPTY on Windows) through `portable-pty`, and what the
//! client draws is fed into a `vt100` screen model rather than searched as raw bytes: ConPTY rewrites
//! the client's output into its own escape sequences, so only the rendered screen is the same on every
//! platform. A reader thread pumps the pty the whole time, so the client never blocks on a full output
//! buffer, not even while it exits.

use crate::claude_cli::{Claude, child_env};
use portable_pty::{Child, CommandBuilder, MasterPty, PtyPair, PtySize, native_pty_system};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

/// The prompt glyph Claude Code draws at the start of its input box.
pub const PROMPT: char = '\u{276F}';
/// The terminal size the client is given: 40 rows by 120 columns, wide enough that nothing wraps.
pub const DEFAULT_SIZE: (u16, u16) = (40, 120);
/// How much of the client's raw output is kept for diagnostics.
const RAW_TAIL: usize = 64 * 1024;
/// How often the screen is looked at while waiting.
const POLL: Duration = Duration::from_millis(50);
/// How many characters of the client's last line go into an error.
const LAST_LINE: usize = 100;
/// How long the reader gets to catch up after the client exited.
const EXIT_GRACE: Duration = Duration::from_millis(500);

/// The prompt did not appear in time: what the client last said, for the error the human sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitError {
    /// The last non-empty line on the client's screen, control codes stripped, at most 100 characters.
    pub last_line: Option<String>,
    /// Whether the client exited before the prompt appeared (as it does for an unknown session) rather
    /// than outstaying the timeout.
    pub exited: bool,
}

impl std::fmt::Display for WaitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the session's prompt never appeared")?;
        if let Some(line) = &self.last_line {
            write!(f, ": claude said \"{line}\"")?;
        }
        Ok(())
    }
}

impl std::error::Error for WaitError {}

/// What the client has drawn so far: the screen model, the raw tail, and whether the pty has closed.
struct Screen {
    parser: vt100::Parser,
    raw: Vec<u8>,
    eof: bool,
}

impl Screen {
    fn new(rows: u16, cols: u16) -> Screen {
        Screen {
            parser: vt100::Parser::new(rows, cols, 0),
            raw: Vec::new(),
            eof: false,
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        self.raw.extend_from_slice(bytes);
        if self.raw.len() > RAW_TAIL {
            let excess = self.raw.len() - RAW_TAIL;
            self.raw.drain(..excess);
        }
    }

    /// The visible rows, top to bottom, trailing blanks trimmed.
    fn rows(&self) -> Vec<String> {
        let screen = self.parser.screen();
        let (_, cols) = screen.size();
        screen.rows(0, cols).map(|row| row.trim_end().to_owned()).collect()
    }

    /// Whether the prompt is on screen: a row that starts with the glyph. Claude Code draws a hint line
    /// under its input box, so the prompt is seldom the very last non-empty row.
    fn prompt_shown(&self) -> bool {
        self.rows().iter().any(|row| row.trim_start().starts_with(PROMPT))
    }

    /// The last non-empty line the client printed, from the screen, or from the raw tail with its
    /// escape sequences taken out when the screen is blank.
    fn last_line(&self) -> Option<String> {
        let rows = self.rows();
        let from_screen = rows.iter().map(|row| row.trim()).rfind(|row| !row.is_empty());
        let line = match from_screen {
            Some(row) => Some(row.to_owned()),
            None => {
                let text = plain_text(&self.raw);
                text.lines()
                    .map(str::trim)
                    .rfind(|line| !line.is_empty())
                    .map(str::to_owned)
            }
        };
        line.map(|line| line.chars().take(LAST_LINE).collect())
    }
}

/// `bytes` as text with every escape sequence replaced by a line break, as the Swift `lastLine` had it,
/// so what is left splits into the lines the client meant.
fn plain_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1B}' {
            out.push(c);
            continue;
        }
        out.push('\n');
        match chars.peek() {
            // CSI: parameters and intermediates, then one final byte in 0x40..=0x7E.
            Some('[') => {
                chars.next();
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7E}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: up to BEL or ESC \.
            Some(']') => {
                chars.next();
                let mut previous = ' ';
                for c in chars.by_ref() {
                    if c == '\u{07}' || (previous == '\u{1B}' && c == '\\') {
                        break;
                    }
                    previous = c;
                }
            }
            // Anything else: intermediates in 0x20..=0x2F, then one final byte.
            Some(_) => {
                while chars.next_if(|c| ('\u{20}'..='\u{2F}').contains(c)).is_some() {}
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// `claude attach` running in a pty of its own.
pub struct Attach {
    child: Box<dyn Child + Send + Sync>,
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Box<dyn Write + Send>>,
    screen: Arc<Mutex<Screen>>,
    closed: bool,
}

impl Attach {
    /// Starts `claude attach short_id` in a new pty of `size` (rows, columns) with the clean environment
    /// from [`child_env`], `TERM` forced to `xterm-256color` so the client draws what the screen model
    /// reads, and begins reading its output.
    pub fn spawn(claude: &Claude, short_id: &str, size: (u16, u16)) -> io::Result<Attach> {
        let (rows, cols) = size;
        let PtyPair { master, slave } = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(io::Error::other)?;
        // The child runs in another folder, so a relative binary must be made absolute here.
        let exe = std::path::absolute(&claude.exe).unwrap_or_else(|_| claude.exe.clone());
        let mut command = CommandBuilder::new(&exe);
        command.args(["attach", short_id]);
        command.env_clear();
        for (name, value) in child_env(&exe_dir(&exe), &claude.config_dir).vars {
            command.env(name, value);
        }
        command.env("TERM", "xterm-256color");
        command.cwd(std::env::temp_dir());
        let child = slave.spawn_command(command).map_err(io::Error::other)?;
        // The child holds its own end now; keeping ours would hide its exit from the reader.
        drop(slave);
        let mut reader = master.try_clone_reader().map_err(io::Error::other)?;
        let writer = master.take_writer().map_err(io::Error::other)?;
        let screen = Arc::new(Mutex::new(Screen::new(rows, cols)));
        let sink = Arc::clone(&screen);
        thread::Builder::new()
            .name("budd-pty-reader".to_owned())
            .spawn(move || {
                let mut buffer = [0u8; 16 * 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(count) => lock(&sink).feed(&buffer[..count]),
                    }
                }
                lock(&sink).eof = true;
            })?;
        Ok(Attach {
            child,
            master: Some(master),
            writer: Some(writer),
            screen,
            closed: false,
        })
    }

    /// Waits until the prompt is on screen, looking every 50 ms, up to `timeout`. A client that exits
    /// first (an unknown session, a refused attach) fails at once rather than at the timeout.
    pub fn wait_prompt(&mut self, timeout: Duration) -> Result<(), WaitError> {
        let deadline = Instant::now() + timeout;
        loop {
            if lock(&self.screen).prompt_shown() {
                return Ok(());
            }
            if self.exited() || lock(&self.screen).eof {
                let grace = Instant::now() + EXIT_GRACE;
                while Instant::now() < grace && !lock(&self.screen).eof {
                    thread::sleep(POLL);
                }
                let screen = lock(&self.screen);
                if screen.prompt_shown() {
                    return Ok(());
                }
                return Err(WaitError {
                    last_line: screen.last_line(),
                    exited: true,
                });
            }
            if Instant::now() >= deadline {
                return Err(WaitError {
                    last_line: lock(&self.screen).last_line(),
                    exited: false,
                });
            }
            thread::sleep(POLL);
        }
    }

    /// Gives the client `duration` to draw; the reader thread keeps pumping meanwhile.
    pub fn drain(&self, duration: Duration) {
        thread::sleep(duration);
    }

    /// Pastes `text` as one bracketed paste, so a line break inside it stays in the message instead of
    /// acting as Return. `text` must not contain an escape character, which would end the paste early;
    /// `budd_core::delivery::sendable` takes them out.
    pub fn paste(&mut self, text: &str) -> io::Result<()> {
        self.write(format!("\u{1B}[200~{text}\u{1B}[201~").as_bytes())
    }

    /// Presses Return.
    pub fn enter(&mut self) -> io::Result<()> {
        self.write(b"\r")
    }

    /// The visible screen as text, for diagnostics and tests.
    pub fn screen_text(&self) -> String {
        lock(&self.screen).rows().join("\n")
    }

    /// The last non-empty line on the client's screen, control codes stripped, at most 100 characters.
    pub fn last_line(&self) -> Option<String> {
        lock(&self.screen).last_line()
    }

    /// Whether the client has exited.
    pub fn exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Ends the client: a polite termination first, with its output pumped while it exits so it never
    /// blocks on the terminal, then the terminal hung up, and only then a hard kill. Closing twice is
    /// harmless.
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.terminate();
        let mut reaped = false;
        for _ in 0..20 {
            thread::sleep(POLL);
            if self.exited() {
                reaped = true;
                break;
            }
        }
        // Dropping the writer sends EOF and dropping the master hangs the terminal up, which frees a
        // client still stuck on it.
        self.writer.take();
        self.master.take();
        if reaped {
            return;
        }
        thread::sleep(Duration::from_millis(100));
        if !self.exited() {
            self.kill();
            let _ = self.child.wait();
        }
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let Some(writer) = self.writer.as_mut() else {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "the terminal is closed"));
        };
        writer.write_all(bytes)?;
        writer.flush()
    }

    /// Asks the client to end: SIGTERM on Unix, where Claude Code detaches cleanly on it; Windows has
    /// no signal, so the process is terminated.
    fn terminate(&mut self) {
        #[cfg(unix)]
        {
            if let Some(pid) = self.child.process_id() {
                signal(pid, libc::SIGTERM);
                return;
            }
        }
        let _ = self.child.kill();
    }

    /// Ends the client for certain.
    fn kill(&mut self) {
        #[cfg(unix)]
        {
            if let Some(pid) = self.child.process_id() {
                signal(pid, libc::SIGKILL);
                return;
            }
        }
        let _ = self.child.kill();
    }
}

impl Drop for Attach {
    fn drop(&mut self) {
        self.close();
    }
}

/// Sends `signal` to the child `pid`, which this handle owns and has not reaped.
#[cfg(unix)]
fn signal(pid: u32, signal: libc::c_int) {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return;
    };
    // SAFETY: kill takes a pid and a signal number and touches no memory. The pid is the child this
    // handle owns and has not yet reaped, so it cannot have been recycled for another process.
    unsafe {
        libc::kill(pid, signal);
    }
}

/// The folder the binary is in, which heads the child's `PATH`.
fn exe_dir(exe: &Path) -> std::path::PathBuf {
    exe.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// The screen behind its mutex; a reader thread that panicked mid-write leaves the screen usable.
fn lock(screen: &Mutex<Screen>) -> MutexGuard<'_, Screen> {
    screen.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_is_a_row_starting_with_the_glyph() {
        let mut screen = Screen::new(10, 40);
        screen.feed(b"Welcome back\r\n\r\n");
        assert!(!screen.prompt_shown());
        screen.feed("\u{276F} \r\n  ? for shortcuts".as_bytes());
        assert!(screen.prompt_shown());
        assert_eq!(screen.last_line().as_deref(), Some("? for shortcuts"));
    }

    #[test]
    fn a_prompt_mentioned_mid_line_does_not_count() {
        let mut screen = Screen::new(10, 40);
        screen.feed("type at the \u{276F} prompt".as_bytes());
        assert!(!screen.prompt_shown());
    }

    #[test]
    fn the_last_line_comes_without_control_codes_and_cut_to_100() {
        let mut screen = Screen::new(5, 200);
        screen.feed(b"\x1b[?25l\x1b[1mError:\x1b[0m No session found matching abc\x1b[K\r\n");
        assert_eq!(
            screen.last_line().as_deref(),
            Some("Error: No session found matching abc")
        );
        let long = "x".repeat(150);
        screen.feed(format!("\r\n{long}").as_bytes());
        assert_eq!(screen.last_line().map(|l| l.chars().count()), Some(100));
        assert_eq!(Screen::new(5, 20).last_line(), None);
    }

    #[test]
    fn the_raw_tail_is_the_fallback_and_stays_bounded() {
        let mut screen = Screen::new(2, 20);
        // A title change and a line that is then wiped off the screen: only the raw tail remembers it.
        screen.feed(b"\x1b]0;claude\x07connecting\x1b[2J\x1b[H");
        assert_eq!(screen.last_line().as_deref(), Some("connecting"));
        screen.feed(&vec![b'a'; RAW_TAIL + 5000]);
        assert_eq!(screen.raw.len(), RAW_TAIL);
    }

    #[test]
    fn plain_text_splits_on_every_escape_sequence() {
        assert_eq!(plain_text(b"a\x1b[31mb\x1b]2;t\x1b\\c\x1b(Bd"), "a\nb\nc\nd");
    }

    #[test]
    fn the_error_reads_as_the_swift_one_did() {
        let with = WaitError {
            last_line: Some("No session found".to_owned()),
            exited: true,
        };
        assert_eq!(
            with.to_string(),
            "the session's prompt never appeared: claude said \"No session found\""
        );
        let without = WaitError {
            last_line: None,
            exited: false,
        };
        assert_eq!(without.to_string(), "the session's prompt never appeared");
    }
}
