//! A test double for the `claude` CLI: the subcommands Budd drives, answered from files.
//!
//! It stands in for Claude Code in tests on all three platforms, so it does only what the tests need
//! and does it the way the real binary does:
//!
//! - `fake-claude --version` prints "2.1.316 (Claude Code)".
//! - `fake-claude agents --json [--all]` prints the JSON in the file `FAKE_CLAUDE_AGENTS` names (or `[]`).
//!   With `FAKE_CLAUDE_AGENTS_AFTER=<n>` and `FAKE_CLAUDE_AGENTS_LATER=<file>`, the first n calls print
//!   the first file and every call after them the second, so a test can let a session wake up; calls are
//!   counted in `<FAKE_CLAUDE_AGENTS>.calls`.
//! - `fake-claude attach <id>` puts its terminal in raw mode, prints a few lines and the prompt "❯ ", then
//!   reads stdin byte by byte. A complete bracketed paste followed by Return is recorded in the file
//!   `FAKE_CLAUDE_TRANSCRIPT` names: as a `user` entry, or as a queued `queue-operation` when
//!   `FAKE_CLAUDE_BUSY=1`. With `FAKE_CLAUDE_PICTURES=1`, image paths in the text become "[Image #n]"
//!   markers and image parts, as Claude Code records them. A paste of "/clear" instead rewrites the
//!   agents file, giving the id a new session id. With `FAKE_CLAUDE_NO_PROMPT=exit` it says the session
//!   is unknown and exits 1; with `FAKE_CLAUDE_NO_PROMPT=hang` it prints a line and never a prompt.
//!   It exits when stdin closes, or on SIGTERM.
//! - `fake-claude stop <id>` prints "stopped <id>".
//!
//! Budd starts `claude` with a scrubbed environment, so each setting is read from the environment first
//! and then from `fake-claude.env` in `CLAUDE_CONFIG_DIR`, one `NAME=value` per line.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

const VERSION: &str = "2.1.316 (Claude Code)";
const PROMPT: &str = "\u{276F} ";
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";
const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["--version"] | ["-v"] => {
            println!("{VERSION}");
            ExitCode::SUCCESS
        }
        ["agents", ..] => agents(),
        ["attach", id] => attach(id),
        ["stop", id] => {
            println!("stopped {id}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("fake-claude: unknown command: {}", words.join(" "));
            ExitCode::from(2)
        }
    }
}

/// A setting, from the environment or from `fake-claude.env` in the config folder.
fn setting(name: &str) -> Option<String> {
    if let Ok(value) = std::env::var(name) {
        return Some(value);
    }
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")?;
    let text = fs::read_to_string(PathBuf::from(config_dir).join("fake-claude.env")).ok()?;
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == name).then(|| value.trim().to_owned())
    })
}

/// The listing: the agents file, or the later one once enough calls have been made.
fn agents() -> ExitCode {
    let Some(path) = setting("FAKE_CLAUDE_AGENTS") else {
        println!("[]");
        return ExitCode::SUCCESS;
    };
    let mut chosen = PathBuf::from(&path);
    if let Some(after) = setting("FAKE_CLAUDE_AGENTS_AFTER").and_then(|n| n.parse::<u64>().ok())
        && let Some(later) = setting("FAKE_CLAUDE_AGENTS_LATER")
    {
        let counter = PathBuf::from(format!("{path}.calls"));
        let calls = fs::read_to_string(&counter)
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .unwrap_or(0)
            + 1;
        let _ = fs::write(&counter, calls.to_string());
        if calls > after {
            chosen = PathBuf::from(later);
        }
    }
    match fs::read_to_string(&chosen) {
        Ok(text) => {
            println!("{}", text.trim_end());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("fake-claude: cannot read {}: {error}", chosen.display());
            ExitCode::from(1)
        }
    }
}

