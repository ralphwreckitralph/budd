//! Transcript entries, the tail reader, turn summaries and answer lookup (from AvatarCore/TranscriptSummary.swift, TranscriptFollower.swift).
//!
//! A transcript is Claude Code's JSONL log of a session. Nothing here opens a file: the follower reads
//! through a `TranscriptSource`, and everything else takes lines.

use crate::delivery::{last_path_component, shell_command, shell_output};
use crate::history::{HistoryTurn, TranscriptSource};
use crate::text::{human_text, is_work_command};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::collections::HashSet;
use unicode_segmentation::UnicodeSegmentation;

/// Deserialises a field as the Swift `as?` cast did: a value of another shape reads as `None` rather
/// than failing the whole entry.
fn lenient<'de, T: DeserializeOwned, D: Deserializer<'de>>(deserializer: D) -> Result<Option<T>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

/// One line of a transcript, with every field optional, since Claude Code writes many kinds of entry
/// and only some fields of each matter here.
///
/// Entry types read: `user`, `assistant`, `attachment`, `queue-operation`, `agent-name`. Ignored:
/// `last-prompt` (live entries carry only `leafUuid`; the `lastPrompt` field the Swift app once read is
/// gone), `summary`, `system`, `file-history-snapshot` and anything else.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct Entry {
    /// The entry's type: `user`, `assistant`, `attachment`, `queue-operation`, `agent-name`, ...
    #[serde(rename = "type", deserialize_with = "lenient")]
    pub kind: Option<String>,
    /// Written by a subagent rather than the session itself.
    #[serde(rename = "isSidechain", deserialize_with = "lenient")]
    pub is_sidechain: Option<bool>,
    /// A `user` entry that is not the human: a system reminder, hook output.
    #[serde(rename = "isMeta", deserialize_with = "lenient")]
    pub is_meta: Option<bool>,
    /// The entry's own id.
    #[serde(deserialize_with = "lenient")]
    pub uuid: Option<String>,
    /// When it was written, as "2026-10-15T02:48:17.095Z".
    #[serde(deserialize_with = "lenient")]
    pub timestamp: Option<String>,
    /// The model message of a `user` or `assistant` entry.
    #[serde(deserialize_with = "lenient")]
    pub message: Option<Message>,
    /// What an `attachment` entry carries.
    #[serde(deserialize_with = "lenient")]
    pub attachment: Option<Attachment>,
    /// A `queue-operation`'s operation: `enqueue`, `remove`, ...
    #[serde(deserialize_with = "lenient")]
    pub operation: Option<String>,
    /// A `queue-operation`'s queued text.
    #[serde(deserialize_with = "lenient")]
    pub content: Option<String>,
    /// An `agent-name` entry's name for the session.
    #[serde(rename = "agentName", deserialize_with = "lenient")]
    pub agent_name: Option<String>,
}

/// The model message inside a `user` or `assistant` entry.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct Message {
    /// The API message id; one reply can arrive as several entries of the same message.
    #[serde(deserialize_with = "lenient")]
    pub id: Option<String>,
    /// A string, or a list of parts (`text`, `tool_use`, `tool_result`, `image`).
    #[serde(deserialize_with = "lenient")]
    pub content: Option<Value>,
    /// The token counts of an assistant message's model call.
    #[serde(deserialize_with = "lenient")]
    pub usage: Option<Usage>,
}

/// The token counts of a model call, as Claude Code records them on an `assistant` entry.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct Usage {
    /// Fresh input tokens.
    #[serde(deserialize_with = "lenient")]
    pub input_tokens: Option<u64>,
    /// Input tokens read from the prompt cache.
    #[serde(deserialize_with = "lenient")]
    pub cache_read_input_tokens: Option<u64>,
    /// Input tokens written to the prompt cache.
    #[serde(deserialize_with = "lenient")]
    pub cache_creation_input_tokens: Option<u64>,
}

impl Usage {
    /// Everything the model call read: input, cache reads and cache writes.
    pub fn total(&self) -> u64 {
        self.input_tokens.unwrap_or(0)
            + self.cache_read_input_tokens.unwrap_or(0)
            + self.cache_creation_input_tokens.unwrap_or(0)
    }
}

/// What an `attachment` entry carries: a `queued_command` is a message taken in mid-turn.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct Attachment {
    /// The attachment's type, such as `queued_command`.
    #[serde(rename = "type", deserialize_with = "lenient")]
    pub kind: Option<String>,
    /// A queued command's prompt: a string, or parts, as a message's content.
    #[serde(deserialize_with = "lenient")]
    pub prompt: Option<Value>,
}

impl Entry {
    /// The entry on one transcript line, or `None` when the line is not a JSON object.
    pub fn parse(line: &str) -> Option<Entry> {
        let value: Value = serde_json::from_str(line).ok()?;
        if !value.is_object() {
            return None;
        }
        serde_json::from_value(value).ok()
    }

    /// Written by a subagent, so not the session speaking.
    pub fn is_sidechain(&self) -> bool {
        self.is_sidechain == Some(true)
    }

    /// A `user` entry that is not the human speaking.
    pub fn is_meta(&self) -> bool {
        self.is_meta == Some(true)
    }

