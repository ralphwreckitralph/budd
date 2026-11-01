//! The IPC schema between the Budd daemon and its clients (app, CLI, hook, MCP).
//!
//! Every message is one line of JSON. A client sends a [`Request`]: an `id` of its choosing and a
//! `cmd` naming one of [`Command`]'s variants, with that command's fields beside it. The daemon
//! answers each request with a [`Reply`] carrying the same `id`, `ok`, and either the reply's fields or
//! an `error`. A client that said `hello` also receives [`Event`]s, each with an `ev` naming its kind,
//! whenever something it subscribed to changes. Two lines break the pattern on purpose: `hook.poke`
//! gets no reply, and a bare tool call (`{"tool","arguments","callers"}`, with no `cmd`), which is what
//! `budd mcp` sends, is answered with a bare [`ToolResult`] (`{"text","isError"}`), so the existing MCP
//! server works unchanged.
//!
//! The protocol is version [`PROTO`]. Within a version every change is additive: new commands, events
//! and optional fields may appear, and a client ignores what it does not know.

use budd_core::account::AccountStatus;
use budd_core::crew::{JobState, LiveSession};
use budd_core::history::HistoryTurn;
use budd_core::presence::{Mood, Presence, SessionReport};
use budd_core::reply::{Evidence, PilotMode, ReplyLink};
use budd_core::tools::{BuddAsk, Call, ToolResult};
use budd_core::transcript::TranscriptSummary;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The protocol version this schema describes.
pub const PROTO: u32 = 1;

/// Which session a command is about: the assistant's own, or a crew member by its full session id.
///
/// On the wire the assistant is the string `"assistant"` and a crew member is `{"crew": "<session id>"}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    /// The assistant's bound session.
    Assistant,
    /// A crew member, by its full session id.
    Crew(String),
}

impl Target {
    /// The crew session id this names, or `None` for the assistant.
    pub fn crew(&self) -> Option<&str> {
        match self {
            Target::Assistant => None,
            Target::Crew(session) => Some(session),
        }
    }
}

/// The kinds of event a client may subscribe to in `hello`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Topic {
    /// [`Event::Sessions`].
    Sessions,
    /// [`Event::Presence`].
    Presence,
    /// [`Event::Crew`].
    Crew,
    /// [`Event::Reply`].
    Reply,
    /// [`Event::Ask`].
    Ask,
    /// [`Event::Unread`].
    Unread,
    /// [`Event::Evidence`].
    Evidence,
    /// [`Event::Account`].
    Account,
    /// [`Event::Say`].
    Say,
    /// [`Event::Log`].
    Log,
}

impl Topic {
    /// Every topic, for a client that wants everything.
    pub const ALL: [Topic; 10] = [
        Topic::Sessions,
        Topic::Presence,
        Topic::Crew,
        Topic::Reply,
        Topic::Ask,
        Topic::Unread,
        Topic::Evidence,
        Topic::Account,
        Topic::Say,
        Topic::Log,
    ];
}

