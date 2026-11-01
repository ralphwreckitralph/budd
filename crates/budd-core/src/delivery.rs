//! What a message looks like when pasted, and how its arrival is confirmed in a transcript (from AvatarCore/DeliveryCheck.swift).
//!
//! Also what the office says while files are held over a desk, and how a message's attachments are noted
//! over its answer (from AvatarCore/DropCatch.swift).
//!
//! An idle session records a `user` entry; a busy one queues the message and records a
//! `queue-operation` with `operation: "enqueue"` straight away.
//!
//! Pictures go in as file paths pasted after the text, as a terminal pastes a dragged file. Claude Code
//! turns each into an image attachment: the path leaves the text, an "[Image #1]" marker takes its place
//! at the front, and an image part joins the entry. So the text is matched without markers, and the
//! pictures by counting image parts.
//!
//! A message starting with "!" is a shell command, which Claude Code runs itself (bash mode): it records
//! `<bash-input>command</bash-input>` without the "!", then `<bash-stdout>…<bash-stderr>…` with its output.

use crate::text::{content_text, tag, without_image_markers};
use crate::transcript::Entry;
use fancy_regex::Regex as FancyRegex;
use regex::Regex;
use std::sync::LazyLock;

static IMAGE_PATH: LazyLock<FancyRegex> =
    LazyLock::new(|| FancyRegex::new(r"(?i)(^|\s)/\S+\.(png|jpe?g|gif|webp)(?=\s|$)").unwrap());
static FILE_ADDRESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\bfile://\S+").unwrap());
static DROPPED_PATH: LazyLock<FancyRegex> =
    LazyLock::new(|| FancyRegex::new(r"(?<!\S)/(?:\\.|[^\s\\])*/(?:\\.|[^\s\\])+").unwrap());
static ESCAPED_CHARACTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\(.)").unwrap());

/// How a dropped file's path is written into the message, for the shell the session's terminal runs.
///
/// `Posix` is what macOS Terminal and iTerm write: every shell-special character backslash-escaped.
/// `WindowsQuoted` is provisional, for a Windows terminal pasting a dropped file: the whole path in
/// double quotes, with any double quote inside it doubled. It has not been checked against a real
/// Claude Code session on Windows yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathStyle {
    /// Backslash escapes, as Terminal and iTerm write a dropped file.
    Posix,
    /// The path wrapped in double quotes, inner quotes doubled.
    WindowsQuoted,
}

/// The characters a POSIX shell would split or expand on, which `escaped_path` puts a backslash before.
const SHELL_SPECIAL: &str = " \t!\"#$&'()*,;<>?[\\]^`{|}~";

/// What is pasted: the text, then each picture's path, space separated. The paths must be free of
/// spaces, or Claude Code would not see them as paths.
pub fn pasteable(text: &str, pictures: &[String]) -> String {
    joined(std::iter::once(sendable(text)).chain(pictures.iter().cloned()))
}

/// The message with files dropped on a desk or a card: the text, then each file's path as a terminal
/// pastes a dragged file. Claude Code keeps such a path in the message as written, and the session opens it.
pub fn with_files(text: &str, paths: &[String], style: PathStyle) -> String {
    joined(std::iter::once(sendable(text)).chain(paths.iter().map(|path| escaped_path(path, style))))
}

/// The non-empty pieces of a message, space separated.
fn joined(pieces: impl Iterator<Item = String>) -> String {
    pieces.filter(|piece| !piece.is_empty()).collect::<Vec<_>>().join(" ")
}

/// `path` as a terminal pastes a dropped file, so a space does not end the path: with `PathStyle::Posix`,
/// the characters a shell would split or expand on escaped by a backslash; with
/// `PathStyle::WindowsQuoted`, the path in double quotes.
pub fn escaped_path(path: &str, style: PathStyle) -> String {
    match style {
        PathStyle::Posix => {
            let mut escaped = String::with_capacity(path.len());
            for character in path.chars() {
                if SHELL_SPECIAL.contains(character) {
                    escaped.push('\\');
                }
                escaped.push(character);
            }
            escaped
        }
        PathStyle::WindowsQuoted => format!("\"{}\"", path.replace('"', "\"\"")),
    }
}

