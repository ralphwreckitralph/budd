//! Paging back through a session's conversation across its transcripts (from AvatarCore/History.swift).
//!
//! No file is opened here: a transcript is read through a `TranscriptSource`, and the set of a
//! session's transcripts comes from a `TranscriptStore` the app implements over the file system.

use crate::delivery::shortened;
use crate::text::without_image_markers;
use crate::transcript::{Entry, TranscriptReader, whole_lines};
use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

/// One exchange in a session's history: the human's message and the last thing the session said to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTurn {
    /// The transcript entry the message is, which tells two turns apart even when the human said the same
    /// thing twice; `None` for an entry without one.
    pub id: Option<String>,
    /// `None` when something other than the human woke the session, such as a background task finishing.
    pub prompt: Option<String>,
    /// `None` when the session said nothing before the next message.
    pub reply: Option<String>,
    /// When the message came, in milliseconds since the Unix epoch, UTC.
    pub at: Option<i64>,
    /// The first turn of its transcript: the session started afresh here, so what came before it is not in
    /// its context.
    pub opens_session: bool,
}

impl HistoryTurn {
    /// A turn that does not open a session.
    pub fn new(id: Option<String>, prompt: Option<String>, reply: Option<String>, at: Option<i64>) -> Self {
        Self {
            id,
            prompt,
            reply,
            at,
            opens_session: false,
        }
    }

    /// A transcript's timestamp ("2026-10-15T02:48:17.095Z"), with or without fractional seconds, as
    /// milliseconds since the Unix epoch; `None` for anything else.
    pub fn date(stamp: Option<&str>) -> Option<i64> {
        parse_iso8601(stamp?)
    }

    /// The turns before the last one that is the card's (`shown`, answered `reply`), and that one; all of
    /// them when none is. Without a message, the card's is a turn the session spoke in on its own, known by
    /// what it said.
    pub fn turns(
        turns: &[HistoryTurn],
        shown: Option<&str>,
        reply: Option<&str>,
    ) -> (Vec<HistoryTurn>, Option<HistoryTurn>) {
        let index = if let Some(shown) = shown {
            turns.iter().rposition(|turn| turn.is_shown(shown))
        } else if let Some(reply) = reply {
            turns
                .iter()
                .rposition(|turn| turn.prompt.is_none() && turn.reply.as_deref() == Some(reply))
        } else {
            None
        };
        match index {
            Some(index) => (turns[..index].to_vec(), Some(turns[index].clone())),
            None => (turns.to_vec(), None),
        }
    }

    /// Whether this turn is the one a card shows as `shown`: the message as the card has it, which may be
    /// the hook's shortened copy, carry "[report.pdf]" for a file or lack a picture's "[Image #1]".
    pub fn is_shown(&self, shown: &str) -> bool {
        let words = |text: &str| {
            let plain = without_image_markers(&shortened(text));
            let joined = plain.split_whitespace().collect::<Vec<_>>().join(" ");
            joined.strip_suffix('…').map_or(joined.clone(), str::to_owned)
        };
        let Some(prompt) = self.prompt.as_deref() else {
            return false;
        };
        let (mine, theirs) = (words(prompt), words(shown));
        if mine.is_empty() || theirs.is_empty() {
            return mine == theirs;
        }
        mine.starts_with(&theirs) || theirs.starts_with(&mine)
    }
}