    /// The message's content: a string or a list of parts.
    pub fn content(&self) -> Option<&Value> {
        self.message.as_ref()?.content.as_ref()
    }

    /// The message's content parts, or none when the content is a string or missing.
    pub fn parts(&self) -> &[Value] {
        self.content().and_then(Value::as_array).map_or(&[], Vec::as_slice)
    }

    /// The API message id.
    pub fn message_id(&self) -> Option<&str> {
        self.message.as_ref()?.id.as_deref()
    }

    /// The tokens the model call of an `assistant` entry read, or `None` for any other entry, for a
    /// subagent's, or when the total is zero: zero usage is never a count.
    pub fn usage_tokens(&self) -> Option<u64> {
        if self.kind.as_deref() != Some("assistant") || self.is_sidechain() {
            return None;
        }
        let total = self.message.as_ref()?.usage.as_ref()?.total();
        (total > 0).then_some(total)
    }
}

/// The tokens an `assistant` entry's model call read: `input_tokens`, `cache_read_input_tokens` and
/// `cache_creation_input_tokens` from `message.usage`, added up. `None` for any other entry, for a
/// subagent's (`isSidechain`), and when the total is zero, so zero never overwrites a real count.
pub fn usage_tokens(entry: &Value) -> Option<u64> {
    serde_json::from_value::<Entry>(entry.clone()).ok()?.usage_tokens()
}

/// What a session's transcript says about where it is: the message its current or last turn answers,
/// the reply to it so far, the tool call it has not had a result for (running, or waiting on a
/// permission prompt), the last command the human ran, how full it is, and what it said.
/// Read from the transcript's tail, for crew sessions, which have no state hook.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TranscriptSummary {
    /// The human's last message, or the slash command they ran.
    pub prompt: Option<String>,
    /// The transcript entry that message is, which tells two turns apart even when the human said the
    /// same thing twice ("skip", then "skip" again).
    pub prompt_id: Option<String>,
    /// The session's reply to that message, or `None` before it has said anything. For a message taken in
    /// mid-turn, everything the session said since: it answers somewhere in there and goes on with the
    /// work it was doing, and the last thing it says may be about that work rather than the message.
    pub reply: Option<String>,
    /// The message came while the session was busy with an earlier one, and was taken into that turn.
    pub mid_turn: bool,
    /// The newest tool call still without a result, in words: its description, else its command or file,
    /// else the tool's name.
    pub pending: Option<String>,
    /// The last slash command the human ran, such as "/lets-finalize-it CASBO-1025"; Claude Code's own
    /// commands (/clear, /chrome, /effort...) do not count, as they say nothing about the work's stage.
    pub command: Option<String>,
    /// Tokens the last model call read, as the assistant's hook counts them.
    pub context: Option<u64>,
    /// The newest things the session said in the tail, oldest first, for the links it gave: up to 200,
    /// cut back to the last 100 when they overflow. The reply module collects links from these.
    pub texts: Vec<String>,
}

impl TranscriptSummary {
    /// The summary of `lines`, read in order.
    pub fn read<S: AsRef<str>>(lines: impl IntoIterator<Item = S>) -> TranscriptSummary {
        let mut reader = TranscriptReader::new(false);
        reader.read(lines);
        reader.summary
    }

    /// The turn the session is on: its message's entry, else the message itself.
    pub fn turn(&self) -> Option<&str> {
        self.prompt_id.as_deref().or(self.prompt.as_deref())
    }

