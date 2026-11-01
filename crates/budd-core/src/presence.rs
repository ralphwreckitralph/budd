//! The assistant's mood from its state file (from AvatarCore/Presence.swift).
//!
//! A Claude session's hooks write one JSON object per event; [`SessionReport`] is that file, and
//! [`PresenceResolver`] turns the newest one into what the avatar shows right now.

use crate::text::{collapsed, without_image_markers};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::LazyLock;

static PASTE_OPEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^<pasted_content[^>]*> ?").unwrap());

/// What the avatar is showing, from the session's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mood {
    /// The session is at its prompt with nothing to do.
    Idle,
    /// The model is composing, between tool calls.
    Thinking,
    /// A tool is running.
    Working,
    /// The session needs the human: a question, a permission prompt, a notification.
    Waiting,
    /// The turn ended well.
    Done,
    /// The turn ended on an API error (rate limit, overload, auth); the detail says which.
    Failed,
    /// The session has ended or its process is gone.
    Sleeping,
}

impl Mood {
    /// Every mood, in the order the state file names them.
    pub const ALL: [Mood; 7] = [
        Mood::Idle,
        Mood::Thinking,
        Mood::Working,
        Mood::Waiting,
        Mood::Done,
        Mood::Failed,
        Mood::Sleeping,
    ];

    /// The mood's name in the state file: "idle", "thinking", "working", "waiting", "done", "failed", "sleeping".
    pub fn name(self) -> &'static str {
        match self {
            Mood::Idle => "idle",
            Mood::Thinking => "thinking",
            Mood::Working => "working",
            Mood::Waiting => "waiting",
            Mood::Done => "done",
            Mood::Failed => "failed",
            Mood::Sleeping => "sleeping",
        }
    }

    /// The mood a state file names, or `None` for a name no version of the hook writes.
    pub fn parse(name: &str) -> Option<Mood> {
        Mood::ALL.into_iter().find(|mood| mood.name() == name)
    }
}

/// Where a session takes messages: the socket of Claude Code's messaging server and the token it wants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inbox {
    /// The socket's path, from `CLAUDE_CODE_MESSAGING_SOCKET`.
    pub path: String,
    /// The token that authenticates a sender, from `CLAUDE_CODE_MESSAGING_TOKEN`, when the session has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// The state file a Claude session's hooks write, one JSON object replaced atomically on every event.
///
/// Version 1, which the Python hook wrote:
/// `{"state": "working", "detail": "Run the tests", "tool": "Bash", "pid": 1123, "at": 1791418967.7,
///   "transcript": "…/id.jsonl", "turn": "…", "context": 212000, "contextLimit": 400000}`
/// Version 2 adds `v` and the fields below it: which session this is, where it runs, what it is working on
/// and how to reach it. Every one of them is optional, so a v1 file still decodes.
/// `pid` is the Claude process, so a session that died without a SessionEnd hook still reads as asleep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionReport {
    /// A [`Mood`] name, or anything else from a hook this app does not know, which reads as idle.
    pub state: String,
    /// One line on the state: the tool's step, the question, the reply's first line, the error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The running tool's name while working, such as "Bash", or `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The Claude process, when the hook could find it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// When the file was written, in seconds since the Unix epoch.
    pub at: f64,
    /// The session's transcript, for confirming that a quick reply arrived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    /// The prompt the current turn answers, shortened the way the hook shortens it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<String>,
    /// Tokens the session's last model call read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
    /// The context size at which the keep-alive replaces the session with a fresh one.
    #[serde(default, rename = "contextLimit", skip_serializing_if = "Option::is_none")]
    pub context_limit: Option<u64>,
    /// The file's version: absent in v1, 2 from Budd's own hook on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub v: Option<u32>,
    /// The session's id, the name of its transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The short id Claude Code's job runner uses, the name of the session's folder under `jobs/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_id: Option<String>,
    /// The session's name, as `claude --name` gave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The folder the session runs in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Claude Code's id for the prompt the current turn answers; surer than matching `turn`'s text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
    /// A fingerprint of the whole prompt the current turn answers, since `turn` is cut short.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_hash: Option<String>,
    /// The last slash command of the session's own work, such as "/lets-do-it CASBO-1", for its stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Links the last reply offered; their shape belongs to the reply module.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Value>,
    /// The permission mode the session runs in, such as "auto".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// The model the session runs on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Where to send the session a message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbox: Option<Inbox>,
    /// Why the session ended, from the SessionEnd hook, once it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<String>,
    /// When the Claude process started, so a reused pid is not mistaken for the session's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_start: Option<f64>,
}