/// What a client asks the daemon to do. The `cmd` field on the wire names the variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd")]
pub enum Command {
    /// The first line of a long-lived connection: who the client is, which protocol it speaks, and
    /// which events it wants. Answered with [`HelloReply`]. A missing `subscribe` means every topic.
    #[serde(rename = "hello")]
    Hello {
        /// The client's name, for the log: "ui", "watch", "cli".
        client: String,
        /// The protocol the client speaks; the daemon refuses one it does not.
        proto: u32,
        /// The events the client wants; `None` for all of them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subscribe: Option<Vec<Topic>>,
    },
    /// Sends a message to a session, with pictures and files as paths. Answered once the message has
    /// arrived, or could not be confirmed, with [`SendReply`].
    #[serde(rename = "send")]
    Send {
        /// Which session.
        target: Target,
        /// The text.
        message: String,
        /// Pictures to attach, as file paths without spaces.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pictures: Vec<String>,
        /// Files to attach, as file paths.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<String>,
    },
    /// Presses one of the answers a session offered: sends the text, and records that the human saw
    /// the ask. Answered with [`SendReply`].
    #[serde(rename = "answer")]
    Answer {
        /// Which session.
        target: Target,
        /// The answer's text.
        answer: String,
    },
    /// Stops a crew session so it leaves the office. One in the middle of work is not stopped unless
    /// `force`; the reply then says `asks_first`. Answered with [`ArchiveReply`].
    #[serde(rename = "archive")]
    Archive {
        /// The crew session id.
        session: String,
        /// Stop it even while it works.
        #[serde(default)]
        force: bool,
    },
    /// Starts one crew session per ticket key typed, in the project its prefix belongs to, named after
    /// the key. Nothing starts while any prefix is unclaimed. Answered with [`StartReply`].
    #[serde(rename = "start_tickets")]
    StartTickets {
        /// What the human typed, holding the keys.
        text: String,
    },
    /// Starts a crew session by name with a brief. Answered with [`StartReply`].
    #[serde(rename = "crew.start")]
    CrewStart {
        /// The session's name.
        name: String,
        /// Its first message.
        brief: String,
        /// The folder to start in.
        cwd: String,
        /// The model, when not the folder's default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// A folder to add to its reach.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        add_dir: Option<String>,
        /// Start with Claude in Chrome.
        #[serde(default)]
        chrome: bool,
    },
    /// Starts an office chat: a session in the assistant's folder named after the message's first
    /// words, which gets the message once it is listed. Answered with [`ChatReply`].
    #[serde(rename = "chat")]
    Chat {
        /// The first message.
        message: String,
        /// Pictures to attach.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pictures: Vec<String>,
        /// Files to attach.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        files: Vec<String>,
    },
    /// Clears a session's conversation, as `/clear` would. Answered with [`SendReply`].
    #[serde(rename = "clear")]
    Clear {
        /// Which session.
        target: Target,
    },
    /// Sets who answers the crew's asks, and whether the office is open. Fields left out are kept.
    #[serde(rename = "mode")]
    Mode {
        /// "human" or "autopilot".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pilot: Option<PilotMode>,
        /// Whether the office is open, kept across launches.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        office_open: Option<bool>,
    },
    /// The human has seen that a crew session is blocked, so it stops calling them over.
    #[serde(rename = "attention.seen")]
    AttentionSeen {
        /// The crew session id.
        session: String,
    },
    /// The human read the assistant's replies.
    #[serde(rename = "unread.read")]
    UnreadRead,
    /// A page of a session's conversation: the turns read so far, or with `earlier`, one more page
    /// back. Answered with [`HistoryReply`].
    #[serde(rename = "history.page")]
    HistoryPage {
        /// Which session.
        target: Target,
        /// Page further back before answering.
        #[serde(default)]
        earlier: bool,
    },
    /// The hook wrote a state file: read it now rather than at the next tick. No reply.
    #[serde(rename = "hook.poke")]
    HookPoke {
        /// The session whose file changed, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
    },
    /// A call to one of Budd's tools, from `budd mcp`. Answered with a [`ToolResult`] in the reply.
    #[serde(rename = "tool.call")]
    ToolCall {
        /// The call as the MCP server forwards it.
        #[serde(flatten)]
        call: Call,
    },
    /// The daemon's settings. Answered with [`Settings`].
    #[serde(rename = "settings.get")]
    SettingsGet,
    /// Changes `config.json`: `patch` is laid over the file (objects merge key by key, `null` removes,
    /// anything else replaces), the result is written and applied. Answered with [`Settings`].
    #[serde(rename = "settings.set")]
    SettingsSet {
        /// The partial config.
        patch: Value,
    },
    /// Runs the doctor's checks. Answered with [`DoctorReply`].
    #[serde(rename = "doctor.run")]
    DoctorRun,
    /// Forgets what Budd learned while running (unread replies, asks seen, autopilot's answers, the
    /// mode) and stops the assistant so it starts over; the config stays. Answered with a bare `ok`.
    #[serde(rename = "reset")]
    Reset,
    /// Stops the daemon.
    #[serde(rename = "shutdown")]
    Shutdown,
}

/// One line from a client: a request id and a command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// The client's id for the request, echoed in the reply; `hook.poke` may leave it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    /// What to do.
    #[serde(flatten)]
    pub command: Command,
}