    /// The answer to `message` in a transcript, once its turn is over: the last thing the session said after
    /// the message, before whatever came next (the human's next message, a background task finishing, another
    /// session writing in). `None` while that turn may still be going, or when the message is not in `lines`.
    /// This finds an answer the latest turn alone would miss: a task that finished right after it starts a
    /// turn of its own, and the answer is no longer the last one.
    pub fn answer<S: AsRef<str>>(message: &str, lines: impl IntoIterator<Item = S>) -> Option<String> {
        let mut found = false;
        let mut reply: Option<String> = None;
        let mut reply_message: Option<String> = None;
        for line in lines {
            let Some(entry) = Entry::parse(line.as_ref()) else {
                continue;
            };
            if entry.is_sidechain() {
                continue;
            }
            let said: String = match entry.kind.as_deref() {
                Some("user") if !entry.is_meta() => {
                    let parts = entry.parts();
                    if parts.iter().any(|part| part_type(part) == Some("tool_result")) {
                        continue;
                    }
                    match entry.content() {
                        Some(Value::String(text)) => text.clone(),
                        Some(Value::Array(parts)) => parts
                            .iter()
                            .filter_map(|part| part.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("\n"),
                        _ => continue,
                    }
                }
                Some("attachment") => {
                    let Some(attachment) = entry.attachment.as_ref() else {
                        continue;
                    };
                    if attachment.kind.as_deref() != Some("queued_command") {
                        continue;
                    }
                    attachment
                        .prompt
                        .as_ref()
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned()
                }
                Some("assistant") if found => {
                    for part in entry.parts().iter().filter(|part| part_type(part) == Some("text")) {
                        let text = part.get("text").and_then(Value::as_str).unwrap_or("").trim();
                        if text.is_empty() {
                            continue;
                        }
                        let id = entry.message_id().map(str::to_owned);
                        reply = Some(if id.is_some() && id == reply_message {
                            format!("{}\n\n{text}", reply.unwrap_or_default())
                        } else {
                            text.to_owned()
                        });
                        reply_message = id;
                    }
                    continue;
                }
                _ => continue,
            };
            // Something new came in: it ends the message's turn, unless it is the message itself.
            if found {
                return reply;
            }
            if turn_is_for(&said, message) {
                found = true;
                reply = None;
                reply_message = None;
            }
        }
        None
    }

    /// A tool call in words: its description, else "run" and its command, else the tool and its file,
    /// else the tool's name.
    pub fn describe(call: &Value) -> String {
        let name = call.get("name").and_then(Value::as_str).unwrap_or("a tool");
        let input = call.get("input");
        let field = |key: &str| input.and_then(|input| input.get(key)).and_then(Value::as_str);
        if let Some(description) = field("description")
            && !description.is_empty()
        {
            return description.to_owned();
        }
        if let Some(command) = field("command") {
            return format!("run {}", command.graphemes(true).take(80).collect::<String>());
        }
        if let Some(path) = field("file_path") {
            return format!("{name} {}", last_path_component(path));
        }
        // An MCP tool's name is "mcp__<server>__<tool>"; say it in words.
        let words = if name.starts_with("mcp__") {
            name.split("__").skip(1).collect::<Vec<_>>().join(" ")
        } else {
            name.to_owned()
        };
        format!("use {words}")
    }
}

/// Whether `turn`, the prompt as the hook or the transcript records it, is `message`: the text alone,
/// without the pictures' "[Image #1]" markers, and cut short as the hook cuts it. Ported from
/// `SessionReport.turn(_:isFor:)` in AvatarCore/Presence.swift; the presence module should reuse it.
pub fn turn_is_for(turn: &str, message: &str) -> bool {
    crate::presence::SessionReport::turn_is_for(turn, message)
}

/// The `type` of a content part.
fn part_type(part: &Value) -> Option<&str> {
    part.get("type").and_then(Value::as_str)
}

/// A turn as history reads it. Unlike the summary's, it ends when anything wakes the session: a background
/// task finishing does not answer the human, so what the session says then is a turn of its own, with
/// no message.
#[derive(Debug, Clone, Default)]
struct OpenTurn {
    id: Option<String>,
    prompt: Option<String>,
    stamp: Option<String>,
    mid_turn: bool,
    reply: Option<String>,
    reply_message: Option<String>,
}

impl OpenTurn {
    /// The turn as history keeps it, or `None` when neither side said anything.
    fn turn(&self) -> Option<HistoryTurn> {
        if self.prompt.is_none() && self.reply.is_none() {
            return None;
        }
        Some(HistoryTurn::new(
            self.id.clone(),
            self.prompt.clone(),
            self.reply.clone(),
            HistoryTurn::date(self.stamp.as_deref()),
        ))
    }
}

/// A transcript summed up as it is read, so it can be read a piece at a time as the transcript grows.
#[derive(Debug, Clone)]
pub struct TranscriptReader {
    /// What the transcript says so far; `pending` is brought up to date by `finish`.
    pub summary: TranscriptSummary,
    /// A message from the human has been read, so `summary` knows which turn the session is on.
    pub found_turn: bool,
    reply_message: Option<String>,
    /// Tool calls without a result yet, oldest first: the call's id and what it is in words.
    calls: Vec<(String, String)>,
    /// Keeps every turn that ends, for the card's history; the live summary has no use for them.
    keeps_turns: bool,
    /// The turns that ended, oldest first, when `keeps_turns`: each the human's message and the last thing
    /// the session said before the next one, or before something else woke it.
    pub turns: Vec<HistoryTurn>,
    /// The turn history is reading, when `keeps_turns`.
    open: Option<OpenTurn>,
    /// How many turns history has opened.
    opens: usize,
}

impl Default for TranscriptReader {
    fn default() -> Self {
        Self::new(false)
    }
}

impl TranscriptReader {
    /// A reader that keeps the turns that end when `keeps_turns`, for history; the live summary does not.
    pub fn new(keeps_turns: bool) -> Self {
        Self {
            summary: TranscriptSummary::default(),
            found_turn: false,
            reply_message: None,
            calls: Vec::new(),
            keeps_turns,
            turns: Vec::new(),
            open: None,
            opens: 0,
        }
    }

    /// Reads `lines` in order and brings the summary up to date.
    pub fn read<S: AsRef<str>>(&mut self, lines: impl IntoIterator<Item = S>) {
        for line in lines {
            self.read_line(line.as_ref());
        }
        self.finish();
    }

    /// Reads one line without bringing `summary`'s pending call up to date, which `finish` does;
    /// true when the line starts a turn of history's.
    pub fn read_line(&mut self, line: &str) -> bool {
        let Some(entry) = Entry::parse(line) else { return false };
        if entry.is_sidechain() {
            return false;
        }
        let opened = self.opens;
        self.read_entry(&entry);
        self.opens != opened
    }

    /// Brings `summary.pending` up to date after a run of lines.
    pub fn finish(&mut self) {
        self.summary.pending = self.calls.last().map(|(_, what)| what.clone());
    }

