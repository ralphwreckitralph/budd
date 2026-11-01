//! Crew sessions: names, start rules, stages, attention, archiving (from bin/crew and AvatarCore/CrewBoard.swift).
//!
//! Crew are the background sessions the assistant runs for the human, one per ticket, plus the chats
//! the human opens at a desk in Budd's office. This module holds the rules about them that need no OS:
//! what a name may be, what a start is refused for, how a session's line on the bridge reads, which
//! blocked session still calls the human, and which archived one stays off the bridge.

use crate::fingerprint::fingerprint;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::LazyLock;
use thiserror::Error;

/// The name of the assistant's own session, which is never crew.
pub const ASSISTANT: &str = "assistant";
/// What a chat's name starts with.
pub const CHAT_PREFIX: &str = "chat-";
/// The most characters a session name may have.
pub const LONGEST_NAME: usize = 40;

static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9-]{1,39}$").unwrap());

/// What Claude Code's background-job runner keeps about a session in `<config>/jobs/<short id>/state.json`:
/// its state (working, blocked on the human, done), a line on where it stands, and the command it was
/// started with. Every field is optional, as the file belongs to Claude Code and may change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct JobState {
    /// "working", "blocked", "done", or whatever Claude Code writes.
    #[serde(default)]
    pub state: Option<String>,
    /// One line on where the job stands, such as "Jira comment awaiting your approval".
    #[serde(default)]
    pub detail: Option<String>,
    /// What the job was started with, such as "/lets-do-it-v2 CASBO-999".
    #[serde(default)]
    pub intent: Option<String>,
}

impl JobState {
    /// The job state in a file's text, or `None` when it is not JSON.
    pub fn decode(text: &str) -> Option<JobState> {
        serde_json::from_str(text).ok()
    }
}

/// How a gauge reads: calm below 80%, a warning from there, and full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Below 80%.
    Calm,
    /// From 80% up, short of full.
    High,
    /// At or past the limit.
    Full,
}

/// How a crew session's line on the bridge reads: the stage of its ticket, how long it has been quiet,
/// and how full it is.
pub struct CrewBoard;

impl CrewBoard {
    /// The context size at which a crew session counts as full. Crew sessions have no keep-alive to
    /// replace them, so this is the human's own budget; the `crewContextLimit` setting changes it.
    pub const DEFAULT_CONTEXT_LIMIT: u64 = 150_000;

    /// The stage a session is at, from the last slash command it ran, else the command it was started
    /// with: "/lets-finalize-it CASBO-1025" is "finalize", "/lets-do-it-v2 CASBO-999" is "do-it-v2".
    pub fn stage(command: Option<&str>, intent: Option<&str>) -> Option<String> {
        let command = command.or_else(|| intent.filter(|i| i.starts_with('/')))?;
        let first = command.split_whitespace().next()?;
        let mut name = first.get(1..).unwrap_or("");
        if name.is_empty() {
            return None;
        }
        name = name.strip_prefix("lets-").unwrap_or(name);
        // "finalize-it" says no more than "finalize", but "do-it" would leave only "do".
        if name.chars().count() > 5
            && let Some(stem) = name.strip_suffix("-it")
        {
            name = stem;
        }
        Some(name.to_owned())
    }

    /// How long a session has been quiet, coarsely: "now", "12m", "3h", "2d".
    pub fn quiet(seconds: f64) -> String {
        let seconds = seconds.max(0.0);
        if seconds < 60.0 {
            "now".to_owned()
        } else if seconds < 3600.0 {
            format!("{}m", (seconds / 60.0) as u64)
        } else if seconds < 86400.0 {
            format!("{}h", (seconds / 3600.0) as u64)
        } else {
            format!("{}d", (seconds / 86400.0) as u64)
        }
    }

    /// How long a session has been quiet, for its line. One whose process is gone says it is asleep.
    pub fn quiet_or_asleep(seconds: f64, asleep: bool) -> String {
        if asleep {
            format!("asleep {}", Self::quiet(seconds))
        } else {
            Self::quiet(seconds)
        }
    }

    /// Whether a background session whose process is gone stays on the bridge.
    ///
    /// Claude Code's daemon retires a session left idle at its prompt for an hour, and one retired while
    /// blocked on the human still waits on them; `claude attach` brings it back, so a message still
    /// reaches it. A session the human archived leaves the listing altogether, and the assistant's own
    /// old sessions are replaced by the keep-alive, not waiting on anyone.
    pub fn keeps_asleep(state: Option<&str>, name: &str, assistant: &str) -> bool {
        state == Some("blocked") && name != assistant
    }