/// `YYYY-MM-DDTHH:MM:SS`, optional `.fraction`, then `Z` or an `+HH:MM` / `+HHMM` offset, as milliseconds
/// since the Unix epoch.
fn parse_iso8601(stamp: &str) -> Option<i64> {
    let bytes = stamp.as_bytes();
    let digits = |range: Range<usize>| -> Option<i64> {
        let slice = bytes.get(range)?;
        if slice.is_empty() || !slice.iter().all(u8::is_ascii_digit) {
            return None;
        }
        std::str::from_utf8(slice).ok()?.parse().ok()
    };
    let separator = |index: usize, expected: &[u8]| bytes.get(index).is_some_and(|b| expected.contains(b));
    if !(separator(4, b"-") && separator(7, b"-") && separator(10, b"Tt") && separator(13, b":") && separator(16, b":"))
    {
        return None;
    }
    let (year, month, day) = (digits(0..4)?, digits(5..7)?, digits(8..10)?);
    let (hour, minute, second) = (digits(11..13)?, digits(14..16)?, digits(17..19)?);
    let mut index = 19;
    let mut millis = 0;
    if separator(index, b".,") {
        let start = index + 1;
        let mut end = start;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        if end == start {
            return None;
        }
        let fraction = &stamp[start..end];
        let padded = format!("{:0<3}", &fraction[..fraction.len().min(3)]);
        millis = padded.parse().ok()?;
        index = end;
    }
    let offset_seconds = match bytes.get(index) {
        Some(b'Z' | b'z') if index + 1 == bytes.len() => 0,
        Some(sign @ (b'+' | b'-')) => {
            let rest = &stamp[index + 1..];
            let (hours, minutes) = match rest.len() {
                5 if rest.as_bytes()[2] == b':' => (rest[..2].parse::<i64>().ok()?, rest[3..].parse::<i64>().ok()?),
                4 => (rest[..2].parse::<i64>().ok()?, rest[2..].parse::<i64>().ok()?),
                2 => (rest.parse::<i64>().ok()?, 0),
                _ => return None,
            };
            let total = hours * 3600 + minutes * 60;
            if *sign == b'+' { total } else { -total }
        }
        _ => return None,
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset_seconds;
    Some(seconds * 1000 + millis)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// A transcript's bytes, read by range: a file on disk in the app, a buffer in tests.
pub trait TranscriptSource {
    /// How many bytes the transcript holds now.
    fn len(&self) -> u64;
    /// Whether the transcript is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// The bytes in `range`, cut short at the end of the transcript.
    fn read(&self, range: Range<u64>) -> Vec<u8>;
    /// What tells this transcript from one that replaced it at the same path (an inode number on disk).
    fn identity(&self) -> u64;
}

impl<T: TranscriptSource + ?Sized> TranscriptSource for &T {
    fn len(&self) -> u64 {
        (**self).len()
    }

    fn read(&self, range: Range<u64>) -> Vec<u8> {
        (**self).read(range)
    }

    fn identity(&self) -> u64 {
        (**self).identity()
    }
}

/// A transcript held in memory, for tests and previews.
#[derive(Debug, Clone, Default)]
pub struct MemorySource {
    /// The transcript's bytes; append to them to write more lines.
    pub bytes: Vec<u8>,
    /// Stands in for the file's inode number.
    pub identity: u64,
}

impl MemorySource {
    /// A transcript with `identity` holding `bytes`.
    pub fn new(identity: u64, bytes: Vec<u8>) -> Self {
        Self { bytes, identity }
    }
}

impl TranscriptSource for MemorySource {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn read(&self, range: Range<u64>) -> Vec<u8> {
        let end = (range.end.min(self.len())) as usize;
        let start = (range.start as usize).min(end);
        self.bytes[start..end].to_vec()
    }

    fn identity(&self) -> u64 {
        self.identity
    }
}

/// A run of turns read back from a point in a transcript: from the human's message at `start` up to `end`.
#[derive(Debug, Clone)]
pub struct TranscriptPage {
    /// Fed the page's lines, its turns included.
    pub reader: TranscriptReader,
    /// Where the page's first message starts, or 0 when it reaches back to the start of the file.
    pub start: u64,
    /// Where the page's last whole line ends.
    pub end: u64,
}

impl TranscriptPage {
    /// How much is read back at first, growing fourfold until it holds the messages asked for.
    pub const FIRST_TAIL: u64 = 256 * 1024;

    /// The page's turns, oldest first, the one under way included; the first opens the session when the
    /// page reaches the start of the file.
    pub fn turns(&self) -> Vec<HistoryTurn> {
        let mut turns = self.reader.turns.clone();
        turns.extend(self.reader.current_turn());
        if self.start == 0
            && let Some(first) = turns.first_mut()
        {
            first.opens_session = true;
        }
        turns
    }

    /// The turns that end at `end` (the end of the transcript when `None`), at least `minimum` of them unless
    /// the file starts sooner. `end` is where a message starts, or the end of the file, so no turn is cut in two.
    pub fn read(source: &dyn TranscriptSource, end: Option<u64>, minimum: usize, tail: u64) -> TranscriptPage {
        let size = source.len();
        let end = end.unwrap_or(size).min(size);
        let mut tail = tail;
        loop {
            let start = end.saturating_sub(tail);
            let data = source.read(start..end);
            let mut reader = TranscriptReader::new(true);
            let mut messages: Vec<u64> = Vec::new();
            let mut offset = start;
            let mut consumed: u64 = 0;
            let mut first = start > 0;
            // Whole lines only: one cut off at the start belongs to the page before, one still being
            // written at the end waits for its newline.
            let mut lines: Vec<&[u8]> = data.split(|&byte| byte == b'\n').collect();
            lines.pop();
            for line in lines {
                let line_start = offset;
                offset += line.len() as u64 + 1;
                consumed = offset - start;
                if first {
                    first = false;
                    continue;
                }
                if reader.read_line(&String::from_utf8_lossy(line)) {
                    messages.push(line_start);
                }
            }
            if messages.len() >= minimum || start == 0 {
                reader.finish();
                let page_start = if start == 0 {
                    0
                } else {
                    messages.first().copied().unwrap_or(end)
                };
                return TranscriptPage {
                    reader,
                    start: page_start,
                    end: start + consumed,
                };
            }
            tail = tail.saturating_mul(4);
        }
    }
}

/// A transcript file the caller found under `<config>/projects/*/*.jsonl`, for `TranscriptChain::earlier`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The file's path.
    pub path: String,
    /// When the file was created, as a comparable moment (milliseconds since the epoch will do).
    pub created_at: i64,
    /// The first 64 KiB of the file, where its `agent-name` entry is.
    pub head: Vec<u8>,
}

/// The other transcripts of a session by its name: those a keep-alive refresh or a clear left behind.
pub struct TranscriptChain;

impl TranscriptChain {
    /// How much of a transcript's head the caller reads for its name.
    pub const HEAD_BYTES: usize = 64 * 1024;

    /// Every candidate whose session carries `name`, apart from `current` and anything created after it,
    /// oldest first. The caller scans `<config>/projects/*/*.jsonl` for the candidates.
    pub fn earlier(name: &str, current: Option<&str>, candidates: &[Candidate]) -> Vec<String> {
        let current_start = current
            .and_then(|current| candidates.iter().find(|c| c.path == current))
            .map(|c| c.created_at);
        let mut found: Vec<(&str, i64)> = candidates
            .iter()
            .filter(|candidate| Some(candidate.path.as_str()) != current)
            .filter(|candidate| current_start.is_none_or(|start| candidate.created_at <= start))
            .filter(|candidate| Self::name_of(&candidate.head).as_deref() == Some(name))
            .map(|candidate| (candidate.path.as_str(), candidate.created_at))
            .collect();
        found.sort_by_key(|&(_, at)| at);
        found.into_iter().map(|(path, _)| path.to_owned()).collect()
    }

    /// The session's name, from the `agent-name` entry Claude Code writes near the top of its transcript.
    pub fn name_of(head: &[u8]) -> Option<String> {
        let text = String::from_utf8_lossy(head);
        for line in text.split('\n').filter(|line| line.contains("\"agent-name\"")) {
            let Some(entry) = Entry::parse(line) else { continue };
            if entry.kind.as_deref() != Some("agent-name") {
                continue;
            }
            return entry.agent_name;
        }
        None
    }
}

/// Where a session's transcripts come from: the current one by path, and every transcript file that
/// might be an earlier one of the same session.
pub trait TranscriptStore {
    /// The transcript at `path`, or `None` while it does not exist.
    fn open(&self, path: &str) -> Option<Box<dyn TranscriptSource + '_>>;
    /// Every `<config>/projects/*/*.jsonl`, with when it was created and its head.
    fn candidates(&self) -> Vec<Candidate>;
}

/// One transcript in a `MemoryStore`.
#[derive(Debug, Clone, Default)]
struct MemoryFile {
    source: MemorySource,
    created_at: i64,
}

/// Transcripts held in memory, keyed by path, for tests and previews.
#[derive(Debug, Clone, Default)]
pub struct MemoryStore {
    files: BTreeMap<String, MemoryFile>,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes a transcript at `path`, created at `created_at`, replacing one that was there.
    pub fn write(&mut self, path: &str, created_at: i64, bytes: Vec<u8>) {
        let identity = self.files.len() as u64 + 1;
        self.files.insert(
            path.to_owned(),
            MemoryFile {
                source: MemorySource::new(identity, bytes),
                created_at,
            },
        );
    }

    /// Appends to the transcript at `path`, which must exist.
    pub fn append(&mut self, path: &str, bytes: &[u8]) {
        if let Some(file) = self.files.get_mut(path) {
            file.source.bytes.extend_from_slice(bytes);
        }
    }
}

impl TranscriptStore for MemoryStore {
    fn open(&self, path: &str) -> Option<Box<dyn TranscriptSource + '_>> {
        self.files
            .get(path)
            .map(|file| Box::new(&file.source) as Box<dyn TranscriptSource + '_>)
    }

    fn candidates(&self) -> Vec<Candidate> {
        self.files
            .iter()
            .map(|(path, file)| Candidate {
                path: path.clone(),
                created_at: file.created_at,
                head: file
                    .source
                    .bytes
                    .iter()
                    .take(TranscriptChain::HEAD_BYTES)
                    .copied()
                    .collect(),
            })
            .collect()
    }
}

/// What a card can scroll back through: the session's turns as its transcript has them, newest last, and
/// on request the ones before, page by page, back through the transcripts it had before a refresh or a
/// clear. The newest page is followed as the transcript grows.
#[derive(Debug, Clone)]
pub struct ConversationHistory {
    /// The session's transcript now; it may not exist yet, before a fresh session's first message.
    pub current: String,
    /// The session's name, which its earlier transcripts carry too.
    pub name: String,
    /// Turns paged in from before the newest page, oldest first.
    older: Vec<HistoryTurn>,
    /// Follows `current` from its newest page on.
    live: TranscriptReader,
    live_start: u64,
    offset: u64,
    /// The file being followed, to notice it replaced; `None` until it is first read.
    file: Option<u64>,
    /// The older transcripts of the session, oldest first, looked up the first time they are needed.
    earlier: Option<Vec<String>>,
    /// Where the next older page ends: a file and an offset in it, `None` for its end; `None` when all is in.
    cursor: Option<(String, Option<u64>)>,
    /// The first read has happened.
    pub started: bool,
}

impl ConversationHistory {
    /// How many messages a page holds at least.
    pub const PAGE_TURNS: usize = 10;

    /// A history of the session named `name` whose transcript is `current`, before anything is read.
    pub fn new(current: &str, name: &str) -> Self {
        Self {
            current: current.to_owned(),
            name: name.to_owned(),
            older: Vec::new(),
            live: TranscriptReader::new(true),
            live_start: 0,
            offset: 0,
            file: None,
            earlier: None,
            cursor: None,
            started: false,
        }
    }

    /// Every turn read so far, oldest first; the last may still be under way.
    pub fn turns(&self) -> Vec<HistoryTurn> {
        let mut newest = self.live.turns.clone();
        newest.extend(self.live.current_turn());
        if self.file.is_some()
            && self.live_start == 0
            && let Some(first) = newest.first_mut()
        {
            first.opens_session = true;
        }
        let mut all = self.older.clone();
        all.extend(newest);
        all
    }

    /// There is more to page in before the first turn.
    pub fn has_earlier(&self) -> bool {
        !self.started || self.cursor.is_some()
    }

    /// The turns before the one the card shows as `shown` (the last turn that is it, searching back), or
    /// every turn when none is: the card's message may not be in the transcript yet.
    pub fn turns_before(&self, shown: Option<&str>, reply: Option<&str>) -> (Vec<HistoryTurn>, Option<HistoryTurn>) {
        HistoryTurn::turns(&self.turns(), shown, reply)
    }

    /// Reads what `current` gained since the last time; the first time, its newest page, and from the
    /// transcripts before it as much as that page lacks, so a session fresh from a refresh still shows its past.
    pub fn update(&mut self, store: &dyn TranscriptStore) {
        let first = !self.started;
        self.read(store);
        if first && self.turns().len() < Self::PAGE_TURNS {
            self.load_earlier(store);
        }
    }

    fn read(&mut self, store: &dyn TranscriptStore) {
        let Some(source) = store.open(&self.current) else {
            if !self.started {
                self.started = true;
                self.cursor = None;
                self.move_to_earlier_file(None, store);
            }
            return;
        };
        let identity = source.identity();
        let size = source.len();
        if self.file.is_some() && (self.file != Some(identity) || size < self.offset) {
            // Replaced or cut back: start over.
            *self = Self::new(&self.current, &self.name);
        }
        if self.file.is_none() {
            self.file = Some(identity);
            if self.started {
                // A fresh session's first message created it: all of it is new, after what is paged in.
                self.follow(&*source, 0, size);
            } else {
                self.started = true;
                let page = TranscriptPage::read(&*source, None, Self::PAGE_TURNS, TranscriptPage::FIRST_TAIL);
                self.live = page.reader;
                self.live_start = page.start;
                self.offset = page.end;
                if page.start > 0 {
                    self.cursor = Some((self.current.clone(), Some(page.start)));
                } else {
                    let current = self.current.clone();
                    self.move_to_earlier_file(Some(&current), store);
                }
            }
        } else if size > self.offset {
            self.follow(&*source, self.offset, size);
        }
    }

    fn follow(&mut self, source: &dyn TranscriptSource, start: u64, size: u64) {
        let (lines, length) = whole_lines(&source.read(start..size));
        for line in &lines {
            self.live.read_line(line);
        }
        self.offset = start + length;
    }

    /// Pages in the turns before the first one read, at least `PAGE_TURNS` unless the history starts sooner.
    pub fn load_earlier(&mut self, store: &dyn TranscriptStore) {
        let mut gained: Vec<HistoryTurn> = Vec::new();
        let already = if self.older.is_empty() { self.turns().len() } else { 0 };
        let wanted = Self::PAGE_TURNS.saturating_sub(already).max(1);
        while gained.len() < wanted {
            let Some((path, end)) = self.cursor.clone() else { break };
            let Some(source) = store.open(&path) else {
                self.move_to_earlier_file(Some(&path), store);
                continue;
            };
            let page = TranscriptPage::read(&*source, end, wanted - gained.len(), TranscriptPage::FIRST_TAIL);
            let mut turns = page.turns();
            turns.append(&mut gained);
            gained = turns;
            if page.start > 0 {
                self.cursor = Some((path, Some(page.start)));
            } else {
                self.move_to_earlier_file(Some(&path), store);
            }
        }
        // A session resumed in another folder can carry its turns into both files.
        let known: HashSet<String> = self.turns().into_iter().filter_map(|turn| turn.id).collect();
        let mut older: Vec<HistoryTurn> = gained
            .into_iter()
            .filter(|turn| turn.id.as_ref().is_none_or(|id| !known.contains(id)))
            .collect();
        older.append(&mut self.older);
        self.older = older;
    }

    /// Points the cursor at the end of the transcript before `path` (`None`: before them all), or at nothing.
    fn move_to_earlier_file(&mut self, path: Option<&str>, store: &dyn TranscriptStore) {
        let earlier = match self.earlier.take() {
            Some(earlier) => earlier,
            None => TranscriptChain::earlier(&self.name, Some(&self.current), &store.candidates()),
        };
        let index = path
            .and_then(|path| earlier.iter().position(|p| p == path))
            .unwrap_or(earlier.len());
        self.cursor = if index > 0 {
            Some((earlier[index - 1].clone(), None))
        } else {
            None
        };
        self.earlier = Some(earlier);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::TranscriptSummary;
    use serde_json::{Value, json};

    fn line(object: Value) -> String {
        object.to_string()
    }

    fn user(text: &str, uuid: &str, stamp: &str) -> String {
        line(json!({"type": "user", "uuid": uuid, "timestamp": stamp, "message": {"content": text}}))
    }

    fn said(text: &str, id: &str, stamp: &str) -> String {
        line(
            json!({"type": "assistant", "timestamp": stamp, "message": {"id": id, "content": [{"type": "text", "text": text}]}}),
        )
    }

    /// `count` exchanges, "message N" answered "answer N", each with some tool output in between.
    fn exchanges(count: usize, first: usize, padding: usize) -> Vec<String> {
        (first..first + count)
            .flat_map(|n| {
                [
                    user(&format!("message {n}"), &format!("u{n}"), &format!("2026-10-14T01:{:02}:00.000Z", n % 60)),
                    line(json!({
                        "type": "user",
                        "message": {"content": [{"type": "tool_result", "tool_use_id": "t", "content": "x".repeat(padding)}]},
                    })),
                    said(&format!("answer {n}"), &format!("m{n}"), &format!("2026-10-14T01:{:02}:30.000Z", n % 60)),
                ]
            })
            .collect()
    }

    fn file(lines: &[String]) -> Vec<u8> {
        format!("{}\n", lines.join("\n")).into_bytes()
    }

    fn prompts(turns: &[HistoryTurn]) -> Vec<Option<&str>> {
        turns.iter().map(|turn| turn.prompt.as_deref()).collect()
    }

    #[test]
    fn the_reader_keeps_each_turns_last_word_and_when_it_was_said() {
        let mut reader = TranscriptReader::new(true);
        let mut lines = exchanges(2, 1, 2_000);
        lines.push(said("and one more thing", "m2b", "2026-10-14T01:02:45.000Z"));
        reader.read(&lines);
        assert_eq!(prompts(&reader.turns), vec![Some("message 1")]);
        assert_eq!(reader.turns[0].reply.as_deref(), Some("answer 1"));
        assert_eq!(reader.turns[0].id.as_deref(), Some("u1"));
        assert_eq!(reader.turns[0].at, HistoryTurn::date(Some("2026-10-14T01:01:00.000Z")));
        assert!(reader.turns[0].at.is_some());
        // The turn under way is not kept yet, but can be read as one.
        let current = reader.current_turn().unwrap();
        assert_eq!(current.reply.as_deref(), Some("and one more thing"));
        assert_eq!(current.at, HistoryTurn::date(Some("2026-10-14T01:02:00.000Z")));
        // The live summary keeps nothing.
        let mut live = TranscriptReader::new(false);
        live.read(exchanges(3, 1, 2_000));
        assert!(live.turns.is_empty());
    }

    #[test]
    fn a_background_task_finishing_does_not_answer_the_human() {
        let mut reader = TranscriptReader::new(true);
        let notification = "<task-notification>\n<task-id>b1</task-id>\n<summary>done</summary>\n</task-notification>";
        let lines = [
            user("give me the server file", "u1", "2026-10-14T01:00:00.000Z"),
            said("It is backend/server.js.", "m1", "2026-10-14T01:00:05.000Z"),
            user(notification, "n1", "2026-10-14T01:05:00.000Z"),
            said(
                "That alert was the transcription job ending.",
                "m2",
                "2026-10-14T01:05:03.000Z",
            ),
            user("now deploy it", "u2", "2026-10-14T01:06:00.000Z"),
            said("Deploying.", "m3", "2026-10-14T01:06:05.000Z"),
            // One taken in while it works is part of that work: what it says next still answers the human.
            line(
                json!({"type": "attachment", "uuid": "n2", "attachment": {"type": "queued_command", "prompt": notification}}),
            ),
            said("Deployed to staging.", "m4", "2026-10-14T01:07:00.000Z"),
            user("thanks", "u3", "2026-10-14T01:08:00.000Z"),
        ];
        reader.read(&lines);
        assert_eq!(
            prompts(&reader.turns),
            vec![Some("give me the server file"), None, Some("now deploy it")]
        );
        assert_eq!(
            reader.turns[1].reply.as_deref(),
            Some("That alert was the transcription job ending.")
        );
        assert_eq!(reader.turns[2].reply.as_deref(), Some("Deployed to staging."));
        assert_eq!(reader.current_turn().unwrap().prompt.as_deref(), Some("thanks"));
        // The live summary is as before.
        assert_eq!(
            TranscriptSummary::read(&lines[..4]).prompt.as_deref(),
            Some("give me the server file")
        );
        assert_eq!(
            TranscriptSummary::read(&lines[..4]).reply.as_deref(),
            Some("That alert was the transcription job ending.")
        );
    }

    #[test]
    fn a_long_pasted_message_is_still_the_humans() {
        let pasted = user("<pasted_content id=\"1\">a long brief</pasted_content>", "u", "x");
        assert_eq!(
            TranscriptSummary::read([pasted]).prompt.as_deref(),
            Some("a long brief")
        );
    }

    #[test]
    fn a_page_ends_where_a_message_starts_so_no_turn_is_cut() {
        let source = MemorySource::new(1, file(&exchanges(30, 1, 2_000)));
        // A small tail, so it has to grow to hold the messages asked for.
        let newest = TranscriptPage::read(&source, None, 4, 4_000);
        let turns = newest.turns();
        assert!(turns.len() >= 4);
        assert_eq!(turns.last().unwrap().prompt.as_deref(), Some("message 30"));
        assert!(
            turns
                .iter()
                .all(|turn| turn.reply == turn.prompt.as_ref().map(|prompt| prompt.replace("message", "answer")))
        );
        assert!(newest.start > 0);
        let before = TranscriptPage::read(&source, Some(newest.start), 4, 4_000);
        let first_newest = turns.first().unwrap().prompt.clone().unwrap();
        let last_before = before.turns().last().unwrap().prompt.clone().unwrap();
        let number: usize = last_before[8..].parse().unwrap();
        assert_eq!(format!("message {}", number + 1), first_newest);
        // All the way back: the first turn opens the session.
        let all = TranscriptPage::read(&source, None, 100, 4_000);
        assert_eq!(all.start, 0);
        let all_turns = all.turns();
        assert_eq!(all_turns.len(), 30);
        assert!(all_turns[0].opens_session);
        assert!(all_turns[1..].iter().all(|turn| !turn.opens_session));
    }

    #[test]
    fn history_pages_back_through_the_sessions_transcripts_before_it() {
        let config = "/c";
        let folder = format!("{config}/projects/-Users-me-assistant");
        let name = |agent: &str| line(json!({"type": "agent-name", "agentName": agent, "sessionId": "s"}));
        let mut store = MemoryStore::new();
        let with_name = |agent: &str, lines: Vec<String>| {
            let mut all = vec![name(agent)];
            all.extend(lines);
            file(&all)
        };
        // Two sessions named "assistant" before this one, and one of another name between them.
        store.write(
            &format!("{folder}/a.jsonl"),
            100,
            with_name("assistant", exchanges(4, 1, 2_000)),
        );
        store.write(
            &format!("{config}/projects/-Users-me-gamecenter/b.jsonl"),
            120,
            with_name("casbo-1077", exchanges(3, 100, 2_000)),
        );
        store.write(
            &format!("{folder}/c.jsonl"),
            140,
            with_name("assistant", exchanges(12, 5, 2_000)),
        );
        store.write(
            &format!("{folder}/d.jsonl"),
            160,
            with_name("assistant", exchanges(15, 17, 2_000)),
        );

        assert_eq!(
            TranscriptChain::earlier("assistant", Some(&format!("{folder}/d.jsonl")), &store.candidates()),
            vec![format!("{folder}/a.jsonl"), format!("{folder}/c.jsonl")]
        );
        let mut history = ConversationHistory::new(&format!("{folder}/d.jsonl"), "assistant");
        assert!(history.has_earlier());
        history.update(&store);
        assert!(history.turns().len() >= ConversationHistory::PAGE_TURNS);
        assert_eq!(history.turns().last().unwrap().prompt.as_deref(), Some("message 31"));
        let mut pages = 0;
        while history.has_earlier() && pages < 10 {
            history.load_earlier(&store);
            pages += 1;
        }
        assert!(!history.has_earlier());
        let expected: Vec<String> = (1..=31).map(|n| format!("message {n}")).collect();
        assert_eq!(
            prompts(&history.turns()),
            expected.iter().map(|s| Some(s.as_str())).collect::<Vec<_>>()
        );
        // Each transcript's first turn starts a session.
        let opening: Vec<String> = history
            .turns()
            .into_iter()
            .filter(|turn| turn.opens_session)
            .filter_map(|turn| turn.prompt)
            .collect();
        assert_eq!(opening, vec!["message 1", "message 5", "message 17"]);

        // It follows the transcript as it grows, a half-written line waiting for its newline.
        let next = user("message 32", "u32", "2026-10-14T02:00:00.000Z");
        let path = format!("{folder}/d.jsonl");
        store.append(
            &path,
            format!("{next}\n{}", said("answer 32", "m32", "2026-10-14T02:00:10.000Z")).as_bytes(),
        );
        history.update(&store);
        assert_eq!(history.turns().last().unwrap().prompt.as_deref(), Some("message 32"));
        assert_eq!(history.turns().last().unwrap().reply, None);
        store.append(&path, b"\n");
        history.update(&store);
        assert_eq!(history.turns().last().unwrap().reply.as_deref(), Some("answer 32"));
        assert_eq!(history.turns().len(), 32);
    }

    #[test]
    fn a_fresh_session_without_a_transcript_still_has_its_past() {
        let folder = "/c/projects/-Users-me-assistant";
        let name = line(json!({"type": "agent-name", "agentName": "assistant"}));
        let mut store = MemoryStore::new();
        let mut a = vec![name.clone()];
        a.extend(exchanges(3, 1, 2_000));
        store.write(&format!("{folder}/a.jsonl"), 100, file(&a));
        let mut history = ConversationHistory::new(&format!("{folder}/new.jsonl"), "assistant");
        assert!(history.has_earlier());
        // The first read goes back as far as it takes to fill a page.
        history.update(&store);
        assert_eq!(
            prompts(&history.turns()),
            vec![Some("message 1"), Some("message 2"), Some("message 3")]
        );
        assert!(!history.has_earlier());
        // Its first message creates the file: that is new, after what is paged in.
        let mut fresh = vec![name];
        fresh.extend(exchanges(1, 4, 2_000));
        store.write(&format!("{folder}/new.jsonl"), 200, file(&fresh));
        history.update(&store);
        assert_eq!(
            prompts(&history.turns()),
            vec![
                Some("message 1"),
                Some("message 2"),
                Some("message 3"),
                Some("message 4")
            ]
        );
        assert!(history.turns().last().unwrap().opens_session);
    }

    #[test]
    fn the_cards_own_turn_is_left_out_of_its_history() {
        let turn = |id: &str, prompt: Option<&str>, reply: &str| {
            HistoryTurn::new(
                Some(id.to_owned()),
                prompt.map(str::to_owned),
                Some(reply.to_owned()),
                None,
            )
        };
        let turns = vec![
            turn("1", Some("skip"), "Skipped."),
            turn(
                "2",
                Some("look at /Users/me/Desktop/report\\ final.pdf please"),
                "Read it.",
            ),
            turn("3", Some("[Image #1] what is this"), "A cat."),
        ];
        let ids = |turns: &[HistoryTurn]| turns.iter().map(|turn| turn.id.clone().unwrap()).collect::<Vec<_>>();
        let before = |shown: Option<&str>| ids(&HistoryTurn::turns(&turns, shown, None).0);
        // As the card notes a dropped file, and without a picture's marker.
        assert_eq!(before(Some("look at [report final.pdf] please")), vec!["1"]);
        assert_eq!(before(Some("what is this")), vec!["1", "2"]);
        // Cut short by the hook.
        assert_eq!(before(Some("look at [report final.pdf] pl…")), vec!["1"]);
        // A turn the session spoke in on its own is known by what it said.
        let mut woken = turns.clone();
        woken.push(turn("4", None, "The job ended."));
        assert_eq!(
            ids(&HistoryTurn::turns(&woken, None, Some("The job ended.")).0),
            vec!["1", "2", "3"]
        );
        assert_eq!(HistoryTurn::turns(&woken, None, Some("Something else.")).0.len(), 4);
        // Not in the transcript yet: all of it is history.
        assert_eq!(before(Some("something new")), vec!["1", "2", "3"]);
        assert_eq!(before(None), vec!["1", "2", "3"]);
    }

    #[test]
    fn dates_parse_with_or_without_fractional_seconds() {
        assert_eq!(HistoryTurn::date(Some("1970-01-01T00:00:00Z")), Some(0));
        assert_eq!(HistoryTurn::date(Some("1970-01-01T00:00:01.5Z")), Some(1500));
        assert_eq!(
            HistoryTurn::date(Some("2026-10-15T02:48:17.095Z")),
            Some(1_792_032_497_095)
        );
        assert_eq!(HistoryTurn::date(Some("2026-10-15T02:48:17Z")), Some(1_792_032_497_000));
        assert_eq!(
            HistoryTurn::date(Some("2026-10-15T10:48:17+08:00")),
            Some(1_792_032_497_000)
        );
        assert_eq!(HistoryTurn::date(Some("x")), None);
        assert_eq!(HistoryTurn::date(None), None);
    }
}