/// Any line a client may send: a request, or a bare tool call from `budd mcp`.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientLine {
    /// A request with a `cmd`.
    Request(Request),
    /// A bare `{"tool","arguments","callers"}`, answered with a bare `ToolResult`.
    BareTool(Call),
}

impl ClientLine {
    /// The line decoded, or why it could not be.
    pub fn parse(line: &str) -> Result<ClientLine, String> {
        let value: Value = serde_json::from_str(line).map_err(|error| format!("not JSON: {error}"))?;
        let Some(object) = value.as_object() else {
            return Err("not a JSON object".to_owned());
        };
        if object.contains_key("cmd") {
            return serde_json::from_value(value)
                .map(ClientLine::Request)
                .map_err(|error| format!("not a request: {error}"));
        }
        if object.contains_key("tool") {
            return serde_json::from_value(value)
                .map(ClientLine::BareTool)
                .map_err(|error| format!("not a tool call: {error}"));
        }
        Err("a line needs a cmd".to_owned())
    }
}

/// The daemon's answer to a request: the request's `id`, `ok`, and the reply's fields or an `error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    /// The request's id.
    pub id: u64,
    /// Whether the request was carried out.
    pub ok: bool,
    /// Why not, when `ok` is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The reply's own fields, when `ok`.
    #[serde(flatten)]
    pub data: Map<String, Value>,
}

impl Reply {
    /// A reply that succeeded with no fields.
    pub fn done(id: u64) -> Reply {
        Reply {
            id,
            ok: true,
            error: None,
            data: Map::new(),
        }
    }

    /// A reply that succeeded with `data`'s fields.
    pub fn ok<T: Serialize>(id: u64, data: &T) -> Reply {
        let data = match serde_json::to_value(data) {
            Ok(Value::Object(map)) => map,
            Ok(other) => Map::from_iter([("value".to_owned(), other)]),
            Err(_) => Map::new(),
        };
        Reply {
            id,
            ok: true,
            error: None,
            data,
        }
    }

    /// A reply that failed, with why.
    pub fn error(id: u64, error: impl Into<String>) -> Reply {
        Reply {
            id,
            ok: false,
            error: Some(error.into()),
            data: Map::new(),
        }
    }

    /// The reply's fields as `T`, or `None` when they are not one or the reply failed.
    pub fn decode<T: DeserializeOwned>(&self) -> Option<T> {
        if !self.ok {
            return None;
        }
        serde_json::from_value(Value::Object(self.data.clone())).ok()
    }
}

/// The reply to `hello`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloReply {
    /// The protocol the daemon speaks.
    pub proto: u32,
    /// The daemon's version.
    pub daemon_version: String,
    /// Everything the client would have learned from events so far.
    pub snapshot: Snapshot,
}

/// The daemon's state at one moment, as the events would have told it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Snapshot {
    /// The sessions.
    pub sessions: SessionsEvent,
    /// The assistant's presence.
    pub presence: PresenceEvent,
    /// The crew.
    pub crew: CrewEvent,
    /// Who calls the human over.
    pub ask: AskEvent,
    /// The unread replies.
    pub unread: UnreadEvent,
    /// The account, once it is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<AccountEvent>,
    /// The daemon's settings.
    pub settings: Settings,
}

/// The reply to `send`, `answer` and `clear`: whether it arrived, or why that could not be confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendReply {
    /// The message showed up in the session's transcript.
    pub delivered: bool,
    /// Why it could not be confirmed, when it was not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The reply to `archive`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveReply {
    /// The session was stopped.
    pub stopped: bool,
    /// The session is working, so it was not stopped; send again with `force` to stop it anyway.
    #[serde(default)]
    pub asks_first: bool,
}

/// The reply to `start_tickets` and `crew.start`: the sessions started, and the ones `claude --bg`
/// refused, each as "name: reason".
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StartReply {
    /// The names started, in the order asked.
    pub started: Vec<String>,
    /// What could not start, as "name: reason".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<String>,
}

/// The reply to `chat`: the chat's name, and whether its first message arrived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatReply {
    /// The session's name, such as "chat-where-is-the-dev".
    pub name: String,
    /// The message showed up in its transcript.
    pub delivered: bool,
    /// Why it could not be confirmed, when it was not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The reply to `doctor.run`: one entry per check, each `{name, state, detail}` with `state` one of