    /// The turn under way, or the last one read, as history keeps it.
    pub fn current_turn(&self) -> Option<HistoryTurn> {
        self.open.as_ref().and_then(OpenTurn::turn)
    }

    /// Ends history's turn and starts the next: the human's message, or `None` for something else waking the
    /// session.
    fn open(&mut self, prompt: Option<String>, id: Option<String>, stamp: Option<String>, mid_turn: bool) {
        if !self.keeps_turns {
            return;
        }
        if let Some(ended) = self.open.as_ref().and_then(OpenTurn::turn) {
            self.turns.push(ended);
        }
        self.open = Some(OpenTurn {
            id,
            prompt,
            stamp,
            mid_turn,
            reply: None,
            reply_message: None,
        });
        self.opens += 1;
    }

    /// What the session said, as history's turn keeps it, the way `summary.reply` keeps it.
    fn said(&mut self, text: &str, id: Option<&str>) {
        if !self.keeps_turns {
            return;
        }
        let Some(turn) = self.open.as_mut() else { return };
        let joins = turn.mid_turn || (id.is_some() && id == turn.reply_message.as_deref());
        turn.reply = Some(joined_reply(turn.reply.take(), text, joins));
        turn.reply_message = id.map(str::to_owned);
    }

    fn read_entry(&mut self, entry: &Entry) {
        let stamp = entry.timestamp.clone();
        match entry.kind.as_deref() {
            Some("user") => {
                if entry.is_meta() {
                    return;
                }
                let parts = entry.parts();
                if parts.iter().any(|part| part_type(part) == Some("tool_result")) {
                    let answered: HashSet<&str> = parts
                        .iter()
                        .filter_map(|part| part.get("tool_use_id").and_then(Value::as_str))
                        .collect();
                    self.calls.retain(|(id, _)| !answered.contains(id.as_str()));
                    return;
                }
                if let Some(text) = entry.content().and_then(Value::as_str) {
                    if let Some(command) = shell_command(text)
                        && text.starts_with("<bash-input>")
                    {
                        // A shell escape: what was typed, "!" and all; its output is the answer.
                        self.begin(format!("! {command}"), entry.uuid.clone(), false, stamp);
                        return;
                    }
                    if let Some(output) = shell_output(text) {
                        let shown = if output.is_empty() {
                            "(no output)".to_owned()
                        } else {
                            last_graphemes(&output, 4000)
                        };
                        let reply = format!("```\n{shown}\n```");
                        self.summary.reply = Some(reply.clone());
                        self.reply_message = None;
                        if let Some(open) = self.open.as_mut() {
                            open.reply = Some(reply);
                            open.reply_message = None;
                        }
                        return;
                    }
                }
                let Some(prompt) = human_text(entry.content().unwrap_or(&Value::Null)) else {
                    // A background task finishing while the session is idle, or a command's output: not the human,
                    // but it wakes the session.
                    self.open(None, entry.uuid.clone(), stamp, false);
                    return;
                };
                if is_work_command(&prompt) {
                    self.summary.command = Some(prompt.clone());
                }
                self.begin(prompt, entry.uuid.clone(), false, stamp);
            }
            Some("assistant") => {
                if let Some(tokens) = entry.usage_tokens() {
                    self.summary.context = Some(tokens);
                }
                for part in entry.parts() {
                    match part_type(part) {
                        Some("text") => {
                            let text = part.get("text").and_then(Value::as_str).unwrap_or("").trim();
                            if text.is_empty() {
                                continue;
                            }
                            self.summary.texts.push(text.to_owned());
                            let count = self.summary.texts.len();
                            if count > 200 {
                                self.summary.texts.drain(..count - 100);
                            }
                            // One reply can arrive as several entries of the same message; a new message starts over,
                            // except after a message taken in mid-turn, where all of it is kept.
                            let id = entry.message_id();
                            let joins = self.summary.mid_turn || (id.is_some() && id == self.reply_message.as_deref());
                            self.summary.reply = Some(joined_reply(self.summary.reply.take(), text, joins));
                            self.reply_message = id.map(str::to_owned);
                            self.said(text, id);
                        }
                        Some("tool_use") => {
                            if let Some(id) = part.get("id").and_then(Value::as_str) {
                                self.calls.push((id.to_owned(), TranscriptSummary::describe(part)));
                            }
                        }
                        _ => continue,
                    }
                }
            }
            Some("attachment") => {
                // A message sent while the session was busy is taken in mid-turn, recorded as a queued
                // command rather than a user entry; the reply after it answers it. Subagent hand-backs and
                // task notifications are queued the same way, and are not the human speaking.
                // Taken in while the session works, they do not end the turn: what it says next still answers the
                // human, unlike a task finishing while it is idle, which wakes it.
                let Some(attachment) = entry.attachment.as_ref() else {
                    return;
                };
                if attachment.kind.as_deref() != Some("queued_command") {
                    return;
                }
                let Some(prompt) = human_text(attachment.prompt.as_ref().unwrap_or(&Value::Null)) else {
                    return;
                };
                if prompt.starts_with("Another Claude session") {
                    return;
                }
                self.begin(prompt, entry.uuid.clone(), true, stamp);
            }
            // `last-prompt` is ignored: live entries carry only `leafUuid`, nothing to read.
            _ => {}
        }
    }