impl SessionReport {
    /// A v1 report with only `state` and `at`; the other fields default to nothing.
    pub fn new(state: &str, at: f64) -> SessionReport {
        SessionReport {
            state: state.to_owned(),
            detail: None,
            tool: None,
            pid: None,
            at,
            transcript: None,
            turn: None,
            context: None,
            context_limit: None,
            v: None,
            session_id: None,
            short_id: None,
            name: None,
            cwd: None,
            prompt_id: None,
            turn_hash: None,
            command: None,
            links: Vec::new(),
            permission_mode: None,
            model: None,
            inbox: None,
            end_reason: None,
            pid_start: None,
        }
    }

    /// The report in a state file's text, or `None` when it is not one.
    pub fn decode(text: &str) -> Option<SessionReport> {
        serde_json::from_str(text).ok()
    }

    /// How full the session is, as a share of the size at which it is replaced, or `None` when unknown.
    pub fn fullness(&self) -> Option<f64> {
        match (self.context, self.context_limit) {
            (Some(context), Some(limit)) if limit > 0 => Some(context as f64 / limit as f64),
            _ => None,
        }
    }

    /// One short line about a fullness. It rounds down, so "100%" never shows before the session is due.
    pub fn fullness_label(fullness: f64) -> String {
        if fullness >= 1.0 {
            "Session full · fresh one when it's quiet".to_owned()
        } else {
            format!("Session {}% full", (fullness * 100.0) as u32)
        }
    }

    /// Whether this report is the end of the turn that answers `message`: finished (done or failed),
    /// and started by that message rather than by whatever was running when it was sent.
    ///
    /// `message` is the text alone; pictures sent with it show in the turn as "[Image #1]" markers.
    /// When the message was sent with a `prompt_id` and the report carries one, the ids decide.
    pub fn finishes_turn(&self, message: &str, prompt_id: Option<&str>) -> bool {
        if self.state != "done" && self.state != "failed" {
            return false;
        }
        if let (Some(mine), Some(theirs)) = (self.prompt_id.as_deref(), prompt_id) {
            return mine == theirs;
        }
        match &self.turn {
            Some(turn) => Self::turn_is_for(turn, message),
            None => false,
        }
    }

    /// Whether `turn`, the prompt as the hook records it, is `message`: the text alone, without the
    /// pictures' "[Image #1]" markers, and cut short as the hook cuts it.
    pub fn turn_is_for(turn: &str, message: &str) -> bool {
        // Claude Code wraps a long message, which arrives as one burst, in a pasted_content tag.
        let unwrapped = PASTE_OPEN.replace(&collapsed(turn), "").into_owned();
        let bare_marked = without_image_markers(&unwrapped);
        let pictured = bare_marked != unwrapped;
        let bare = collapsed(&bare_marked);
        // The hook cuts long prompts to 139 characters plus "…".
        let prompt = bare.strip_suffix('…').unwrap_or(&bare);
        let message = collapsed(message);
        // A picture sent on its own leaves nothing but its marker.
        if prompt.is_empty() {
            return pictured && message.is_empty();
        }
        message.starts_with(prompt)
    }
}

/// What the avatar shows: a mood and, for some moods, a line about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    /// The mood shown.
    pub mood: Mood,
    /// The line under it, when the state file had one.
    pub detail: Option<String>,
}