/// Whether the transcript `lines` written since a message was typed show it reached the session.
///
/// `text` is the message's text as `sendable` gives it, without the pictures' paths; `pictures` is how
/// many pictures went with it.
pub fn confirms<S: AsRef<str>>(lines: impl IntoIterator<Item = S>, text: &str, pictures: usize) -> bool {
    let wanted = normalized(text);
    if wanted.is_empty() && pictures == 0 {
        return false;
    }
    for line in lines {
        let Some(entry) = Entry::parse(line.as_ref()) else {
            continue;
        };
        match entry.kind.as_deref() {
            Some("queue-operation") => {
                // Queued as typed, or with the pictures already turned into markers.
                if entry.operation.as_deref() == Some("enqueue")
                    && let Some(content) = entry.content.as_deref()
                    && normalized(&without_image_paths(&without_image_markers(content))) == wanted
                {
                    return true;
                }
            }
            Some("user") => {
                let Some(said) = user_text(&entry).map(|text| normalized(&without_image_markers(&text))) else {
                    continue;
                };
                // A recorded shell command answers only a message sent with "!"; plain text inside it is not that message.
                if let Some(command) = shell_command(&said) {
                    if let Some(rest) = wanted.strip_prefix('!')
                        && normalized(&command) == normalized(rest)
                    {
                        return true;
                    }
                    continue;
                }
                if (wanted.is_empty() || said.contains(&wanted)) && image_parts(&entry) >= pictures {
                    return true;
                }
            }
            _ => continue,
        }
    }
    false
}

/// The message as it is pasted into the session: line breaks kept (as "\n"), other control characters
/// dropped, since an escape would end the bracketed paste early, and ends trimmed.
pub fn sendable(text: &str) -> String {
    let lines = text.replace("\r\n", "\n").replace('\r', "\n");
    let kept: String = lines
        .chars()
        .filter(|&c| c == '\n' || c == '\t' || !is_control_character(c))
        .collect();
    kept.trim().to_owned()
}

/// Whether `c` is in Foundation's `controlCharacters` set: Unicode general categories Cc and Cf.
fn is_control_character(c: char) -> bool {
    c.is_control() || is_format_character(c)
}

/// Unicode general category Cf (format characters), as of Unicode 15.1, since the standard library has
/// no category tables.
fn is_format_character(c: char) -> bool {
    const RANGES: &[(u32, u32)] = &[
        (0x00AD, 0x00AD),
        (0x0600, 0x0605),
        (0x061C, 0x061C),
        (0x06DD, 0x06DD),
        (0x070F, 0x070F),
        (0x0890, 0x0891),
        (0x08E2, 0x08E2),
        (0x180E, 0x180E),
        (0x200B, 0x200F),
        (0x202A, 0x202E),
        (0x2060, 0x2064),
        (0x2066, 0x206F),
        (0xFEFF, 0xFEFF),
        (0xFFF9, 0xFFFB),
        (0x110BD, 0x110BD),
        (0x110CD, 0x110CD),
        (0x13430, 0x1343F),
        (0x1BCA0, 0x1BCA3),
        (0x1D173, 0x1D17A),
        (0xE0001, 0xE0001),
        (0xE0020, 0xE007F),
    ];
    let code = u32::from(c);
    RANGES.iter().any(|&(low, high)| (low..=high).contains(&code))
}

/// `text` with each picture's path, as pasted after the text, taken out.
fn without_image_paths(text: &str) -> String {
    IMAGE_PATH.replace_all(text, " ").into_owned()
}

/// How many image parts a `user` entry carries.
fn image_parts(entry: &Entry) -> usize {
    entry
        .parts()
        .iter()
        .filter(|part| part.get("type").and_then(|t| t.as_str()) == Some("image"))
        .count()
}

/// The command of a shell escape as the transcript records it, or `None` for anything else.
pub fn shell_command(text: &str) -> Option<String> {
    tag_inside("bash-input", text)
}