    /// The gauge's label: the share rounded down, so "full" never shows early.
    pub fn fullness_label(fullness: f64) -> String {
        if fullness >= 1.0 {
            "full".to_owned()
        } else {
            format!("{}%", (fullness * 100.0) as u32)
        }
    }

    /// The card's line about it. Crew sessions have no keep-alive, so a full one needs the human.
    pub fn card_label(fullness: f64) -> String {
        if fullness >= 1.0 {
            "Session full \u{00B7} start a fresh one".to_owned()
        } else {
            format!("Session {}% full", (fullness * 100.0) as u32)
        }
    }

    /// How the gauge reads at `fullness`.
    pub fn level(fullness: f64) -> Level {
        if fullness >= 1.0 {
            Level::Full
        } else if fullness >= 0.8 {
            Level::High
        } else {
            Level::Calm
        }
    }
}

/// Which crew sessions blocked on the human still call them over.
///
/// A session that ends its turn waiting on the human (Claude Code calls the job "blocked") calls once:
/// the office opens on its desk. When the human has seen it, by resting on its desk or its ask, or by
/// opening its card, it goes quiet: its desk still says it needs them, but it no longer opens the office,
/// as a job can stay blocked for hours. Its next blocked turn, known by its new reply, calls again.
/// What was seen is kept as plain values, so a restart does not call again for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CrewAttention {
    seen: HashMap<String, String>,
}

impl CrewAttention {
    /// Attention restored from what was kept: a fingerprint of the reply each session was blocked on
    /// when the human saw it.
    pub fn new(seen: HashMap<String, String>) -> CrewAttention {
        CrewAttention { seen }
    }

    /// What to keep across launches.
    pub fn seen(&self) -> &HashMap<String, String> {
        &self.seen
    }

    /// Whether a session blocked on `reply` still calls the human.
    pub fn is_loud(&self, session: &str, reply: Option<&str>) -> bool {
        self.seen.get(session) != Some(&Self::fingerprint(reply))
    }

    /// The human has seen that `session` is blocked on `reply`.
    pub fn see(&mut self, session: &str, reply: Option<&str>) {
        self.seen.insert(session.to_owned(), Self::fingerprint(reply));
    }

    /// Drops what is known about sessions that have gone.
    pub fn keep_only<S: AsRef<str>>(&mut self, sessions: &[S]) {
        self.seen.retain(|name, _| sessions.iter().any(|s| s.as_ref() == name));
    }

    /// Short and stable across launches: FNV-1a over the reply's bytes, an absent reply as empty.
    pub fn fingerprint(reply: Option<&str>) -> String {
        fingerprint(reply.unwrap_or(""))
    }
}

/// Crew sessions the human archived. Archiving stops a session (`claude stop`), which takes it out of
/// the live sessions, but the bridge only learns that at the next listing; so it is kept off the bridge
/// from the click until then, and put back if the stop fails.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArchivedCrew {
    since: HashMap<String, f64>,
}

impl ArchivedCrew {
    /// Long enough for a listing that no longer has it to come in, in seconds.
    pub const HOLD: f64 = 30.0;

    /// Nothing archived yet.
    pub fn new() -> ArchivedCrew {
        ArchivedCrew::default()
    }

    /// `session` was archived at `t`; archives older than the hold are forgotten at the same time.
    pub fn archive(&mut self, session: &str, t: f64) {
        self.since.retain(|_, since| t - *since <= Self::HOLD);
        self.since.insert(session.to_owned(), t);
    }

    /// Puts it back on the bridge, as after a stop that failed.
    pub fn restore(&mut self, session: &str) {
        self.since.remove(session);
    }

    /// Whether `session` is still kept off the bridge at `t`.
    pub fn hides(&self, session: &str, t: f64) -> bool {
        self.since.get(session).is_some_and(|since| t - since <= Self::HOLD)
    }

    /// Whether archiving asks first: stopping a session in the middle of work cuts the work off. One that
    /// is idle, done, or waiting on the human has nothing to lose.
    pub fn asks_first(status: Option<&str>) -> bool {
        status == Some("busy")
    }
}

