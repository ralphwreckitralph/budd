//! Delivering a message to a session and proving it arrived; clearing a session; waking a retired one.
//!
//! The steps are the Swift `SessionMessenger`'s: note where the transcript ends, open `claude attach`
//! in a hidden pty, wait for the prompt (and, for a session asleep, until `claude agents` lists it awake
//! and idle, since the client draws its prompt before the daemon has the session back and a paste in
//! that gap is lost), paste the message, press Return, and then watch the transcript for it. Typing is
//! never trusted on its own: the message counts as delivered once it is recorded, as a `user` entry
//! when the session was idle or a queued `queue-operation` when it was busy. A clear writes nothing to
//! the old transcript, so it counts once the listing shows the short id with a new session id.
//!
//! Pictures go in as their paths after the text, as a terminal pastes a dragged file, and Claude Code
//! attaches each as an image. The paths must contain no spaces, or Claude Code would not see them as
//! paths; the caller copies a picture to such a path first. Dropped files go in as escaped paths and
//! stay in the text as written.

use crate::claude_cli::Claude;
use crate::compat::agents_json;
use crate::pty::{Attach, DEFAULT_SIZE};
use budd_core::crew::AgentRow;
use budd_core::delivery::{PathStyle, confirms, pasteable, with_files};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long the prompt may take to appear for a running session.
pub const PROMPT_TIMEOUT: Duration = Duration::from_secs(12);
/// How long the prompt may take to appear for a session asleep, which `claude attach` wakes first.
pub const WAKE_PROMPT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a woken session may take to be listed awake and idle.
pub const WAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a message may take to show up in the transcript.
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(8);
/// How long a clear may take to show up as a new session id.
pub const CLEAR_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the transcript is read while confirming.
const CONFIRM_POLL: Duration = Duration::from_millis(250);
/// How often the listing is read while waiting on it; listing starts a process, so once a second.
const LISTING_POLL: Duration = Duration::from_secs(1);
/// How long the client gets to settle before and after the paste.
const SETTLE: Duration = Duration::from_millis(300);
/// How long a woken session's input gets to settle once it is listed idle.
const WAKE_SETTLE: Duration = Duration::from_millis(1500);
/// How long the client gets after Return when there is no transcript to watch.
const BLIND_SETTLE: Duration = Duration::from_millis(1500);

/// The session a message goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The short id `claude attach` takes; a name works only while it is unambiguous.
    pub short_id: String,
    /// The conversation it has now, which a clear replaces.
    pub session_id: String,
    /// Its transcript, where a message is confirmed; without one a send is never confirmed.
    pub transcript: Option<PathBuf>,
    /// The session is asleep (retired while it waited on the human), so `claude attach` wakes it first.
    pub asleep: bool,
}

/// What goes to the session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Message {
    /// The text, as typed; line breaks are kept, other control characters dropped.
    pub text: String,
    /// Pictures to attach, as paths without spaces (the caller copies them to such a path).
    pub pictures: Vec<PathBuf>,
    /// Files dropped on the session, pasted as escaped paths after the text.
    pub files: Vec<PathBuf>,
}

/// The way a message went in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// Typed into `claude attach` in a hidden pty.
    Pty,
}

/// What became of a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Seen in the transcript (or, for a clear, in the listing).
    Delivered {
        /// How it went in.
        via: Via,
    },
    /// Typed, but not seen in time, with the reason in plain words.
    Unconfirmed(String),
}

impl Outcome {
    /// Whether the message arrived.
    pub fn is_delivered(&self) -> bool {
        matches!(self, Outcome::Delivered { .. })
    }
}

/// Sends `message` to `target`, with dropped files written in `style`, and says whether it arrived.
pub fn send(claude: &Claude, target: &Target, message: &Message, style: PathStyle) -> Outcome {
    let files: Vec<String> = message
        .files
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let pictures: Vec<String> = message
        .pictures
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    // The body is what the transcript must show: the text with the files' paths, without the pictures'.
    let body = with_files(&message.text, &files, style);
    let paste = pasteable(&body, &pictures);
    if paste.is_empty() {
        return Outcome::Unconfirmed("nothing to send".to_owned());
    }
    let start = target
        .transcript
        .as_deref()
        .and_then(|path| fs::metadata(path).ok())
        .map_or(0, |metadata| metadata.len());
    let mut attach = match open_prompt(claude, target) {
        Ok(attach) => attach,
        Err(reason) => return Outcome::Unconfirmed(reason),
    };
    if let Err(reason) = type_in(&mut attach, &paste) {
        return Outcome::Unconfirmed(reason);
    }
    let Some(transcript) = target.transcript.as_deref() else {
        attach.drain(BLIND_SETTLE);
        return Outcome::Unconfirmed("no transcript to check against".to_owned());
    };
    let deadline = Instant::now() + CONFIRM_TIMEOUT;
    while Instant::now() < deadline {
        attach.drain(CONFIRM_POLL);
        if confirms(complete_lines(transcript, start), &body, message.pictures.len()) {
            return Outcome::Delivered { via: Via::Pty };
        }
    }
    Outcome::Unconfirmed("it never showed up in the transcript".to_owned())
}