/// What a shell escape printed, as the transcript records it after the command: stdout, then stderr,
/// with the HTML escapes Claude Code puts in undone. `None` for anything else.
pub fn shell_output(text: &str) -> Option<String> {
    if !text.starts_with("<bash-stdout>") && !text.starts_with("<bash-stderr>") {
        return None;
    }
    let output = [tag_inside("bash-stdout", text), tag_inside("bash-stderr", text)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
    Some(
        output
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&amp;", "&"),
    )
}

/// The contents of the first `<name>…</name>` tag, or `None` when there is none or it is empty, as the
/// Swift `tag` helper had it.
fn tag_inside(name: &str, text: &str) -> Option<String> {
    tag(name, text).filter(|inside| !inside.is_empty())
}

/// `text` with runs of whitespace collapsed to one space, for comparing what was typed with what was recorded.
fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The text of a `user` entry: its content as a string, or the text parts joined by newlines.
fn user_text(entry: &Entry) -> Option<String> {
    content_text(entry.content()?)
}

/// What the office's strip says while files are held over a desk: the file's name for one, how many
/// for several, and "it" when the names are not known yet (a picture from a browser, a promised file).
pub fn invitation(names: &[String]) -> String {
    match names {
        [] => "Drop to attach it".to_owned(),
        [name] => format!("Drop to attach {name}"),
        _ => format!("Drop to attach {} files", names.len()),
    }
}

/// How a message's attachments are noted over its answer in the card: "[picture]", "[report.pdf]".
pub fn attachment_note(pictures: usize, files: &[String]) -> String {
    let picture = match pictures {
        0 => None,
        1 => Some("[picture]".to_owned()),
        n => Some(format!("[{n} pictures]")),
    };
    let file = match files.len() {
        0 => None,
        1 | 2 => Some(format!("[{}]", files.join(", "))),
        n => Some(format!("[{n} files]")),
    };
    [picture, file].into_iter().flatten().collect::<Vec<_>>().join(" ")
}

/// A prompt as the card shows it, with each absolute path in it, as dropped files go in, cut to its
/// file's name in brackets, so the line says what was sent rather than where it lives. A slash
/// command is not a path, as it has no second slash. A file's address (a file dragged into a text
/// box writes one) is cut the same way.
pub fn shortened(prompt: &str) -> String {
    let named = FILE_ADDRESS.replace_all(prompt, |found: &regex::Captures| {
        let address = &found[0];
        file_address_name(address).map_or_else(|| address.to_owned(), |name| format!("[{name}]"))
    });
    let mut result = named.to_string();
    let matches: Vec<_> = DROPPED_PATH.find_iter(&named).filter_map(Result::ok).collect();
    for found in matches.into_iter().rev() {
        let unescaped = ESCAPED_CHARACTER.replace_all(found.as_str(), "$1");
        result.replace_range(found.range(), &format!("[{}]", last_path_component(&unescaped)));
    }
    result
}

/// The file name a `file://` address points at, percent-decoded, or `None` when the address is not one
/// the Swift `URL(string:)` would accept.
fn file_address_name(address: &str) -> Option<String> {
    let rest = address.strip_prefix("file://")?;
    let path = rest.split(['?', '#']).next().unwrap_or("");
    let path = path.find('/').map_or("", |slash| &path[slash..]);
    if path
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '<' || c == '>' || c == '`')
    {
        return None;
    }
    let decoded = percent_decoded(path)?;
    Some(last_path_component(&decoded).to_owned())
}