/// The attached client: a prompt, and a transcript entry for every pasted message.
fn attach(id: &str) -> ExitCode {
    let no_prompt = setting("FAKE_CLAUDE_NO_PROMPT").unwrap_or_default();
    if no_prompt == "exit" {
        println!("No session found matching {id}");
        return ExitCode::from(1);
    }
    terminal::raw_mode();
    let mut out = io::stdout().lock();
    let _ = write!(out, "fake claude 2.1.316\r\nattached to {id}\r\n\r\n");
    if no_prompt == "hang" {
        let _ = write!(out, "Connecting to {id}...");
    } else {
        let _ = write!(out, "{PROMPT}");
    }
    let _ = out.flush();
    let mut input = io::stdin().lock();
    let mut typing = Typing::default();
    let mut byte = [0u8; 1];
    loop {
        match input.read(&mut byte) {
            Ok(1) => {}
            // Ok(0) is the terminal hung up; an error is the same on the platforms a pty can give one.
            _ => return ExitCode::SUCCESS,
        }
        if let Some(text) = typing.feed(byte[0]) {
            record(id, &text);
            let _ = write!(out, "\r\n{PROMPT}");
            let _ = out.flush();
        }
    }
}

/// The bytes typed so far, read for a bracketed paste and the Return after it.
#[derive(Default)]
struct Typing {
    bytes: Vec<u8>,
    pasted: Option<String>,
}

impl Typing {
    /// Takes one byte; gives the pasted message once Return follows a complete paste.
    fn feed(&mut self, byte: u8) -> Option<String> {
        if let Some(text) = self.pasted.as_ref() {
            if byte == b'\r' || byte == b'\n' {
                let text = text.clone();
                self.pasted = None;
                self.bytes.clear();
                return Some(text);
            }
            return None;
        }
        self.bytes.push(byte);
        if self.bytes.ends_with(PASTE_END)
            && let Some(start) = find(&self.bytes, PASTE_START)
        {
            let inside = &self.bytes[start + PASTE_START.len()..self.bytes.len() - PASTE_END.len()];
            self.pasted = Some(String::from_utf8_lossy(inside).into_owned());
        }
        None
    }
}

/// Where `needle` first occurs in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// What Claude Code does with a message: `/clear` starts a new conversation, anything else goes into
/// the transcript.
fn record(id: &str, text: &str) {
    if text.trim() == "/clear" {
        renew_session(id);
        return;
    }
    let Some(transcript) = setting("FAKE_CLAUDE_TRANSCRIPT") else {
        return;
    };
    let entry = if setting("FAKE_CLAUDE_BUSY").as_deref() == Some("1") {
        serde_json::json!({"type": "queue-operation", "operation": "enqueue", "content": text})
    } else if setting("FAKE_CLAUDE_PICTURES").as_deref() == Some("1") {
        serde_json::json!({"type": "user", "message": {"role": "user", "content": with_images(text)}})
    } else {
        serde_json::json!({"type": "user", "message": {"role": "user", "content": text}})
    };
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(transcript) {
        let _ = writeln!(file, "{entry}");
    }
}

/// `text` as Claude Code records a message with pictures pasted as paths: each path leaves the text,
/// an "[Image #n]" marker goes in front, and an image part follows the text part.
fn with_images(text: &str) -> serde_json::Value {
    let mut words = Vec::new();
    let mut images = 0usize;
    for word in text.split(' ') {
        if is_image_path(word) {
            images += 1;
        } else {
            words.push(word);
        }
    }
    if images == 0 {
        return serde_json::Value::String(text.to_owned());
    }
    let markers: String = (1..=images).map(|n| format!("[Image #{n}]")).collect();
    let rest = words.join(" ");
    let mut parts = vec![serde_json::json!({"type": "text", "text": format!("{markers}{}", rest.trim())})];
    for _ in 0..images {
        parts.push(serde_json::json!({
            "type": "image",
            "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}
        }));
    }
    serde_json::Value::Array(parts)
}

/// Whether `word` is an absolute path to a picture: a Unix path, a drive path or a UNC path ending in
/// an image extension.
fn is_image_path(word: &str) -> bool {
    let absolute = word.starts_with('/')
        || word.starts_with('\\')
        || word
            .as_bytes()
            .get(1..3)
            .is_some_and(|rest| rest == b":\\" || rest == b":/");
    let Some((_, extension)) = word.rsplit_once('.') else {
        return false;
    };
    absolute
        && IMAGE_EXTENSIONS
            .iter()
            .any(|known| extension.eq_ignore_ascii_case(known))
}