impl Presence {
    /// A presence with a mood and no detail.
    pub fn new(mood: Mood) -> Presence {
        Presence { mood, detail: None }
    }

    /// A presence with a mood and a detail line.
    pub fn with_detail(mood: Mood, detail: &str) -> Presence {
        Presence {
            mood,
            detail: Some(detail.to_owned()),
        }
    }

    /// One short line for the hover bubble.
    pub fn label(&self) -> String {
        let detail = self.detail.as_deref();
        match self.mood {
            Mood::Idle => "Idle".to_owned(),
            Mood::Thinking => "Thinking…".to_owned(),
            Mood::Working => detail.unwrap_or("Working…").to_owned(),
            Mood::Waiting => detail.unwrap_or("Needs you").to_owned(),
            Mood::Done => "Done".to_owned(),
            Mood::Failed => detail.unwrap_or("The turn failed").to_owned(),
            Mood::Sleeping => detail.unwrap_or("Asleep").to_owned(),
        }
    }
}

/// The rules that turn the newest state file into a presence, with time and liveness taken into account.
pub struct PresenceResolver;

impl PresenceResolver {
    /// How long "done" shows before the avatar settles back to idle, in seconds.
    pub const DONE_LINGERS: f64 = 8.0;
    /// An error stays up longer, so it is not missed.
    pub const FAILED_LINGERS: f64 = 60.0;
    /// A busy state this old, from a session that `claude agents` calls idle, is stale.
    pub const STALE_BUSY: f64 = 5.0;