    /// The human's message starts what the summary is about.
    fn begin(&mut self, prompt: String, id: Option<String>, mid_turn: bool, stamp: Option<String>) {
        self.open(Some(prompt.clone()), id.clone(), stamp, mid_turn);
        self.summary.prompt = Some(prompt);
        self.summary.prompt_id = id;
        self.summary.mid_turn = mid_turn;
        self.summary.reply = None;
        self.reply_message = None;
        self.found_turn = true;
    }
}

/// `text` added to a reply so far: appended after a blank line when `joins`, else starting over.
fn joined_reply(so_far: Option<String>, text: &str, joins: bool) -> String {
    match (joins, so_far) {
        (true, Some(reply)) => format!("{reply}\n\n{text}"),
        _ => text.to_owned(),
    }
}

/// The last `count` grapheme clusters of `text`, as Swift's `suffix` counts characters.
fn last_graphemes(text: &str, count: usize) -> String {
    let total = text.graphemes(true).count();
    text.graphemes(true).skip(total.saturating_sub(count)).collect()
}

/// How much `TranscriptFollower` reads back from the end at first, growing fourfold until the latest
/// message is in it.
pub const FIRST_TAIL: u64 = 512 * 1024;

/// The next tail to try when `tail` did not reach back to the human's latest message.
pub fn grown_tail(tail: u64) -> u64 {
    tail.saturating_mul(4)
}

/// Where a tail of `tail` bytes starts in a file of `size` bytes.
pub fn tail_start(size: u64, tail: u64) -> u64 {
    size.saturating_sub(tail)
}

/// The complete lines in `bytes`, and how many bytes they take: a line still being written waits for
/// its newline. Empty lines are left out, as the Swift split did.
pub fn whole_lines(bytes: &[u8]) -> (Vec<String>, u64) {
    let Some(end) = bytes.iter().rposition(|&byte| byte == b'\n') else {
        return (Vec::new(), 0);
    };
    let complete = &bytes[..=end];
    let text = String::from_utf8_lossy(complete);
    let lines = text
        .split('\n')
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    (lines, complete.len() as u64)
}

/// Keeps a transcript's summary up to date as the session writes it. The first time, it reads back from
/// the end only as far as the human's latest message; after that, only what was added. A turn can write
/// megabytes (screenshots, long command output), so a fixed tail would lose the message it answers.
#[derive(Debug, Clone, Default)]
pub struct TranscriptFollower {
    reader: TranscriptReader,
    /// Where the next unread line starts.
    offset: u64,
    /// The file being followed, to notice it replaced.
    file: Option<u64>,
}

impl TranscriptFollower {
    /// A follower that has read nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// What the transcript says so far.
    pub fn summary(&self) -> &TranscriptSummary {
        &self.reader.summary
    }

    /// Reads what the transcript gained since the last time. The caller opens the file and skips the
    /// call while it does not exist.
    pub fn update(&mut self, source: &dyn TranscriptSource) {
        let size = source.len();
        let identity = source.identity();
        if self.file != Some(identity) || size < self.offset {
            self.file = Some(identity);
            self.catch_up(source, size);
        } else if size > self.offset {
            let (lines, length) = whole_lines(&source.read(self.offset..size));
            self.reader.read(lines);
            self.offset += length;
        }
    }