/// Clears `target`'s conversation, as `/clear` typed in its terminal would. It counts once the listing
/// shows the short id with a session id other than `target.session_id`.
pub fn clear(claude: &Claude, target: &Target) -> Outcome {
    let mut attach = match open_prompt(claude, target) {
        Ok(attach) => attach,
        Err(reason) => return Outcome::Unconfirmed(reason),
    };
    if let Err(reason) = type_in(&mut attach, "/clear") {
        return Outcome::Unconfirmed(reason);
    }
    let deadline = Instant::now() + CLEAR_TIMEOUT;
    while Instant::now() < deadline {
        attach.drain(LISTING_POLL);
        let renewed = listing(claude).iter().any(|row| {
            row.id.as_deref() == Some(target.short_id.as_str())
                && row.session_id.as_deref().is_some_and(|id| id != target.session_id)
        });
        if renewed {
            return Outcome::Delivered { via: Via::Pty };
        }
    }
    Outcome::Unconfirmed("the session never started a new conversation".to_owned())
}

/// `claude attach` with the session's prompt on screen and, for a session asleep, the session listed
/// awake and idle; or the reason it did not get there.
fn open_prompt(claude: &Claude, target: &Target) -> Result<Attach, String> {
    let mut attach = Attach::spawn(claude, &target.short_id, DEFAULT_SIZE)
        .map_err(|error| format!("could not start claude attach: {error}"))?;
    let timeout = if target.asleep {
        WAKE_PROMPT_TIMEOUT
    } else {
        PROMPT_TIMEOUT
    };
    attach.wait_prompt(timeout).map_err(|error| error.to_string())?;
    if target.asleep && !awake(claude, target, &attach) {
        return Err("the session did not wake up".to_owned());
    }
    Ok(attach)
}

/// Pastes `text` and presses Return, with a moment for the client to settle on either side.
fn type_in(attach: &mut Attach, text: &str) -> Result<(), String> {
    attach.drain(SETTLE);
    attach
        .paste(text)
        .map_err(|error| format!("could not paste into claude attach: {error}"))?;
    attach.drain(SETTLE);
    attach
        .enter()
        .map_err(|error| format!("could not press Return in claude attach: {error}"))
}

/// Waits, with the client's output pumped meanwhile, until the listing shows the session with a live
/// process and idle at its prompt, then a moment more for its input to settle.
fn awake(claude: &Claude, target: &Target, attach: &Attach) -> bool {
    let deadline = Instant::now() + WAKE_TIMEOUT;
    while Instant::now() < deadline {
        attach.drain(LISTING_POLL);
        let up = listing(claude).iter().any(|row| {
            row.id.as_deref() == Some(target.short_id.as_str())
                && row.pid.is_some()
                && row.status.as_deref() == Some("idle")
        });
        if up {
            attach.drain(WAKE_SETTLE);
            return true;
        }
    }
    false
}

/// The rows of `claude agents --json`, or none when the listing failed; it is polled, so a bad read
/// costs one look and not the delivery.
fn listing(claude: &Claude) -> Vec<AgentRow> {
    claude.agents(false).map(agents_json::from_values).unwrap_or_default()
}

/// The whole lines written to the transcript at `path` from byte `offset` on; a line still being
/// written is left for the next look.
fn complete_lines(path: &Path, offset: u64) -> Vec<String> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let Some(end) = bytes.iter().rposition(|&b| b == b'\n') else {
        return Vec::new();
    };
    String::from_utf8_lossy(&bytes[..end])
        .lines()
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn only_whole_lines_count_and_the_offset_is_honoured() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("t.jsonl");
        let mut file = File::create(&path).unwrap();
        writeln!(file, "old line").unwrap();
        let offset = fs::metadata(&path).unwrap().len();
        write!(file, "one\ntwo\npart").unwrap();
        assert_eq!(complete_lines(&path, offset), ["one", "two"]);
        writeln!(file, "ial").unwrap();
        assert_eq!(complete_lines(&path, offset), ["one", "two", "partial"]);
        assert!(complete_lines(&path, 1_000_000).is_empty());
        assert!(complete_lines(&temp.path().join("none.jsonl"), 0).is_empty());
    }

    #[test]
    fn an_empty_message_is_refused_before_anything_starts() {
        let claude = Claude::new("/nowhere/claude", "/nowhere/config");
        let target = Target {
            short_id: "abc".to_owned(),
            session_id: String::new(),
            transcript: None,
            asleep: false,
        };
        let message = Message {
            text: " \t \n \u{200B} ".to_owned(),
            ..Message::default()
        };
        assert_eq!(
            send(&claude, &target, &message, PathStyle::Posix),
            Outcome::Unconfirmed("nothing to send".to_owned())
        );
    }

    #[test]
    fn a_missing_binary_is_reported_not_panicked() {
        let claude = Claude::new("/nowhere/claude", "/nowhere/config");
        let target = Target {
            short_id: "abc".to_owned(),
            session_id: "old".to_owned(),
            transcript: None,
            asleep: false,
        };
        let outcome = clear(&claude, &target);
        let Outcome::Unconfirmed(reason) = outcome else {
            panic!("a missing binary cannot deliver");
        };
        assert!(
            reason.starts_with("could not start claude attach")
                || reason.starts_with("the session's prompt never appeared"),
            "{reason}"
        );
    }
}