/// "ok", "warn", "fail".
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DoctorReply {
    /// The checks, in the report's order.
    pub checks: Vec<Value>,
}

/// The reply to `history.page`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryReply {
    /// Every turn read so far, oldest first; the last may still be under way.
    pub turns: Vec<HistoryTurn>,
    /// There is more to page in before the first turn.
    pub has_earlier: bool,
}

/// The daemon's settings, as `settings.get` reports them.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Settings {
    /// Budd's home folder.
    pub home: String,
    /// The `CLAUDE_CONFIG_DIR` of Budd's own account.
    pub config_dir: String,
    /// The name of the assistant's session.
    pub assistant_name: String,
    /// Who answers the crew's asks.
    pub pilot_mode: PilotMode,
    /// Whether the office is open.
    pub office_open: bool,
    /// The context size at which a crew session counts as full.
    pub crew_context_limit: u64,
    /// The context size at which the keep-alive replaces the assistant.
    pub rotate_at: u64,
    /// The `claude` binary in use, when found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<String>,
    /// The daemon's version.
    pub version: String,
    /// The whole `config.json` as the daemon has it, with the defaults filled in.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub config: Value,
}

/// What a crew member's transcript says about where it is, as the wire carries it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Summary {
    /// The human's last message, or the slash command they ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// The transcript entry that message is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
    /// The session's reply to it so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    /// The message was taken in mid-turn.
    #[serde(default)]
    pub mid_turn: bool,
    /// The newest tool call still without a result, in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<String>,
    /// The last slash command of the session's own work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Tokens the last model call read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<u64>,
}

impl From<&TranscriptSummary> for Summary {
    fn from(summary: &TranscriptSummary) -> Summary {
        Summary {
            prompt: summary.prompt.clone(),
            prompt_id: summary.prompt_id.clone(),
            reply: summary.reply.clone(),
            mid_turn: summary.mid_turn,
            pending: summary.pending.clone(),
            command: summary.command.clone(),
            context: summary.context,
        }
    }
}

/// How a crew desk reads: needs the human, working, or idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeskState {
    /// Waiting on the human.
    NeedsYou,
    /// In a turn, or autopilot is on it.
    Working,
    /// At its prompt.
    Idle,
}

/// One crew session as the office shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrewMember {
    /// The session as `claude agents` lists it.
    pub session: LiveSession,
    /// What its transcript says.
    pub summary: Summary,
    /// What Claude Code's job runner says about it, when it keeps a file for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<JobState>,
    /// The evidence set of the ticket it works on, once the pipeline has started one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Evidence>,
    /// What it asked the human through Budd's `ask` tool this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<BuddAsk>,
    /// Its status line, set through Budd's `status` tool this turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Its last reply, ending in the question it put through Budd while that stands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    /// One line on where it stands: what it told Budd, else what the job runner says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Retired by Claude Code while it waited on the human; a message wakes it.
    pub asleep: bool,
    /// Stopped on a question or permission prompt, which needs the terminal's picker.
    pub asks: bool,
    /// Its turn ended waiting on the human.
    pub blocked: bool,
    /// Either of the two.
    pub needs_you: bool,
    /// The stage of its ticket, or "chat".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// How long it has been quiet, coarsely; `None` while it works.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet: Option<String>,
    /// How full it is, against the budget for crew sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fullness: Option<f64>,
    /// What autopilot did or will do about its ask, in a line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autopilot: Option<String>,
    /// How its desk reads.
    pub desk: DeskState,
    /// The line under its desk.
    pub line: String,
    /// It has a reply the human has not looked at.
    pub is_new: bool,
    /// It calls the human over right now.
    pub loud: bool,
    /// What it pointed the human at in its replies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<ReplyLink>,
    /// When its transcript last changed, wall-clock seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<f64>,
}

/// A session waiting on the human, as its ask says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrewAsk {
    /// The session, or the assistant.
    pub target: Target,
    /// The short id, what `claude attach` takes.
    pub short_id: String,
    /// The session's name.
    pub name: String,
    /// What it wants, in a line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub what: Option<String>,
    /// It stopped on a prompt that needs the terminal's picker.
    pub picker: bool,
    /// The answers it offers as buttons.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<String>,
    /// "asleep 21h" when it was retired while it waited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asleep: Option<String>,
}