/// Why a crew session is not started.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Refusal {
    /// The name is not lowercase kebab-case of 2 to 40 characters, or is the assistant's.
    #[error("\"{0}\" is not a crew name: lowercase letters, digits and dashes, 2 to 40, not \"assistant\"")]
    NotACrewName(String),
    /// The name is a crew name but not a chat's.
    #[error("\"{0}\" is not a chat name: \"chat-\" then lowercase letters, digits and dashes, 40 at most")]
    NotAChatName(String),
    /// A live session already has the name.
    #[error("a live session is already called \"{0}\"; pick another name or stop that one first")]
    NameInUse(String),
    /// The brief has no words.
    #[error("the brief is empty")]
    BlankBrief,
    /// The default permission mode is bypassPermissions; crew never run without prompts.
    #[error("the default permission mode is bypassPermissions; crew never run without prompts")]
    BypassPermissions,
}

/// A session name the crew script takes: lowercase letters, digits and dashes, 2 to 40 characters,
/// starting with a letter or digit, and never "assistant".
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CrewName(String);

impl CrewName {
    /// `name` as a crew name, or why it is not one.
    pub fn parse(name: &str) -> Result<CrewName, Refusal> {
        if NAME.is_match(name) && name != ASSISTANT {
            Ok(CrewName(name.to_owned()))
        } else {
            Err(Refusal::NotACrewName(name.to_owned()))
        }
    }

    /// The name as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is a chat's name.
    pub fn is_chat(&self) -> bool {
        self.0.starts_with(CHAT_PREFIX)
    }
}

/// What the human asks a crew session to be started as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartRequest {
    /// The session's name, such as a ticket id lowercased.
    pub name: String,
    /// Its first message. It is sent to the session once it is up, not passed on the command line.
    pub brief: String,
    /// The model to start on, when not the folder's default.
    pub model: Option<String>,
    /// A folder to add to the session's reach, when any.
    pub add_dir: Option<String>,
    /// Whether to start with Claude in Chrome.
    pub chrome: bool,
}

/// The `claude` arguments that start the crew session `req` asks for, or why it is refused.
///
/// `live_names` are the names of the background sessions whose process is alive, and `default_mode` the
/// permission mode of the folder's settings, which the session gets explicitly so it never inherits
/// whatever mode the caller runs in. A bypassPermissions default is refused outright.
pub fn check_start<S: AsRef<str>>(
    req: &StartRequest,
    live_names: &[S],
    default_mode: &str,
) -> Result<Vec<String>, Refusal> {
    let name = CrewName::parse(&req.name)?;
    if live_names.iter().any(|n| n.as_ref() == name.as_str()) {
        return Err(Refusal::NameInUse(req.name.clone()));
    }
    if req.brief.trim().is_empty() {
        return Err(Refusal::BlankBrief);
    }
    if default_mode == "bypassPermissions" {
        return Err(Refusal::BypassPermissions);
    }
    let mut argv = flags(["--bg", "--name", name.as_str(), "--permission-mode", default_mode]);
    if let Some(model) = &req.model {
        argv.extend(flags(["--model", model]));
    }
    if let Some(dir) = &req.add_dir {
        argv.extend(flags(["--add-dir", dir]));
    }
    if req.chrome {
        argv.push("--chrome".to_owned());
    }
    argv.push(req.brief.trim().to_owned());
    Ok(argv)
}

/// How a session in the assistant's folder starts, the assistant and its chats alike: in auto mode, as
/// nothing can accept the bypass disclaimer in the background; on `model`, as a folder that has not
/// learned its account's plan otherwise starts on a lesser one; with `add_dir` (the home folder) added;
/// with Chrome when asked.
pub fn assistant_flags(name: &str, model: &str, add_dir: &str, chrome: bool) -> Vec<String> {
    let mut argv = flags([
        "--bg",
        "--name",
        name,
        "--model",
        model,
        "--permission-mode",
        "auto",
        "--add-dir",
        add_dir,
    ]);
    if chrome {
        argv.push("--chrome".to_owned());
    }
    argv
}

/// The arguments that start the chat `name`, which starts like the assistant, or why it is refused: a
/// chat's name starts with "chat-" and is not a live session's.
pub fn chat_flags<S: AsRef<str>>(
    name: &str,
    model: &str,
    add_dir: &str,
    chrome: bool,
    live_names: &[S],
) -> Result<Vec<String>, Refusal> {
    let parsed = CrewName::parse(name).map_err(|_| Refusal::NotAChatName(name.to_owned()))?;
    if !parsed.is_chat() {
        return Err(Refusal::NotAChatName(name.to_owned()));
    }
    if live_names.iter().any(|n| n.as_ref() == name) {
        return Err(Refusal::NameInUse(name.to_owned()));
    }
    Ok(assistant_flags(name, model, add_dir, chrome))
}

fn flags<'a>(parts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    parts.into_iter().map(str::to_owned).collect()
}