    /// Reads from far enough back to hold the human's latest message, or the whole file.
    fn catch_up(&mut self, source: &dyn TranscriptSource, size: u64) {
        let mut tail = FIRST_TAIL;
        loop {
            let start = tail_start(size, tail);
            let (mut lines, length) = whole_lines(&source.read(start..size));
            // Starting partway into the file, the first line is the end of one cut off.
            if start > 0 && !lines.is_empty() {
                lines.remove(0);
            }
            let mut reader = TranscriptReader::new(false);
            reader.read(lines);
            if reader.found_turn || start == 0 {
                self.reader = reader;
                self.offset = start + length;
                return;
            }
            tail = grown_tail(tail);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::MemorySource;
    use serde_json::json;

    fn summary(lines: &[&str]) -> TranscriptSummary {
        TranscriptSummary::read(lines)
    }

    // TranscriptSummaryTests
    #[test]
    fn a_tool_call_without_a_result_is_what_it_is_waiting_on() {
        let lines = [
            r#"{"type":"user","message":{"content":"<command-name>/lets-do-it-v2</command-name>\n<command-args>CASBO-1077</command-args>"}}"#,
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
            r#"{"type":"assistant","message":{"id":"m2","content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"php q.php","description":"Look up Round 6581774 read-only on the dev database"}}]}}"#,
            r#"{"type":"last-prompt","lastPrompt":"/lets-do-it-v2 CASBO-1077"}"#,
        ];
        let summary = summary(&lines);
        assert_eq!(summary.prompt.as_deref(), Some("/lets-do-it-v2 CASBO-1077"));
        assert_eq!(summary.reply, None);
        assert_eq!(
            summary.pending.as_deref(),
            Some("Look up Round 6581774 read-only on the dev database")
        );
    }

    #[test]
    fn the_reply_is_the_last_messages_text_and_a_new_prompt_clears_it() {
        let lines = [
            r#"{"type":"user","message":{"content":"first question"}}"#,
            r#"{"type":"assistant","message":{"id":"a","content":[{"type":"text","text":"old answer"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"second question"}]}}"#,
            r#"{"type":"assistant","message":{"id":"b","content":[{"type":"text","text":"Part one."}]}}"#,
            r#"{"type":"assistant","message":{"id":"b","content":[{"type":"text","text":"Part two."}]}}"#,
            r#"{"type":"user","isMeta":true,"message":{"content":"<system-reminder>ignore</system-reminder>"}}"#,
            r#"{"type":"last-prompt","lastPrompt":"something else"}"#,
        ];
        let summary = summary(&lines);
        assert_eq!(summary.prompt.as_deref(), Some("second question"));
        assert_eq!(summary.reply.as_deref(), Some("Part one.\n\nPart two."));
        assert_eq!(summary.pending, None);
    }

    #[test]
    fn skips_subagents_system_text_and_garbage() {
        let lines = [
            r#"{"type":"user","message":{"content":"real question"}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"id":"s","content":[{"type":"text","text":"helper chatter"}]}}"#,
            r#"{"type":"user","message":{"content":"<local-command-stdout>noise</local-command-stdout>"}}"#,
            "not json",
            r#"{"type":"assistant","message":{"id":"r","content":[{"type":"tool_use","id":"t","name":"Edit","input":{"file_path":"/a/b/Bubble.swift"}}]}}"#,
        ];
        let summary = summary(&lines);
        assert_eq!(summary.prompt.as_deref(), Some("real question"));
        assert_eq!(summary.reply, None);
        assert_eq!(summary.pending.as_deref(), Some("Edit Bubble.swift"));
    }

    #[test]
    fn a_tool_with_nothing_to_say_is_named_in_words() {
        let call = |name: &str| {
            vec![format!(
                r#"{{"type":"assistant","message":{{"id":"m","content":[{{"type":"tool_use","id":"t","name":"{name}","input":{{}}}}]}}}}"#
            )]
        };
        assert_eq!(
            TranscriptSummary::read(call("EnterWorktree")).pending.as_deref(),
            Some("use EnterWorktree")
        );
        assert_eq!(
            TranscriptSummary::read(call("mcp__claude-in-chrome__navigate"))
                .pending
                .as_deref(),
            Some("use claude-in-chrome navigate")
        );
    }

    #[test]
    fn remembers_the_last_work_command_how_full_and_the_links() {
        let lines = [
            r#"{"type":"user","message":{"content":"<command-name>/lets-do-it-v2</command-name>\n<command-args>CASBO-1044</command-args>"}}"#,
            r#"{"type":"assistant","message":{"id":"a","usage":{"input_tokens":3,"cache_read_input_tokens":90000,"cache_creation_input_tokens":2000,"output_tokens":500},"content":[{"type":"text","text":"Draft PRs: https://github.com/DWAsia/v2-game-center-frontend-casino-admin/pull/256 and https://github.com/DWAsia/v2-game-center-backend-casino-admin-api/pull/124."}]}}"#,
            r#"{"type":"user","message":{"content":"<command-name>/lets-finalize-it</command-name>"}}"#,
            r#"{"type":"user","message":{"content":"<command-name>/chrome</command-name>"}}"#,
            r#"{"type":"user","message":{"content":"tab is up"}}"#,
            r#"{"type":"assistant","message":{"id":"b","usage":{"input_tokens":1,"cache_read_input_tokens":120000,"cache_creation_input_tokens":3000},"content":[{"type":"text","text":"Open http://localhost:20300/login, then reply.\n\nReply: `logged in`"}]}}"#,
        ];
        let summary = summary(&lines);
        assert_eq!(summary.prompt.as_deref(), Some("tab is up"));
        assert_eq!(summary.command.as_deref(), Some("/lets-finalize-it"));
        assert_eq!(summary.context, Some(123_001));
        // The links are collected from these by the reply module.
        assert_eq!(
            summary.texts,
            vec![
                "Draft PRs: https://github.com/DWAsia/v2-game-center-frontend-casino-admin/pull/256 and https://github.com/DWAsia/v2-game-center-backend-casino-admin-api/pull/124.",
                "Open http://localhost:20300/login, then reply.\n\nReply: `logged in`",
            ]
        );
    }

    #[test]
    fn a_command_only_in_the_last_prompt_is_ignored() {
        // The Swift app read `last-prompt.lastPrompt`; live entries carry only `leafUuid`, so the entry is ignored.
        let lines = [r#"{"type":"last-prompt","lastPrompt":"/lets-commit-it CASBO-1056"}"#];
        assert_eq!(summary(&lines).command, None);
        assert_eq!(summary(&lines).prompt, None);
        assert_eq!(summary(&[r#"{"type":"last-prompt","leafUuid":"abc"}"#]).command, None);
    }

    #[test]
    fn zero_usage_is_not_a_count() {
        let zero = json!({"type":"assistant","message":{"usage":{"input_tokens":0,"cache_read_input_tokens":0}}});
        assert_eq!(usage_tokens(&zero), None);
        let some = json!({"type":"assistant","message":{"usage":{"input_tokens":1,"cache_read_input_tokens":2,"cache_creation_input_tokens":3}}});
        assert_eq!(usage_tokens(&some), Some(6));
        let side = json!({"type":"assistant","isSidechain":true,"message":{"usage":{"input_tokens":5}}});
        assert_eq!(usage_tokens(&side), None);
        let user = json!({"type":"user","message":{"usage":{"input_tokens":5}}});
        assert_eq!(usage_tokens(&user), None);
        // Zero never overwrites a real count in the reader.
        let lines = [
            r#"{"type":"assistant","message":{"id":"a","usage":{"input_tokens":10},"content":[]}}"#,
            r#"{"type":"assistant","message":{"id":"b","usage":{"input_tokens":0},"content":[]}}"#,
        ];
        assert_eq!(summary(&lines).context, Some(10));
    }

    #[test]
    fn entries_are_lenient_about_field_shapes() {
        let entry = Entry::parse(r#"{"type":"user","isMeta":"yes","uuid":7,"message":{"content":"hi"}}"#).unwrap();
        assert_eq!(entry.kind.as_deref(), Some("user"));
        assert!(!entry.is_meta());
        assert_eq!(entry.uuid, None);
        assert_eq!(entry.content().and_then(Value::as_str), Some("hi"));
        assert!(Entry::parse("[1,2]").is_none());
        assert!(Entry::parse("not json").is_none());
    }

    // ShellEscapeTests (the summary half; the delivery half is in delivery.rs)
    const SHELL_LINES: [&str; 2] = [
        r#"{"type":"user","message":{"role":"user","content":"<bash-input> bash scripts/destructive-scan.sh --repo /w --out /t/dscan.json</bash-input>"}}"#,
        r#"{"type":"user","message":{"role":"user","content":"<bash-stdout></bash-stdout><bash-stderr>destructive scan: flag, 1 item(s) -&gt; /t/dscan.json</bash-stderr>"}}"#,
    ];

    #[test]
    fn its_output_is_the_answer() {
        let summary = summary(&SHELL_LINES);
        assert_eq!(
            summary.prompt.as_deref(),
            Some("! bash scripts/destructive-scan.sh --repo /w --out /t/dscan.json")
        );
        assert_eq!(
            summary.reply.as_deref(),
            Some("```\ndestructive scan: flag, 1 item(s) -> /t/dscan.json\n```")
        );
        assert!(turn_is_for(
            summary.prompt.as_deref().unwrap(),
            "! bash scripts/destructive-scan.sh --repo /w --out /t/dscan.json"
        ));
    }

    #[test]
    fn a_silent_command_says_so() {
        let quiet = [
            r#"{"type":"user","message":{"content":"<bash-input>true</bash-input>"}}"#,
            r#"{"type":"user","message":{"content":"<bash-stdout></bash-stdout><bash-stderr></bash-stderr>"}}"#,
        ];
        assert_eq!(summary(&quiet).reply.as_deref(), Some("```\n(no output)\n```"));
    }

    // QueuedMessageTests
    #[test]
    fn a_message_taken_in_mid_turn_is_the_prompt_the_next_reply_answers() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"Small, urgent UI fix"}}"#,
            r#"{"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"Checking the database."}]}}"#,
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"pabuo nalang na \"Rollback\" sa dropdown"}}"#,
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"<agent-message from=\"a1\">[Subagent hand-back] done</agent-message>"}}"#,
            r#"{"type":"attachment","attachment":{"type":"queued_command","prompt":"<task-notification><task-id>a1</task-id></task-notification>"}}"#,
            r#"{"type":"assistant","message":{"id":"m2","content":[{"type":"text","text":"Applying the three changes."}]}}"#,
        ];
        let summary = summary(&lines);
        assert_eq!(
            summary.prompt.as_deref(),
            Some(r#"pabuo nalang na "Rollback" sa dropdown"#)
        );
        assert_eq!(summary.reply.as_deref(), Some("Applying the three changes."));
    }

