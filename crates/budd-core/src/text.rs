//! Text helpers several modules share: what a transcript entry says in the human's words,
//! image markers, tags, and the slash commands that count as work.
//!
//! Ported from `TranscriptSummary.humanText`, `DeliveryCheck.withoutImageMarkers` and
//! `TranscriptSummary.isWorkCommand` in the Swift `AvatarCore`.

use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

static IMAGE_MARKER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[Image #\d+\]").unwrap());
static PASTE_OPEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^<pasted_content[^>]*>\s*").unwrap());
static PASTE_CLOSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*</pasted_content>$").unwrap());
static WHITESPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// Claude Code's own slash commands, which never count as a session's work command.
pub const BUILT_IN_COMMANDS: &[&str] = &[
    "add-dir",
    "agents",
    "chrome",
    "clear",
    "compact",
    "config",
    "context",
    "cost",
    "doctor",
    "effort",
    "exit",
    "export",
    "fast",
    "help",
    "hooks",
    "ide",
    "init",
    "login",
    "logout",
    "mcp",
    "memory",
    "model",
    "permissions",
    "plugin",
    "resume",
    "rewind",
    "status",
    "statusline",
    "theme",
    "usage",
    "vim",
];

/// `[Image #1]` markers, which Claude Code puts in front of a pasted picture's path, replaced by a space.
pub fn without_image_markers(text: &str) -> String {
    IMAGE_MARKER.replace_all(text, " ").into_owned()
}

/// Runs of whitespace collapsed to one space, and the ends trimmed.
pub fn collapsed(text: &str) -> String {
    WHITESPACE.replace_all(text.trim(), " ").into_owned()
}

/// The contents of the first `<name>…</name>` tag in `text`, trimmed, or `None` without one or when it is empty.
pub fn tag(name: &str, text: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    let inside = text[start..end].trim();
    if inside.is_empty() {
        None
    } else {
        Some(inside.to_owned())
    }
}

/// The text of a transcript entry's `message.content`: a string, or the `text` parts of a list joined by newlines.
pub fn content_text(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter(|p| p.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    }
}

/// What the human said in a transcript entry, or `None` when the entry is not the human speaking.
///
/// A long message, which arrives as one burst, is wrapped as a paste; it is still the human's.
/// A typed slash command is recorded as `<command-name>` and `<command-args>` tags and comes back as `name args`.
/// Anything else that starts with `<` (bash input, system reminders, task notifications) is not the human.
pub fn human_text(content: &Value) -> Option<String> {
    let text = content_text(content)?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if !text.starts_with('<') {
        return Some(text.to_owned());
    }
    if text.starts_with("<pasted_content") {
        let unwrapped = PASTE_CLOSE.replace(&PASTE_OPEN.replace(text, ""), "").into_owned();
        return if unwrapped.is_empty() { None } else { Some(unwrapped) };
    }
    let name = tag("command-name", text)?;
    let arguments = tag("command-args", text).unwrap_or_default();
    Some(if arguments.is_empty() {
        name
    } else {
        format!("{name} {arguments}")
    })
}

/// Whether a prompt is a slash command of the session's own work, rather than one of Claude Code's.
pub fn is_work_command(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('/') else {
        return false;
    };
    let name = rest.split_whitespace().next().unwrap_or("");
    !name.is_empty() && !BUILT_IN_COMMANDS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plain_text_is_the_human() {
        assert_eq!(human_text(&json!("  hello  ")).as_deref(), Some("hello"));
        assert_eq!(
            human_text(&json!([{"type": "text", "text": "a"}, {"type": "image"}, {"type": "text", "text": "b"}]))
                .as_deref(),
            Some("a\nb")
        );
        assert_eq!(human_text(&json!("")), None);
        assert_eq!(human_text(&json!(null)), None);
    }

    #[test]
    fn pasted_content_is_unwrapped() {
        assert_eq!(
            human_text(&json!("<pasted_content id=\"x\">\n  long text\n</pasted_content>")).as_deref(),
            Some("long text")
        );
        assert_eq!(human_text(&json!("<pasted_content></pasted_content>")), None);
    }

    #[test]
    fn commands_and_tags() {
        assert_eq!(
            human_text(&json!(
                "<command-name>/lets-do-it</command-name><command-args>CASBO-1</command-args>"
            ))
            .as_deref(),
            Some("/lets-do-it CASBO-1")
        );
        assert_eq!(
            human_text(&json!("<command-name>/clear</command-name>")).as_deref(),
            Some("/clear")
        );
        assert_eq!(human_text(&json!("<bash-input>ls</bash-input>")), None);
        assert_eq!(human_text(&json!("<system-reminder>x</system-reminder>")), None);
        assert_eq!(
            tag("bash-stdout", "<bash-stdout> out </bash-stdout>").as_deref(),
            Some("out")
        );
    }

    #[test]
    fn markers_whitespace_and_work_commands() {
        assert_eq!(without_image_markers("[Image #1] a [Image #12]b"), "  a  b");
        assert_eq!(collapsed("  a \n\t b  "), "a b");
        assert!(is_work_command("/lets-do-it CASBO-1"));
        assert!(!is_work_command("/clear"));
        assert!(!is_work_command("hello"));
        assert!(!is_work_command("/"));
    }
}