/// The chats the human opens at a desk in Budd's office, and how they are named.
pub struct OfficeChat;

impl OfficeChat {
    /// Whether `name` is a chat's.
    pub fn is_chat(name: &str) -> bool {
        name.starts_with(CHAT_PREFIX)
    }

    /// A name for the chat a message starts: "chat-" and its first few words, as kebab case ("Where is
    /// the dev server set up?" is "chat-where-is-the-dev"), with "-2", "-3" when a live session has it.
    /// A message with no words to take, such as a picture alone, is named for the time: `hhmm` is the
    /// time as "HHMM", giving "chat-1432". The stem stays within 37 characters, so a suffix still fits.
    pub fn name<S: AsRef<str>>(message: &str, taken: &[S], hhmm: &str) -> String {
        let lowered = message.to_lowercase();
        let words = lowered
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty() && w.is_ascii());
        let mut stem = CHAT_PREFIX.to_owned();
        for word in words.take(4) {
            let next = if stem == CHAT_PREFIX {
                format!("{stem}{word}")
            } else {
                format!("{stem}-{word}")
            };
            if next.len() > LONGEST_NAME - 3 {
                break;
            }
            stem = next;
        }
        if stem == CHAT_PREFIX {
            stem.push_str(hhmm);
        }
        let is_taken = |name: &str| taken.iter().any(|t| t.as_ref() == name);
        if !is_taken(&stem) {
            return stem;
        }
        (2..)
            .map(|n| format!("{stem}-{n}"))
            .find(|candidate| !is_taken(candidate))
            .unwrap_or(stem)
    }
}

/// One session as `claude agents --json` lists it. Every field is optional and unknown keys are ignored,
/// as the output belongs to Claude Code and may change between versions; what a row lacks decides what
/// [`live_sessions`] makes of it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentRow {
    /// The short id, what `claude attach`, `stop` and `logs` take.
    pub id: Option<String>,
    /// The full session id, what its hooks and transcript are named after.
    pub session_id: Option<String>,
    /// The display name given with `--name`.
    pub name: Option<String>,
    /// The folder the session was started in.
    pub cwd: Option<String>,
    /// The process, or `None` once Claude Code has let it go.
    pub pid: Option<u32>,
    /// "idle", "busy" or "waiting".
    pub status: Option<String>,
    /// Where its job stands: "working", "blocked", "done", "failed" or "stopped".
    pub state: Option<String>,
    /// What a waiting session waits for, as Claude Code words it.
    pub waiting_for: Option<String>,
    /// When it started, as Claude Code counts it (milliseconds since the epoch, as of 2.1.316).
    pub started_at: Option<f64>,
    /// "background" for a session started with `--bg`; interactive sessions say otherwise.
    pub kind: Option<String>,
}

/// What `claude agents` says a session is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    /// At its prompt.
    Idle,
    /// In a turn.
    Busy,
    /// Its turn is paused on the human: a permission, a question.
    Waiting,
}

impl Status {
    /// The status Claude Code names `word`, or `None` for one this module does not know.
    pub fn parse(word: &str) -> Option<Status> {
        match word {
            "idle" => Some(Status::Idle),
            "busy" => Some(Status::Busy),
            "waiting" => Some(Status::Waiting),
            _ => None,
        }
    }
}

/// Where a background session's job stands, as `claude agents` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentState {
    /// Working through its turn.
    Working,
    /// Its turn ended waiting on the human.
    Blocked,
    /// Its work is finished.
    Done,
    /// Its turn ended on an error.
    Failed,
    /// It was stopped.
    Stopped,
}

impl AgentState {
    /// The state Claude Code names `word`, or `None` for one this module does not know.
    pub fn parse(word: &str) -> Option<AgentState> {
        match word {
            "working" => Some(AgentState::Working),
            "blocked" => Some(AgentState::Blocked),
            "done" => Some(AgentState::Done),
            "failed" => Some(AgentState::Failed),
            "stopped" => Some(AgentState::Stopped),
            _ => None,
        }
    }
}

/// A background session that is on the bridge: its process is alive, or it is asleep while it waits on
/// the human (from SessionLauncher.LiveSession).
#[derive(Debug, Clone, PartialEq)]
pub struct LiveSession {
    /// The short id, what `claude attach` takes.
    pub id: String,
    /// The full session id, what its hooks and transcript are named after.
    pub session_id: String,
    /// Its name, or its short id when it was started without one.
    pub name: String,
    /// Its process, or `None` for a session Claude Code retired while it waited on the human
    /// ([`CrewBoard::keeps_asleep`]); `claude attach` wakes it, so a message still reaches it.
    pub pid: Option<u32>,
    /// The folder it was started in.
    pub cwd: String,
    /// What it is doing right now, when Claude Code said something this module knows.
    pub status: Option<Status>,
    /// Where its job stands, when Claude Code said something this module knows.
    pub state: Option<AgentState>,
    /// What a waiting session waits for.
    pub waiting_for: Option<String>,
    /// When it started, as Claude Code counts it; 0 when the row did not say.
    pub started_at: f64,
}