    // TranscriptAnswerTests
    fn line(object: Value) -> String {
        object.to_string()
    }

    fn user(text: &str) -> String {
        line(json!({"type": "user", "message": {"content": text}}))
    }

    fn said(text: &str, id: &str) -> String {
        line(json!({"type": "assistant", "message": {"id": id, "content": [{"type": "text", "text": text}]}}))
    }

    fn tool_call() -> String {
        line(
            json!({"type": "assistant", "message": {"id": "m1", "content": [{"type": "tool_use", "id": "t1", "name": "Bash"}]}}),
        )
    }

    fn tool_result() -> String {
        line(json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1"}]}}))
    }

    fn queued() -> String {
        line(
            json!({"type": "attachment", "uuid": "q1", "attachment": {"type": "queued_command", "prompt": "did we finish it"}}),
        )
    }

    #[test]
    fn a_task_that_finishes_right_after_does_not_hide_the_answer() {
        // As it happened: the answer, then a background task's notification starts a turn of its own.
        let lines = [
            user("Run sleep 20 in the shell"),
            said("Waiting the 20 seconds.", "m0"),
            user("work on this CASBO-1107"),
            tool_call(),
            tool_result(),
            said("Human, I started CASBO-1107 as crew.", "m2"),
            line(json!({"type": "queue-operation", "operation": "enqueue"})),
            user("<task-notification> <task-id>bbb0hlz47</task-id></task-notification>"),
            said("done", "m3"),
        ];
        assert_eq!(
            TranscriptSummary::answer("work on this CASBO-1107", &lines).as_deref(),
            Some("Human, I started CASBO-1107 as crew.")
        );
    }

    #[test]
    fn no_answer_while_its_turn_may_still_be_going() {
        let lines = [
            user("work on this CASBO-1107"),
            tool_call(),
            said("Looking at the ticket.", "m2"),
        ];
        assert_eq!(TranscriptSummary::answer("work on this CASBO-1107", &lines), None);
        assert_eq!(
            TranscriptSummary::answer("something never sent", [user("hi"), said("hello", "a"), user("bye")]),
            None
        );
    }

    #[test]
    fn a_message_taken_in_mid_turn_counts() {
        let lines = [
            user("first"),
            tool_call(),
            line(json!({"type": "attachment", "attachment": {"type": "queued_command", "prompt": "and also this"}})),
            tool_result(),
            said("Both done.", "m5"),
            user("next one"),
        ];
        assert_eq!(
            TranscriptSummary::answer("and also this", &lines).as_deref(),
            Some("Both done.")
        );
    }

    #[test]
    fn a_message_taken_in_mid_turn_keeps_everything_said_since() {
        let lines = vec![
            user("whats the pr link"),
            tool_call(),
            queued(),
            said("Tab underline fixed.", "m2"),
            tool_result(),
            said("The code updates are finished, but it is not finalized yet.", "m3"),
            said("Back to the top-bar fix.", "m4"),
        ];
        let summary = TranscriptSummary::read(&lines);
        assert_eq!(summary.prompt.as_deref(), Some("did we finish it"));
        assert_eq!(summary.turn(), Some("q1"));
        assert!(summary.mid_turn);
        assert_eq!(
            summary.reply.as_deref(),
            Some(
                "Tab underline fixed.\n\nThe code updates are finished, but it is not finalized yet.\n\nBack to the top-bar fix."
            )
        );
        let mut more = lines.clone();
        more.extend([
            user("thanks"),
            said("You're welcome.", "m5"),
            said("Anything else?", "m6"),
        ]);
        let next = TranscriptSummary::read(&more);
        assert!(!next.mid_turn);
        assert_eq!(next.reply.as_deref(), Some("Anything else?"));
    }

    #[test]
    fn the_follower_keeps_the_message_a_megabytes_long_turn_answers() {
        // A screenshot's worth of tool output, many times over: far more than the first tail read back.
        let bulk = line(json!({
            "type": "user",
            "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": "x".repeat(300_000)}]},
        }));
        let mut lines = vec![
            user("whats the pr link"),
            tool_call(),
            queued(),
            said("Finished, not finalized.", "m3"),
        ];
        lines.extend(std::iter::repeat_n(bulk, 8));
        lines.push(tool_call());
        let mut source = MemorySource::new(1, format!("{}\n", lines.join("\n")).into_bytes());
        assert_ne!(
            TranscriptSummary::read(&lines[lines.len() - 3..]).prompt.as_deref(),
            Some("did we finish it")
        );