    /// The presence for `report` at `now`.
    ///
    /// `session_idle` is whether `claude agents` reports the session idle right now, when known.
    /// A turn cut short by Esc fires no hook at all, so the file can say "working" forever; the
    /// session's own status wins over a busy state that is more than a few seconds old.
    /// `is_alive` says whether a process id is still running.
    pub fn resolve(
        report: Option<&SessionReport>,
        now: f64,
        session_idle: Option<bool>,
        is_alive: impl Fn(u32) -> bool,
    ) -> Presence {
        let Some(report) = report else {
            return Presence::with_detail(Mood::Sleeping, "No session yet");
        };
        if let Some(pid) = report.pid
            && !is_alive(pid)
        {
            return Presence::with_detail(Mood::Sleeping, "Session stopped");
        }
        let mood = Mood::parse(&report.state).unwrap_or(Mood::Idle);
        let age = now - report.at;
        if mood == Mood::Done && age > Self::DONE_LINGERS {
            return Presence::new(Mood::Idle);
        }
        if mood == Mood::Failed && age > Self::FAILED_LINGERS {
            return Presence::new(Mood::Idle);
        }
        if (mood == Mood::Thinking || mood == Mood::Working) && session_idle == Some(true) && age > Self::STALE_BUSY {
            return Presence::new(Mood::Idle);
        }
        Presence {
            mood,
            detail: report.detail.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alive(_: u32) -> bool {
        true
    }

    fn report(state: &str, detail: Option<&str>, pid: Option<u32>, at: f64) -> SessionReport {
        let mut report = SessionReport::new(state, at);
        report.detail = detail.map(str::to_owned);
        report.pid = pid;
        report
    }

    fn turn(state: &str, turn: Option<&str>) -> SessionReport {
        let mut report = SessionReport::new(state, 0.0);
        report.turn = turn.map(str::to_owned);
        report
    }

    #[test]
    fn missing_report_is_asleep() {
        assert_eq!(PresenceResolver::resolve(None, 0.0, None, alive).mood, Mood::Sleeping);
    }

    #[test]
    fn dead_session_is_asleep_whatever_it_last_said() {
        let report = report("working", Some("Run tests"), Some(42), 100.0);
        let presence = PresenceResolver::resolve(Some(&report), 101.0, None, |_| false);
        assert_eq!(presence, Presence::with_detail(Mood::Sleeping, "Session stopped"));
    }

    #[test]
    fn working_carries_its_detail() {
        let report = report("working", Some("Run tests"), Some(42), 100.0);
        let presence = PresenceResolver::resolve(Some(&report), 500.0, None, alive);
        assert_eq!(presence, Presence::with_detail(Mood::Working, "Run tests"));
        assert_eq!(presence.label(), "Run tests");
    }

    #[test]
    fn done_settles_to_idle() {
        let report = report("done", Some("All green"), None, 100.0);
        assert_eq!(
            PresenceResolver::resolve(Some(&report), 105.0, None, alive).mood,
            Mood::Done
        );
        assert_eq!(
            PresenceResolver::resolve(Some(&report), 109.0, None, alive),
            Presence::new(Mood::Idle)
        );
    }

    #[test]
    fn failed_shows_the_error_for_a_minute() {
        let report = report("failed", Some("You've hit your session limit"), None, 100.0);
        let presence = PresenceResolver::resolve(Some(&report), 130.0, None, alive);
        assert_eq!(presence.mood, Mood::Failed);
        assert_eq!(presence.label(), "You've hit your session limit");
        assert_eq!(
            PresenceResolver::resolve(Some(&report), 161.0, None, alive),
            Presence::new(Mood::Idle)
        );
    }

    #[test]
    fn stale_busy_gives_way_to_an_idle_session() {
        let report = report("working", Some("Run tests"), None, 100.0);
        // Fresh: trust the file, the agents list may simply lag behind.
        assert_eq!(
            PresenceResolver::resolve(Some(&report), 102.0, Some(true), alive).mood,
            Mood::Working
        );
        // Old and the session says idle: the turn ended without a hook (Esc).
        assert_eq!(
            PresenceResolver::resolve(Some(&report), 110.0, Some(true), alive),
            Presence::new(Mood::Idle)
        );
        // Busy sessions and unknown status keep the file's word.
        assert_eq!(
            PresenceResolver::resolve(Some(&report), 110.0, Some(false), alive).mood,
            Mood::Working
        );
        assert_eq!(
            PresenceResolver::resolve(Some(&report), 110.0, None, alive).mood,
            Mood::Working
        );
    }

    #[test]
    fn recognises_the_turn_that_answers_a_message() {
        let answer = turn("done", Some("What is a   chibi robot?"));
        assert!(answer.finishes_turn("What is a chibi robot?", None));
        // A turn that was already running when the message was queued is not its answer.
        assert!(!turn("done", Some("Run the tests")).finishes_turn("What is a chibi robot?", None));
        // Still going, or failed.
        assert!(!turn("working", Some("What is a chibi robot?")).finishes_turn("What is a chibi robot?", None));
        assert!(turn("failed", Some("What is a chibi robot?")).finishes_turn("What is a chibi robot?", None));
        // Long prompts are cut by the hook.
        let long = "word ".repeat(60);
        let cut = format!("{}…", long.chars().take(139).collect::<String>());
        assert!(turn("done", Some(&cut)).finishes_turn(&long, None));
        assert!(!turn("done", None).finishes_turn("anything", None));
        // A long message comes back wrapped as pasted content, line breaks and all.
        let pasted = turn("done", Some(r#"<pasted_content id="995b"> Long paste test. Line two…"#));
        assert!(pasted.finishes_turn("Long paste test.\nLine two says banana.", None));
        assert!(!pasted.finishes_turn("Something else", None));
    }

    #[test]
    fn recognises_the_turn_that_answers_a_picture() {
        // As the hook sees a message sent with a picture: the marker first, the path gone.
        let pictured = turn("done", Some("[Image #1]what colour is this?"));
        assert!(pictured.finishes_turn("what colour is this?", None));
        assert!(!pictured.finishes_turn("something else", None));
        // A picture alone.
        assert!(turn("done", Some("[Image #3]")).finishes_turn("", None));
        assert!(!turn("done", Some("")).finishes_turn("", None));
    }

    #[test]
    fn prompt_ids_decide_when_both_sides_have_one() {
        let mut answer = turn("done", Some("Run the tests"));
        answer.prompt_id = Some("p1".to_owned());
        assert!(answer.finishes_turn("Something else entirely", Some("p1")));
        assert!(!answer.finishes_turn("Run the tests", Some("p2")));
        // Without an id on one side, the text decides as before.
        assert!(answer.finishes_turn("Run the tests", None));
        assert!(turn("done", Some("Run the tests")).finishes_turn("Run the tests", Some("p1")));
        // An unfinished turn never answers, whatever the ids say.
        answer.state = "working".to_owned();
        assert!(!answer.finishes_turn("Run the tests", Some("p1")));
    }

    #[test]
    fn unknown_state_is_idle() {
        let report = SessionReport::new("dancing", 0.0);
        assert_eq!(
            PresenceResolver::resolve(Some(&report), 0.0, None, alive).mood,
            Mood::Idle
        );
    }

    #[test]
    fn decodes_the_hook_format() {
        let json = r#"{"state":"waiting","detail":"Which one?","pid":1123,"at":1791418967.7}"#;
        assert_eq!(
            SessionReport::decode(json),
            Some(report("waiting", Some("Which one?"), Some(1123), 1_791_418_967.7))
        );
        assert_eq!(SessionReport::decode("not json"), None);
    }

    #[test]
    fn decodes_the_v2_fields_and_writes_them_back() {
        let json = r#"{"state":"idle","at":1,"v":2,"session_id":"s","short_id":"f754f44d","name":"spike0",
            "cwd":"/w","prompt_id":"p","turn_hash":"h","command":"/lets-do-it X","links":[{"url":"u"}],
            "permission_mode":"auto","model":"opus","inbox":{"path":"/tmp/s.sock","token":"t"},
            "end_reason":"other","pid_start":12.5}"#;
        let report = SessionReport::decode(json).expect("decodes");
        assert_eq!(report.v, Some(2));
        assert_eq!(report.short_id.as_deref(), Some("f754f44d"));
        assert_eq!(report.inbox.as_ref().map(|i| i.path.as_str()), Some("/tmp/s.sock"));
        assert_eq!(report.links.len(), 1);
        let again = serde_json::to_string(&report).expect("serializes");
        assert_eq!(SessionReport::decode(&again), Some(report));
        // Nothing is written for what is not known.
        let bare = serde_json::to_string(&SessionReport::new("idle", 1.0)).expect("serializes");
        assert_eq!(bare, r#"{"state":"idle","at":1.0}"#);
    }

    #[test]
    fn decodes_how_full_the_session_is() {
        let json = r#"{"state":"idle","at":1,"context":212000,"contextLimit":400000}"#;
        assert_eq!(SessionReport::decode(json).and_then(|r| r.fullness()), Some(0.53));
        // Older hooks, a fresh session before its first call, or a limit that makes no sense.
        assert_eq!(SessionReport::new("idle", 1.0).fullness(), None);
        let mut report = SessionReport::new("idle", 1.0);
        report.context = Some(1000);
        assert_eq!(report.fullness(), None);
        report.context_limit = Some(0);
        assert_eq!(report.fullness(), None);
    }

    #[test]
    fn fullness_rounds_down_and_says_when_full() {
        assert_eq!(SessionReport::fullness_label(0.529), "Session 52% full");
        assert_eq!(SessionReport::fullness_label(0.999), "Session 99% full");
        assert_eq!(
            SessionReport::fullness_label(1.0),
            "Session full · fresh one when it's quiet"
        );
        assert_eq!(
            SessionReport::fullness_label(1.2),
            "Session full · fresh one when it's quiet"
        );
    }

    #[test]
    fn moods_have_names() {
        for mood in Mood::ALL {
            assert_eq!(Mood::parse(mood.name()), Some(mood));
        }
        assert_eq!(Mood::parse("dancing"), None);
        assert_eq!(Presence::new(Mood::Thinking).label(), "Thinking…");
        assert_eq!(Presence::new(Mood::Waiting).label(), "Needs you");
        assert_eq!(Presence::new(Mood::Sleeping).label(), "Asleep");
    }
}