/// Gives the session `id` a new session id in the agents file, as `/clear` does.
fn renew_session(id: &str) {
    let Some(path) = setting("FAKE_CLAUDE_AGENTS") else {
        return;
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    let Ok(mut rows) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    if let Some(rows) = rows.as_array_mut() {
        for row in rows.iter_mut() {
            if row.get("id").and_then(|v| v.as_str()) == Some(id)
                && let Some(object) = row.as_object_mut()
            {
                object.insert("sessionId".to_owned(), serde_json::Value::String(new_session_id()));
            }
        }
    }
    let _ = fs::write(&path, rows.to_string());
}

/// A fresh id in the shape of a UUID, from the clock and the process id.
fn new_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // An odd multiplier spreads the clock's bits over all 128, so the id does not start with zeros.
    let mixed = nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15_F39C_C060_5CED_C835) ^ u128::from(std::process::id());
    let hex = format!("{mixed:032x}");
    format!(
        "{}-{}-4{}-8{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

/// Raw input, as Claude Code sets it up: a Return arrives as one byte and an escape sequence as itself,
/// nothing is echoed, and the terminal speaks UTF-8.
mod terminal {
    #[cfg(unix)]
    pub fn raw_mode() {
        // stty works on its standard input, which is the pty the client was started in.
        let _ = std::process::Command::new("stty")
            .args(["raw", "-echo"])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }

    #[cfg(windows)]
    pub fn raw_mode() {
        use std::ffi::c_void;

        type Handle = *mut c_void;
        const STD_INPUT_HANDLE: u32 = 0xFFFF_FFF6;
        const INVALID_HANDLE_VALUE: isize = -1;
        const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
        const UTF8: u32 = 65001;

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetStdHandle(std_handle: u32) -> Handle;
            fn SetConsoleMode(console: Handle, mode: u32) -> i32;
            fn SetConsoleCP(code_page: u32) -> i32;
            fn SetConsoleOutputCP(code_page: u32) -> i32;
        }

        // SAFETY: plain Win32 calls on this process's own console; the stdin handle is owned by the
        // process for its whole life and no memory is handed across the boundary.
        unsafe {
            SetConsoleCP(UTF8);
            SetConsoleOutputCP(UTF8);
            let input = GetStdHandle(STD_INPUT_HANDLE);
            if !input.is_null() && input as isize != INVALID_HANDLE_VALUE {
                SetConsoleMode(input, ENABLE_VIRTUAL_TERMINAL_INPUT);
            }
        }
    }

    #[cfg(not(any(unix, windows)))]
    pub fn raw_mode() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paste_counts_once_return_follows_it() {
        let mut typing = Typing::default();
        let mut got = None;
        for byte in b"\x1b[200~first\nsecond\x1b[201~" {
            got = typing.feed(*byte);
        }
        assert_eq!(got, None);
        assert_eq!(typing.feed(b'\r').as_deref(), Some("first\nsecond"));
        // The next paste starts clean.
        for byte in b"x\x1b[200~/clear\x1b[201~" {
            assert_eq!(typing.feed(*byte), None);
        }
        assert_eq!(typing.feed(b'\n').as_deref(), Some("/clear"));
    }

    #[test]
    fn pictures_become_markers_and_image_parts() {
        let value = with_images("what is this? /tmp/a.png C:\\shots\\b.JPG");
        let parts = value.as_array().unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0]["text"], "[Image #1][Image #2]what is this?");
        assert_eq!(parts[1]["type"], "image");
        assert_eq!(with_images("no pictures here"), "no pictures here");
        assert!(!is_image_path("photo.png"));
        assert!(!is_image_path("/tmp/notes.txt"));
    }

    #[test]
    fn a_new_session_id_has_the_uuid_shape() {
        let id = new_session_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.split('-').map(str::len).collect::<Vec<_>>(), [8, 4, 4, 4, 12]);
        assert_ne!(id, new_session_id());
    }
}