        let mut follower = TranscriptFollower::new();
        follower.update(&source);
        assert_eq!(follower.summary().prompt.as_deref(), Some("did we finish it"));
        assert_eq!(follower.summary().reply.as_deref(), Some("Finished, not finalized."));
        assert_eq!(follower.summary().pending.as_deref(), Some("use Bash"));

        // Only what is added is read; a line still being written waits for its newline.
        let next = said("Both PRs are up.", "m9");
        let cut = next.len() - 5;
        source.bytes.extend_from_slice(&next.as_bytes()[..cut]);
        follower.update(&source);
        assert_eq!(follower.summary().reply.as_deref(), Some("Finished, not finalized."));
        source.bytes.extend_from_slice(&next.as_bytes()[cut..]);
        source.bytes.push(b'\n');
        follower.update(&source);
        assert_eq!(
            follower.summary().reply.as_deref(),
            Some("Finished, not finalized.\n\nBoth PRs are up.")
        );
    }

    #[test]
    fn whole_lines_wait_for_the_newline() {
        let (lines, length) = whole_lines(b"a\n\nb\nhalf");
        assert_eq!(lines, vec!["a", "b"]);
        assert_eq!(length, 5);
        assert_eq!(whole_lines(b"half"), (Vec::new(), 0));
        assert_eq!(tail_start(100, FIRST_TAIL), 0);
        assert_eq!(tail_start(600_000, FIRST_TAIL), 600_000 - FIRST_TAIL);
        assert_eq!(grown_tail(FIRST_TAIL), 4 * FIRST_TAIL);
    }

    #[test]
    fn a_turn_is_for_its_message_as_the_hook_records_it() {
        assert!(turn_is_for("hello there", "hello there friend"));
        assert!(turn_is_for("hello th…", "hello there"));
        assert!(turn_is_for("[Image #1]", ""));
        assert!(!turn_is_for("", ""));
        assert!(turn_is_for("<pasted_content id=\"1\"> a long brief", "a long brief"));
        assert!(!turn_is_for("other", "hello"));
    }
}