/// `text` with its `%XX` escapes turned back into bytes, as UTF-8, or `None` when they do not form UTF-8.
fn percent_decoded(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let pair = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
            let byte = u8::from_str_radix(pair, 16).ok()?;
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The last component of a path, as `NSString.lastPathComponent` gives it: trailing slashes are ignored,
/// and "/" stays "/".
pub fn last_path_component(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.is_empty() { "" } else { "/" };
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn confirmed(lines: &[&str], text: &str) -> bool {
        confirms(lines, text, 0)
    }

    #[test]
    fn idle_session_records_a_user_entry() {
        let lines = [
            r#"{"type":"attachment"}"#,
            r#"{"type":"user","message":{"role":"user","content":"Reply with   exactly: pineapple"}}"#,
        ];
        assert!(confirmed(&lines, "Reply with exactly: pineapple"));
        assert!(!confirmed(&lines, "something else"));
    }

    #[test]
    fn busy_session_queues_it() {
        let lines = [r#"{"type":"queue-operation","operation":"enqueue","content":"second-done please"}"#];
        assert!(confirmed(&lines, "second-done please"));
        let removed = [r#"{"type":"queue-operation","operation":"remove","content":"second-done please"}"#];
        assert!(!confirmed(&removed, "second-done please"));
    }

    #[test]
    fn user_content_can_be_parts() {
        let lines = [r#"{"type":"user","message":{"content":[{"type":"text","text":"hi there"}]}}"#];
        assert!(confirmed(&lines, "hi there"));
    }

    #[test]
    fn tolerates_garbage_and_empty_text() {
        assert!(!confirmed(&["not json", "{"], "hi"));
        assert!(!confirmed(&[r#"{"type":"user","message":{"content":"hi"}}"#], "  "));
    }

    #[test]
    fn keeps_line_breaks_and_drops_other_controls() {
        assert_eq!(
            sendable("  first line\nsecond\r\nthird\rfourth \n"),
            "first line\nsecond\nthird\nfourth"
        );
        // An escape would end the bracketed paste and let the rest act as keystrokes.
        assert_eq!(sendable("a\u{1B}[201~b\tc"), "a[201~b\tc");
    }

    #[test]
    fn confirms_a_message_with_line_breaks() {
        let lines = [r#"{"type":"user","message":{"content":"first line\nsecond line"}}"#];
        assert!(confirmed(&lines, "first line\nsecond line"));
    }

    #[test]
    fn pictures_are_pasted_as_paths_after_the_text() {
        assert_eq!(
            pasteable(" what is this? \n", &["/tmp/a.png".to_owned(), "/tmp/b.png".to_owned()]),
            "what is this? /tmp/a.png /tmp/b.png"
        );
        assert_eq!(pasteable("", &["/tmp/a.png".to_owned()]), "/tmp/a.png");
    }

    #[test]
    fn confirms_a_picture_as_claude_code_records_it() {
        // As a session recorded "what colour is this, one word? /…/red.png": the path became a marker
        // at the front and an image part.
        let entry = r#"{"type":"user","message":{"content":[{"type":"text","text":"[Image #1]what colour is this, one word?"},{"type":"image","source":{"type":"base64","data":"…"}}]}}"#;
        assert!(confirms([entry], "what colour is this, one word?", 1));
        // The text without its picture is not enough.
        assert!(!confirms([entry], "what colour is this, one word?", 2));
        let alone = r#"{"type":"user","message":{"content":[{"type":"text","text":"[Image #2]"},{"type":"image"}]}}"#;
        assert!(confirms([alone], "", 1));
        assert!(!confirms([alone], "", 0));
    }

    #[test]
    fn confirms_a_queued_picture_either_way() {
        let typed = r#"{"type":"queue-operation","operation":"enqueue","content":"and this? /Users/me/Library/Caches/x/budd-1.png"}"#;
        assert!(confirms([typed], "and this?", 1));
        let marked = r#"{"type":"queue-operation","operation":"enqueue","content":"[Image #2]and this?"}"#;
        assert!(confirms([marked], "and this?", 1));
    }

    #[test]
    fn files_are_pasted_as_a_terminal_drops_them() {
        assert_eq!(
            escaped_path("/Users/me/My Docs/notes v2.txt", PathStyle::Posix),
            r"/Users/me/My\ Docs/notes\ v2.txt"
        );
        assert_eq!(
            escaped_path("/tmp/a (1)&b's.pdf", PathStyle::Posix),
            r"/tmp/a\ \(1\)\&b\'s.pdf"
        );
        assert_eq!(
            escaped_path("/tmp/plain-file_1.txt", PathStyle::Posix),
            "/tmp/plain-file_1.txt"
        );
        assert_eq!(
            with_files(
                " read these \n",
                &["/tmp/a.txt".to_owned(), "/tmp/My Docs".to_owned()],
                PathStyle::Posix
            ),
            r"read these /tmp/a.txt /tmp/My\ Docs"
        );
        assert_eq!(
            with_files("", &["/tmp/a.txt".to_owned()], PathStyle::Posix),
            "/tmp/a.txt"
        );
    }

    #[test]
    fn confirms_files_as_claude_code_records_them() {
        // As a session recorded it on 2026-10-09: the escaped paths kept in the text as pasted.
        let message = with_files(
            "what is in these?",
            &["/tmp/plain.txt".to_owned(), "/tmp/My Docs/notes v2.txt".to_owned()],
            PathStyle::Posix,
        );
        let entry = r#"{"type":"user","message":{"role":"user","content":"what is in these? /tmp/plain.txt /tmp/My\\ Docs/notes\\ v2.txt"}}"#;
        assert!(confirms([entry], &message, 0));
        let queued = r#"{"type":"queue-operation","operation":"enqueue","content":"what is in these? /tmp/plain.txt /tmp/My\\ Docs/notes\\ v2.txt"}"#;
        assert!(confirms([queued], &message, 0));
        // With a picture too, whose path Claude Code turns into a marker.
        let both = r#"{"type":"queue-operation","operation":"enqueue","content":"what is in these? /tmp/plain.txt /tmp/My\\ Docs/notes\\ v2.txt /Users/me/Library/Caches/x/budd-1.png"}"#;
        assert!(confirms([both], &message, 1));
        assert!(crate::transcript::turn_is_for(
            &format!("[Image #1]{message}"),
            &message
        ));
    }

    #[test]
    fn windows_paths_are_quoted() {
        assert_eq!(
            escaped_path(r"C:\Users\me\My Docs\notes v2.txt", PathStyle::WindowsQuoted),
            r#""C:\Users\me\My Docs\notes v2.txt""#
        );
        assert_eq!(
            escaped_path(r#"C:\say "hi".txt"#, PathStyle::WindowsQuoted),
            r#""C:\say ""hi"".txt""#
        );
        assert_eq!(
            with_files("read", &[r"C:\a b.txt".to_owned()], PathStyle::WindowsQuoted),
            r#"read "C:\a b.txt""#
        );
    }

    // ShellEscapeTests
    // pending-rollback-label, 2026-10-09: the human ran the destructive scan from Budd with "!".
    const SHELL_LINES: [&str; 2] = [
        r#"{"type":"user","message":{"role":"user","content":"<bash-input> bash scripts/destructive-scan.sh --repo /w --out /t/dscan.json</bash-input>"}}"#,
        r#"{"type":"user","message":{"role":"user","content":"<bash-stdout></bash-stdout><bash-stderr>destructive scan: flag, 1 item(s) -&gt; /t/dscan.json</bash-stderr>"}}"#,
    ];

    #[test]
    fn a_shell_escape_counts_as_arrived() {
        assert!(confirmed(
            &SHELL_LINES,
            "! bash scripts/destructive-scan.sh --repo /w --out /t/dscan.json"
        ));
        assert!(confirmed(
            &SHELL_LINES,
            "!bash  scripts/destructive-scan.sh --repo /w --out /t/dscan.json"
        ));
        assert!(!confirmed(&SHELL_LINES, "! bash scripts/other.sh"));
        assert!(
            !confirmed(
                &SHELL_LINES,
                "bash scripts/destructive-scan.sh --repo /w --out /t/dscan.json"
            ),
            "without ! it was a message, not this"
        );
    }

    #[test]
    fn shell_output_undoes_html_escapes() {
        assert_eq!(
            shell_output("<bash-stdout>a &lt;b&gt; &quot;c&quot; &amp;d</bash-stdout><bash-stderr>e</bash-stderr>")
                .as_deref(),
            Some("a <b> \"c\" &d\ne")
        );
        assert_eq!(
            shell_output("<bash-stdout></bash-stdout><bash-stderr></bash-stderr>").as_deref(),
            Some("")
        );
        assert_eq!(shell_output("plain"), None);
        assert_eq!(shell_command("<bash-input></bash-input>"), None);
    }

    // DropCatchTests
    #[test]
    fn invites_the_drop_by_name() {
        assert_eq!(invitation(&["report.pdf".to_owned()]), "Drop to attach report.pdf");
        assert_eq!(
            invitation(&["a.txt".to_owned(), "b.txt".to_owned(), "c.txt".to_owned()]),
            "Drop to attach 3 files"
        );
        assert_eq!(invitation(&[]), "Drop to attach it");
    }

    #[test]
    fn notes_attachments_over_the_answer() {
        assert_eq!(attachment_note(0, &[]), "");
        assert_eq!(attachment_note(1, &["report.pdf".to_owned()]), "[picture] [report.pdf]");
        assert_eq!(
            attachment_note(2, &["a.txt".to_owned(), "b.txt".to_owned()]),
            "[2 pictures] [a.txt, b.txt]"
        );
        assert_eq!(
            attachment_note(0, &["a".to_owned(), "b".to_owned(), "c".to_owned()]),
            "[3 files]"
        );
    }

    #[test]
    fn shows_dropped_paths_by_name() {
        assert_eq!(
            shortened(r"what is in these? /Users/me/plain.txt /Users/me/My\ Docs/notes\ v2.txt"),
            "what is in these? [plain.txt] [notes v2.txt]"
        );
        assert_eq!(shortened("look in\n/tmp/build/"), "look in\n[build]");
        assert_eq!(
            shortened(
                "file:///var/folders/yg/T/TemporaryItems/NSIRD_screencaptureui_Pn49IZ/Screenshot%202026-10-15%20at%206.50.11%E2%80%AFPM.png see this"
            ),
            "[Screenshot 2026-10-15 at 6.50.11\u{202F}PM.png] see this"
        );
        // Slash commands, URLs and words with slashes are left alone.
        assert_eq!(shortened("/lets-do-it CASBO-1"), "/lets-do-it CASBO-1");
        assert_eq!(
            shortened("see https://x.dev/a/b and/or c"),
            "see https://x.dev/a/b and/or c"
        );
    }

    #[test]
    fn last_path_component_matches_foundation() {
        assert_eq!(last_path_component("/a/b/Bubble.swift"), "Bubble.swift");
        assert_eq!(last_path_component("/tmp/build/"), "build");
        assert_eq!(last_path_component("/"), "/");
        assert_eq!(last_path_component(""), "");
        assert_eq!(last_path_component("plain"), "plain");
    }
}