/// The sessions `claude agents` lists, and which one is the assistant's.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SessionsEvent {
    /// The assistant's bound session, when one is live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<LiveSession>,
    /// Every session on the bridge, oldest first.
    pub sessions: Vec<LiveSession>,
    /// The sessions have been listed at least once.
    pub listed: bool,
}

/// What the assistant's desk shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresenceEvent {
    /// The mood and its line.
    pub presence: Presence,
    /// The state file as last read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<SessionReport>,
    /// How full the assistant's session is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fullness: Option<f64>,
    /// The assistant's last full reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reply: Option<String>,
    /// When that reply was written, wall-clock seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reply_at: Option<f64>,
    /// Who answers the crew's asks.
    pub pilot_mode: PilotMode,
    /// Whether the office is open.
    pub office_open: bool,
    /// What the assistant is doing while a sent message awaits its answer, when one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awaiting: Option<Awaiting>,
}

impl Default for PresenceEvent {
    fn default() -> Self {
        PresenceEvent {
            presence: Presence::with_detail(Mood::Sleeping, "No session yet"),
            report: None,
            fullness: None,
            last_reply: None,
            last_reply_at: None,
            pilot_mode: PilotMode::Human,
            office_open: true,
            awaiting: None,
        }
    }
}

/// A sent message whose answer is awaited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Awaiting {
    /// Which session it went to.
    pub target: Target,
    /// The message as the card shows it.
    pub label: String,
    /// When it went, wall-clock seconds.
    pub since: f64,
}

/// The crew as the office shows it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CrewEvent {
    /// Every member, oldest first, without the ones just archived.
    pub members: Vec<CrewMember>,
}

/// A message's answer, or its loss: the answer the session gave to a message sent through Budd, an
/// ask the assistant pinned, or word that the message will never be answered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplyEvent {
    /// Which session.
    pub target: Target,
    /// The message, as the card shows it.
    pub prompt: String,
    /// The answer, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<String>,
    /// Why there will be no answer, when there will not be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost: Option<String>,
    /// When the answer was written, or the loss noticed, wall-clock seconds.
    pub at: f64,
}

/// Who calls the human over right now.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AskEvent {
    /// The asks standing.
    pub asks: Vec<CrewAsk>,
}

/// The assistant's unread replies.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UnreadEvent {
    /// How many are unread.
    pub count: usize,
    /// When the newest reply seen was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newest: Option<f64>,
    /// When the last unread one arrived, wall-clock seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrived_at: Option<f64>,
}

/// A crew member's evidence set changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceEvent {
    /// The crew session id.
    pub session: String,
    /// The set, or `None` once it is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Evidence>,
}

/// Who Budd's config folder is logged in as.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountEvent {
    /// The status, or `None` when `claude auth status` could not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AccountStatus>,
    /// Why it could not say, when it could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A line a session asked Budd to say out loud.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SayEvent {
    /// The line.
    pub text: String,
    /// Which session asked.
    pub target: Target,
    /// Its name.
    pub name: String,
}

/// A line of the daemon's log worth showing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEvent {
    /// "info", "warn" or "error".
    pub level: String,
    /// The line.
    pub message: String,
}

/// What the daemon tells its subscribers. The `ev` field on the wire names the variant.
// A presence event carries the whole state file and the others a few fields; each is built once and
// serialized, so the size difference costs nothing worth a Box in every client's match.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ev", rename_all = "lowercase")]
pub enum Event {
    /// The sessions changed.
    Sessions(SessionsEvent),
    /// The assistant's presence changed.
    Presence(PresenceEvent),
    /// The crew changed.
    Crew(CrewEvent),
    /// A message was answered or lost.
    Reply(ReplyEvent),
    /// Who calls the human over changed.
    Ask(AskEvent),
    /// The unread count changed.
    Unread(UnreadEvent),
    /// A crew member's evidence changed.
    Evidence(EvidenceEvent),
    /// The account was looked up.
    Account(AccountEvent),
    /// A session asked Budd to say something.
    Say(SayEvent),
    /// A line of the log.
    Log(LogEvent),
}