impl LiveSession {
    /// Whether its process is gone: it stays listed only because it waits on the human.
    pub fn asleep(&self) -> bool {
        self.pid.is_none()
    }
}

/// The background sessions in `rows` that belong on the bridge, oldest first: every one whose process
/// `alive` confirms, and every one asleep while it waits on the human. `claude agents --json` also lists
/// interactive sessions, and the background ones a restart killed or the daemon retired (no pid, or a
/// dead one); apart from the blocked crew, those are gone. A row without an id, session id or folder
/// is not a session this module can do anything with and is skipped.
pub fn live_sessions(rows: &[AgentRow], assistant: &str, alive: impl Fn(u32) -> bool) -> Vec<LiveSession> {
    let mut sessions: Vec<LiveSession> = rows
        .iter()
        .filter(|row| row.kind.as_deref() == Some("background"))
        .filter_map(|row| {
            let id = row.id.clone()?;
            let session_id = row.session_id.clone()?;
            let cwd = row.cwd.clone()?;
            let name = row.name.clone().unwrap_or_else(|| id.clone());
            let pid = row.pid.filter(|pid| alive(*pid));
            if pid.is_none() && !CrewBoard::keeps_asleep(row.state.as_deref(), &name, assistant) {
                return None;
            }
            Some(LiveSession {
                id,
                session_id,
                name,
                pid,
                cwd,
                status: row.status.as_deref().and_then(Status::parse),
                state: row.state.as_deref().and_then(AgentState::parse),
                waiting_for: row.waiting_for.clone(),
                started_at: row.started_at.unwrap_or(0.0),
            })
        })
        .collect();
    sessions.sort_by(|a, b| a.started_at.total_cmp(&b.started_at));
    sessions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stage_is_the_last_work_command() {
        assert_eq!(
            CrewBoard::stage(Some("/lets-finalize-it CASBO-1025"), Some("/lets-do-it-v2 CASBO-1025")).as_deref(),
            Some("finalize")
        );
        assert_eq!(
            CrewBoard::stage(None, Some("/lets-do-it-v2 CASBO-999")).as_deref(),
            Some("do-it-v2")
        );
        assert_eq!(
            CrewBoard::stage(Some("/lets-do-it CASBO-1"), None).as_deref(),
            Some("do-it")
        );
        assert_eq!(
            CrewBoard::stage(Some("/jira-comment"), None).as_deref(),
            Some("jira-comment")
        );
        assert_eq!(CrewBoard::stage(None, Some("look into the casino list")), None);
        assert_eq!(CrewBoard::stage(Some("/"), None), None);
    }

    #[test]
    fn quiet_time_is_coarse() {
        assert_eq!(CrewBoard::quiet(20.0), "now");
        assert_eq!(CrewBoard::quiet(12.0 * 60.0 + 40.0), "12m");
        assert_eq!(CrewBoard::quiet(2.0 * 3600.0 + 59.0 * 60.0), "2h");
        assert_eq!(CrewBoard::quiet(50.0 * 3600.0), "2d");
        assert_eq!(CrewBoard::quiet_or_asleep(21.0 * 3600.0, true), "asleep 21h");
        assert_eq!(CrewBoard::quiet_or_asleep(21.0 * 3600.0, false), "21h");
        assert_eq!(CrewBoard::quiet(-5.0), "now");
    }

    #[test]
    fn a_retired_session_stays_only_while_it_waits_on_the_human() {
        assert!(CrewBoard::keeps_asleep(Some("blocked"), "casbo-1055", "assistant"));
        assert!(!CrewBoard::keeps_asleep(Some("done"), "casbo-1055", "assistant"));
        assert!(!CrewBoard::keeps_asleep(None, "casbo-1055", "assistant"));
        // The keep-alive replaces the assistant; its old sessions are not crew.
        assert!(!CrewBoard::keeps_asleep(Some("blocked"), "assistant", "assistant"));
    }

    #[test]
    fn the_gauge_rounds_down_and_warns_early() {
        assert_eq!(CrewBoard::fullness_label(0.719), "71%");
        assert_eq!(CrewBoard::fullness_label(1.01), "full");
        assert_eq!(CrewBoard::card_label(0.719), "Session 71% full");
        assert_eq!(CrewBoard::card_label(1.0), "Session full · start a fresh one");
        assert_eq!(CrewBoard::level(0.5), Level::Calm);
        assert_eq!(CrewBoard::level(0.85), Level::High);
        assert_eq!(CrewBoard::level(1.0), Level::Full);
    }

    #[test]
    fn decodes_the_job_file() {
        let text = r#"{"state":"blocked","detail":"1045 session needs Chrome login","tempo":"idle","tokens":32656,"intent":"/lets-do-it-v2 CASBO-1045","name":"casbo-1045"}"#;
        assert_eq!(
            JobState::decode(text),
            Some(JobState {
                state: Some("blocked".to_owned()),
                detail: Some("1045 session needs Chrome login".to_owned()),
                intent: Some("/lets-do-it-v2 CASBO-1045".to_owned()),
            })
        );
        assert_eq!(JobState::decode("{}"), Some(JobState::default()));
        assert_eq!(JobState::decode("nope"), None);
    }

    #[test]
    fn a_blocked_session_calls_once_per_reply() {
        let mut attention = CrewAttention::default();
        assert!(attention.is_loud("s1", Some("Reply: `tab is up`")));
        attention.see("s1", Some("Reply: `tab is up`"));
        assert!(!attention.is_loud("s1", Some("Reply: `tab is up`")));
        assert!(attention.is_loud("s1", Some("Now approve rev 3?")));
        assert!(attention.is_loud("s2", Some("Reply: `tab is up`")));
        attention.keep_only(&["s2"]);
        assert!(attention.is_loud("s1", Some("Reply: `tab is up`")));
    }

    #[test]
    fn what_was_seen_survives_a_restart() {
        let mut attention = CrewAttention::default();
        attention.see("s1", Some("Reply: `tab is up`"));
        let restored = CrewAttention::new(attention.seen().clone());
        assert!(!restored.is_loud("s1", Some("Reply: `tab is up`")));
        assert!(restored.is_loud("s1", Some("Reply: `approve`")));
        assert_eq!(CrewAttention::fingerprint(Some("abc")), "e71fa2190541574b");
        assert_eq!(CrewAttention::fingerprint(None), CrewAttention::fingerprint(Some("")));
    }

    #[test]
    fn keeps_an_archived_session_off_the_bridge_until_the_listing_catches_up() {
        let mut archived = ArchivedCrew::new();
        archived.archive("a", 100.0);
        assert!(archived.hides("a", 100.0) && archived.hides("a", 100.0 + ArchivedCrew::HOLD));
        assert!(!archived.hides("b", 100.0));
        // By then the listing has dropped it; should it ever come back (attached again), it shows.
        assert!(!archived.hides("a", 101.0 + ArchivedCrew::HOLD));
    }

    #[test]
    fn a_failed_stop_puts_it_back() {
        let mut archived = ArchivedCrew::new();
        archived.archive("a", 0.0);
        archived.archive("b", 1.0);
        archived.restore("a");
        assert!(!archived.hides("a", 2.0) && archived.hides("b", 2.0));
    }

    #[test]
    fn forgets_old_archives() {
        let mut archived = ArchivedCrew::new();
        archived.archive("a", 0.0);
        archived.archive("b", 1000.0);
        let mut fresh = ArchivedCrew::new();
        fresh.archive("b", 1000.0);
        assert_eq!(archived, fresh);
    }

    #[test]
    fn asks_first_only_while_it_works() {
        assert!(ArchivedCrew::asks_first(Some("busy")));
        assert!(!ArchivedCrew::asks_first(Some("idle")) && !ArchivedCrew::asks_first(None));
    }

    #[test]
    fn a_crew_name_is_short_kebab_case_and_not_the_assistant() {
        assert!(CrewName::parse("casbo-1077").is_ok());
        assert!(CrewName::parse("a1").is_ok());
        assert!(CrewName::parse(&"a".repeat(40)).is_ok());
        for bad in [
            "",
            "a",
            "-ab",
            "Casbo-1",
            "casbo 1",
            "casbo_1",
            "assistant",
            &"a".repeat(41),
        ] {
            assert_eq!(
                CrewName::parse(bad),
                Err(Refusal::NotACrewName(bad.to_owned())),
                "{bad:?}"
            );
        }
        assert!(CrewName::parse("chat-hello").unwrap().is_chat());
        assert!(!CrewName::parse("casbo-1").unwrap().is_chat());
    }

    fn request(name: &str, brief: &str) -> StartRequest {
        StartRequest {
            name: name.to_owned(),
            brief: brief.to_owned(),
            model: None,
            add_dir: None,
            chrome: false,
        }
    }

    #[test]
    fn a_start_is_the_argv_or_a_refusal() {
        let live = ["assistant", "casbo-1055"];
        assert_eq!(
            check_start(&request("casbo-1077", "Fix the thing"), &live, "auto"),
            Ok(flags([
                "--bg",
                "--name",
                "casbo-1077",
                "--permission-mode",
                "auto",
                "Fix the thing"
            ]))
        );
        let full = StartRequest {
            model: Some("opus".to_owned()),
            add_dir: Some("/Users/alice".to_owned()),
            chrome: true,
            ..request("casbo-1077", "Fix the thing")
        };
        assert_eq!(
            check_start(&full, &live, "default"),
            Ok(flags([
                "--bg",
                "--name",
                "casbo-1077",
                "--permission-mode",
                "default",
                "--model",
                "opus",
                "--add-dir",
                "/Users/alice",
                "--chrome",
                "Fix the thing"
            ]))
        );
        assert_eq!(
            check_start(&request("casbo-1077", "x"), &live, "bypassPermissions"),
            Err(Refusal::BypassPermissions)
        );
        assert_eq!(
            check_start(&request("casbo-1055", "x"), &live, "auto"),
            Err(Refusal::NameInUse("casbo-1055".to_owned()))
        );
        assert_eq!(
            check_start(&request("casbo-1077", "  \n"), &live, "auto"),
            Err(Refusal::BlankBrief)
        );
        assert_eq!(
            check_start(&request("assistant", "x"), &live, "auto"),
            Err(Refusal::NotACrewName("assistant".to_owned()))
        );
        assert_eq!(
            check_start(&request("CASBO-1", "x"), &live, "auto"),
            Err(Refusal::NotACrewName("CASBO-1".to_owned()))
        );
        assert_eq!(Refusal::BlankBrief.to_string(), "the brief is empty");
    }

    #[test]
    fn the_assistant_and_its_chats_start_alike() {
        assert_eq!(
            assistant_flags("assistant", "opus", "/Users/alice", true),
            flags([
                "--bg",
                "--name",
                "assistant",
                "--model",
                "opus",
                "--permission-mode",
                "auto",
                "--add-dir",
                "/Users/alice",
                "--chrome"
            ])
        );
        assert!(!assistant_flags("assistant", "opus", "/Users/alice", false).contains(&"--chrome".to_owned()));
        let live = ["assistant", "chat-where-is-it"];
        assert_eq!(
            chat_flags("chat-hello", "opus", "/h", false, &live),
            Ok(assistant_flags("chat-hello", "opus", "/h", false))
        );
        assert_eq!(
            chat_flags("casbo-1", "opus", "/h", false, &live),
            Err(Refusal::NotAChatName("casbo-1".to_owned()))
        );
        assert_eq!(
            chat_flags("chat-Hello", "opus", "/h", false, &live),
            Err(Refusal::NotAChatName("chat-Hello".to_owned()))
        );
        assert_eq!(
            chat_flags("chat-where-is-it", "opus", "/h", false, &live),
            Err(Refusal::NameInUse("chat-where-is-it".to_owned()))
        );
    }

    #[test]
    fn a_chat_is_named_for_its_first_words() {
        let none: [&str; 0] = [];
        assert_eq!(
            OfficeChat::name("Where is the dev server set up?", &none, "0905"),
            "chat-where-is-the-dev"
        );
        assert_eq!(
            OfficeChat::name("  JC's PR #256 FE ", &none, "0905"),
            "chat-jc-s-pr-256"
        );
        assert_eq!(
            OfficeChat::name("where is it", &["chat-where-is-it"], "0905"),
            "chat-where-is-it-2"
        );
        assert_eq!(
            OfficeChat::name("where is it", &["chat-where-is-it", "chat-where-is-it-2"], "0905"),
            "chat-where-is-it-3"
        );
        // Long words stop before the name outgrows what the crew script takes, with room for "-2".
        let long = OfficeChat::name("internationalization localization accessibility", &none, "0905");
        assert_eq!(long, "chat-internationalization");
        assert!(long.len() <= 37);
        // Nothing to name it by: the time.
        assert_eq!(OfficeChat::name("", &none, "0905"), "chat-0905");
        assert_eq!(OfficeChat::name("日本語 ☕️", &none, "0905"), "chat-0905");
        assert!(OfficeChat::is_chat("chat-0905"));
        assert!(!OfficeChat::is_chat("casbo-1077"));
        // Every name it makes is one the crew script takes.
        assert!(CrewName::parse(&long).is_ok());
        assert!(CrewName::parse("chat-0905").is_ok());
    }

    fn row(id: &str, name: Option<&str>, pid: Option<u32>, state: Option<&str>, started_at: f64) -> AgentRow {
        AgentRow {
            id: Some(id.to_owned()),
            session_id: Some(format!("{id}-full")),
            name: name.map(str::to_owned),
            cwd: Some("/w/repo".to_owned()),
            pid,
            status: Some("idle".to_owned()),
            state: state.map(str::to_owned),
            kind: Some("background".to_owned()),
            started_at: Some(started_at),
            ..AgentRow::default()
        }
    }

    #[test]
    fn the_bridge_keeps_live_processes_and_blocked_sleepers_oldest_first() {
        let rows = [
            row("b2", Some("casbo-2"), Some(202), Some("working"), 2.0),
            row("a1", Some("assistant"), Some(101), None, 1.0),
            row("c3", Some("casbo-3"), None, Some("blocked"), 3.0),
            row("d4", Some("casbo-4"), Some(404), Some("blocked"), 4.0),
            row("e5", Some("casbo-5"), Some(505), Some("done"), 5.0),
            row("f6", Some("assistant"), None, Some("blocked"), 6.0),
            row("g7", None, Some(707), None, 7.0),
        ];
        let dead = [404, 505];
        let sessions = live_sessions(&rows, "assistant", |pid| !dead.contains(&pid));
        assert_eq!(
            sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["a1", "b2", "c3", "d4", "g7"]
        );
        assert_eq!(sessions[0].pid, Some(101));
        assert_eq!(sessions[1].state, Some(AgentState::Working));
        assert!(sessions[2].asleep());
        assert_eq!(sessions[2].state, Some(AgentState::Blocked));
        // A blocked session whose process died is asleep, not gone.
        assert!(sessions[3].asleep());
        // A row without a name goes by its id.
        assert_eq!(sessions[4].name, "g7");
        assert_eq!(sessions[4].session_id, "g7-full");
        assert_eq!(sessions[4].status, Some(Status::Idle));
    }

    #[test]
    fn only_background_rows_with_the_fields_the_bridge_needs_count() {
        let mut interactive = row("i1", Some("term"), Some(1), None, 1.0);
        interactive.kind = Some("interactive".to_owned());
        let mut no_kind = row("k2", Some("x"), Some(2), None, 2.0);
        no_kind.kind = None;
        let mut no_cwd = row("c3", Some("y"), Some(3), None, 3.0);
        no_cwd.cwd = None;
        let mut no_session = row("s4", Some("z"), Some(4), None, 4.0);
        no_session.session_id = None;
        let mut odd_words = row("o5", Some("w"), Some(5), Some("paused"), 5.0);
        odd_words.status = Some("thinking".to_owned());
        odd_words.started_at = None;
        let rows = [interactive, no_kind, no_cwd, no_session, odd_words];
        let sessions = live_sessions(&rows, "assistant", |_| true);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "o5");
        assert_eq!(sessions[0].status, None);
        assert_eq!(sessions[0].state, None);
        assert_eq!(sessions[0].started_at, 0.0);
    }

    #[test]
    fn an_agent_row_takes_what_claude_prints_and_ignores_the_rest() {
        let json = r#"{"id": "abc123", "sessionId": "0f1e-2d3c", "name": "casbo-1", "cwd": "/w",
            "pid": 4242, "status": "waiting", "state": "blocked", "waitingFor": "permission",
            "startedAt": 1761900000000, "kind": "background", "somethingNew": {"x": 1}}"#;
        let row: AgentRow = serde_json::from_str(json).expect("decodes");
        assert_eq!(row.id.as_deref(), Some("abc123"));
        assert_eq!(row.pid, Some(4242));
        assert_eq!(row.waiting_for.as_deref(), Some("permission"));
        assert_eq!(row.started_at, Some(1761900000000.0));
        let bare: AgentRow = serde_json::from_str("{}").expect("decodes");
        assert_eq!(bare, AgentRow::default());
        assert_eq!(Status::parse("busy"), Some(Status::Busy));
        assert_eq!(Status::parse("Busy"), None);
        assert_eq!(AgentState::parse("stopped"), Some(AgentState::Stopped));
        assert_eq!(AgentState::parse(""), None);
    }
}