impl Event {
    /// The topic this event belongs to.
    pub fn topic(&self) -> Topic {
        match self {
            Event::Sessions(_) => Topic::Sessions,
            Event::Presence(_) => Topic::Presence,
            Event::Crew(_) => Topic::Crew,
            Event::Reply(_) => Topic::Reply,
            Event::Ask(_) => Topic::Ask,
            Event::Unread(_) => Topic::Unread,
            Event::Evidence(_) => Topic::Evidence,
            Event::Account(_) => Topic::Account,
            Event::Say(_) => Topic::Say,
            Event::Log(_) => Topic::Log,
        }
    }
}

/// The reply to a `tool.call` request: the tool result's fields.
pub type ToolReply = ToolResult;

#[cfg(test)]
mod tests {
    use super::*;
    use budd_core::crew::{AgentState, Status};
    use serde_json::json;

    fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: &T) -> Value {
        let text = serde_json::to_string(value).expect("serializes");
        let back: T = serde_json::from_str(&text).expect("deserializes");
        assert_eq!(&back, value);
        serde_json::from_str(&text).expect("json")
    }

    fn session(name: &str) -> LiveSession {
        LiveSession {
            id: "a1b2c3d4".to_owned(),
            session_id: "11111111-1111-4111-8111-111111111111".to_owned(),
            name: name.to_owned(),
            pid: Some(4242),
            cwd: "/w".to_owned(),
            status: Some(Status::Idle),
            state: Some(AgentState::Blocked),
            waiting_for: None,
            started_at: 1.0,
        }
    }

    #[test]
    fn every_command_round_trips_with_its_cmd_name() {
        let commands = vec![
            (
                Command::Hello {
                    client: "ui".into(),
                    proto: PROTO,
                    subscribe: Some(vec![Topic::Sessions, Topic::Crew]),
                },
                "hello",
            ),
            (
                Command::Send {
                    target: Target::Assistant,
                    message: "hi".into(),
                    pictures: vec!["/tmp/a.png".into()],
                    files: vec![],
                },
                "send",
            ),
            (
                Command::Answer {
                    target: Target::Crew("s1".into()),
                    answer: "skip".into(),
                },
                "answer",
            ),
            (
                Command::Archive {
                    session: "s1".into(),
                    force: true,
                },
                "archive",
            ),
            (
                Command::StartTickets {
                    text: "CASBO-1 GNX-2".into(),
                },
                "start_tickets",
            ),
            (
                Command::CrewStart {
                    name: "casbo-1".into(),
                    brief: "do it".into(),
                    cwd: "/w".into(),
                    model: None,
                    add_dir: Some("/h".into()),
                    chrome: false,
                },
                "crew.start",
            ),
            (
                Command::Chat {
                    message: "where is it".into(),
                    pictures: vec![],
                    files: vec![],
                },
                "chat",
            ),
            (
                Command::Clear {
                    target: Target::Assistant,
                },
                "clear",
            ),
            (
                Command::Mode {
                    pilot: Some(PilotMode::Autopilot),
                    office_open: None,
                },
                "mode",
            ),
            (Command::AttentionSeen { session: "s1".into() }, "attention.seen"),
            (Command::UnreadRead, "unread.read"),
            (
                Command::HistoryPage {
                    target: Target::Crew("s1".into()),
                    earlier: true,
                },
                "history.page",
            ),
            (Command::HookPoke { session: None }, "hook.poke"),
            (
                Command::ToolCall {
                    call: Call {
                        tool: "say".into(),
                        arguments: json!({"text": "hi"}),
                        callers: vec![1, 2],
                    },
                },
                "tool.call",
            ),
            (Command::SettingsGet, "settings.get"),
            (
                Command::SettingsSet {
                    patch: json!({"sounds": false}),
                },
                "settings.set",
            ),
            (Command::DoctorRun, "doctor.run"),
            (Command::Reset, "reset"),
            (Command::Shutdown, "shutdown"),
        ];
        for (command, name) in commands {
            let request = Request { id: Some(7), command };
            let json = round_trip(&request);
            assert_eq!(json["cmd"], name, "{json}");
            assert_eq!(json["id"], 7);
        }
    }

    #[test]
    fn the_wire_shapes_are_as_designed() {
        let line = r#"{"id":1,"cmd":"hello","client":"ui","proto":1,"subscribe":["sessions","crew"]}"#;
        match ClientLine::parse(line).expect("parses") {
            ClientLine::Request(Request {
                id: Some(1),
                command:
                    Command::Hello {
                        client,
                        proto,
                        subscribe,
                    },
            }) => {
                assert_eq!(client, "ui");
                assert_eq!(proto, 1);
                assert_eq!(subscribe, Some(vec![Topic::Sessions, Topic::Crew]));
            }
            other => panic!("{other:?}"),
        }
        let tool = r#"{"id":2,"cmd":"tool.call","tool":"status","arguments":{"line":"on it"},"callers":[4242]}"#;
        match ClientLine::parse(tool).expect("parses") {
            ClientLine::Request(Request {
                command: Command::ToolCall { call },
                ..
            }) => {
                assert_eq!(call.tool, "status");
                assert_eq!(call.callers, [4242]);
            }
            other => panic!("{other:?}"),
        }
        // The bare MCP wire, with no cmd.
        let bare = r#"{"tool":"say","arguments":{"text":"hi"},"callers":[1]}"#;
        assert!(matches!(ClientLine::parse(bare), Ok(ClientLine::BareTool(call)) if call.tool == "say"));
        // A poke needs no id.
        assert!(matches!(
            ClientLine::parse(r#"{"cmd":"hook.poke"}"#),
            Ok(ClientLine::Request(Request {
                id: None,
                command: Command::HookPoke { session: None }
            }))
        ));
        assert!(ClientLine::parse("nope").is_err());
        assert!(ClientLine::parse("[]").is_err());
        assert!(ClientLine::parse("{}").is_err());
        assert!(ClientLine::parse(r#"{"cmd":"dance"}"#).is_err());
        // Targets.
        assert_eq!(
            serde_json::to_string(&Target::Assistant).expect("json"),
            r#""assistant""#
        );
        assert_eq!(
            serde_json::to_string(&Target::Crew("s1".into())).expect("json"),
            r#"{"crew":"s1"}"#
        );
        assert_eq!(Target::Crew("s1".into()).crew(), Some("s1"));
        assert_eq!(Target::Assistant.crew(), None);
    }

    #[test]
    fn replies_carry_ok_and_either_fields_or_an_error() {
        let done = round_trip(&Reply::done(3));
        assert_eq!(done, json!({"id": 3, "ok": true}));
        let sent = Reply::ok(
            4,
            &SendReply {
                delivered: false,
                reason: Some("timed out".into()),
            },
        );
        let json = round_trip(&sent);
        assert_eq!(
            json,
            json!({"id": 4, "ok": true, "delivered": false, "reason": "timed out"})
        );
        assert_eq!(
            sent.decode::<SendReply>(),
            Some(SendReply {
                delivered: false,
                reason: Some("timed out".into())
            })
        );
        let failed = Reply::error(5, "not yet");
        assert_eq!(round_trip(&failed), json!({"id": 5, "ok": false, "error": "not yet"}));
        assert_eq!(failed.decode::<SendReply>(), None);
        let hello = Reply::ok(
            1,
            &HelloReply {
                proto: PROTO,
                daemon_version: "0.0.0".into(),
                snapshot: Snapshot::default(),
            },
        );
        let json = round_trip(&hello);
        assert_eq!(json["proto"], 1);
        assert!(json["snapshot"]["presence"]["presence"]["mood"] == "sleeping");
        let tool = Reply::ok(6, &ToolResult::ok("Said."));
        assert_eq!(
            round_trip(&tool),
            json!({"id": 6, "ok": true, "text": "Said.", "isError": false})
        );
        round_trip(&Reply::ok(
            7,
            &ArchiveReply {
                stopped: false,
                asks_first: true,
            },
        ));
        round_trip(&Reply::ok(
            8,
            &HistoryReply {
                turns: vec![HistoryTurn::new(Some("u1".into()), Some("hi".into()), None, Some(5))],
                has_earlier: true,
            },
        ));
        round_trip(&Reply::ok(9, &Settings::default()));
    }

    #[test]
    fn every_event_round_trips_with_its_ev_name() {
        let member = CrewMember {
            session: session("casbo-1"),
            summary: Summary {
                prompt: Some("/lets-do-it CASBO-1".into()),
                prompt_id: Some("u1".into()),
                reply: Some("Done.\n\nReply: `ok`".into()),
                mid_turn: false,
                pending: None,
                command: Some("/lets-do-it CASBO-1".into()),
                context: Some(1000),
            },
            job: Some(JobState {
                state: Some("blocked".into()),
                detail: Some("waiting".into()),
                intent: Some("/lets-do-it CASBO-1".into()),
            }),
            evidence: None,
            ask: Some(BuddAsk::new("Go?", vec!["go".into()], true)),
            status: Some("testing".into()),
            reply: Some("Done.\n\nReply: `ok`".into()),
            detail: Some("testing".into()),
            asleep: false,
            asks: false,
            blocked: true,
            needs_you: true,
            stage: Some("do".into()),
            quiet: Some("12m".into()),
            fullness: Some(0.5),
            autopilot: None,
            desk: DeskState::NeedsYou,
            line: "waiting".into(),
            is_new: true,
            loud: true,
            links: vec![],
            updated: Some(10.0),
        };
        let events = vec![
            (
                Event::Sessions(SessionsEvent {
                    bound: Some(session("assistant")),
                    sessions: vec![session("assistant"), session("casbo-1")],
                    listed: true,
                }),
                "sessions",
            ),
            (Event::Presence(PresenceEvent::default()), "presence"),
            (
                Event::Crew(CrewEvent {
                    members: vec![member.clone()],
                }),
                "crew",
            ),
            (
                Event::Reply(ReplyEvent {
                    target: Target::Assistant,
                    prompt: "hi".into(),
                    reply: Some("hello".into()),
                    lost: None,
                    at: 1.0,
                }),
                "reply",
            ),
            (
                Event::Ask(AskEvent {
                    asks: vec![CrewAsk {
                        target: Target::Crew("s1".into()),
                        short_id: "a1".into(),
                        name: "casbo-1".into(),
                        what: Some("Go?".into()),
                        picker: false,
                        answers: vec!["go".into()],
                        asleep: None,
                    }],
                }),
                "ask",
            ),
            (
                Event::Unread(UnreadEvent {
                    count: 2,
                    newest: Some(3.0),
                    arrived_at: None,
                }),
                "unread",
            ),
            (
                Event::Evidence(EvidenceEvent {
                    session: "s1".into(),
                    evidence: None,
                }),
                "evidence",
            ),
            (
                Event::Account(AccountEvent {
                    status: AccountStatus::decode(r#"{"loggedIn": true, "email": "a@b"}"#),
                    error: None,
                }),
                "account",
            ),
            (
                Event::Say(SayEvent {
                    text: "shipped".into(),
                    target: Target::Crew("s1".into()),
                    name: "casbo-1".into(),
                }),
                "say",
            ),
            (
                Event::Log(LogEvent {
                    level: "info".into(),
                    message: "hello".into(),
                }),
                "log",
            ),
        ];
        for (event, name) in events {
            let json = round_trip(&event);
            assert_eq!(json["ev"], name, "{json}");
            assert_eq!(serde_json::to_value(event.topic()).expect("json"), name);
        }
        let json = round_trip(&member);
        assert_eq!(json["desk"], "needs_you");
        assert_eq!(json["session"]["status"], "idle");
        assert_eq!(json["session"]["state"], "blocked");
        assert_eq!(Summary::from(&TranscriptSummary::default()), Summary::default());
    }

    #[test]
    fn unknown_fields_are_ignored_so_the_protocol_can_grow() {
        let line = r#"{"id":1,"cmd":"unread.read","later":true}"#;
        assert!(matches!(
            ClientLine::parse(line),
            Ok(ClientLine::Request(Request {
                command: Command::UnreadRead,
                ..
            }))
        ));
        let event: Event = serde_json::from_str(r#"{"ev":"unread","count":1,"something_new":[1]}"#).expect("decodes");
        assert_eq!(
            event,
            Event::Unread(UnreadEvent {
                count: 1,
                newest: None,
                arrived_at: None
            })
        );
        assert_eq!(Topic::ALL.len(), 10);
    }
}
