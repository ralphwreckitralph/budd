//! The state machine: what Budd knows, what he does about it, and what he tells his clients.
//!
//! [`Engine`] is driven from outside: the daemon feeds it the sessions listing, the state files, the
//! clock and the clients' requests, and takes back [`Effect`]s to carry out (sends, stops, starts, which
//! take seconds and run off the engine's thread) and [`Outgoing`] lines (replies to clients, events for
//! subscribers). Nothing in here blocks or waits, so the whole machine runs in tests against a mock
//! host, one input at a time. The rules are the macOS app's: `SessionWatcher.swift` for what is read
//! and when, `AvatarController.swift` for catching replies, the crew's rows, autopilot and the tools.

use crate::config::Config;
use crate::host::{ClaudeHost, Outcome, SendTarget};
use crate::state::Persisted;
use budd_claude::watch::{Cadence, JobInfo};
use budd_core::crew::{
    AgentRow, ArchivedCrew, CrewAttention, CrewBoard, LiveSession, OfficeChat, Status, assistant_flags, live_sessions,
};
use budd_core::delivery::{PathStyle, attachment_note, sendable, with_files};
use budd_core::history::{ConversationHistory, TranscriptSource};
use budd_core::keepalive::{self, Decision};
use budd_core::presence::{Mood, Presence, PresenceResolver, SessionReport};
use budd_core::reply::{Autopilot, Evidence, PilotMode, ReplyLink, ReplyOptions, UnreadReplies};
use budd_core::tools::{BuddAsk, BuddBoard, BuddRequest, Call, ToolResult};
use budd_core::transcript::{FIRST_TAIL, TranscriptFollower, TranscriptSummary, grown_tail, tail_start, whole_lines};
use budd_proto::{
    AccountEvent, ArchiveReply, AskEvent, Awaiting as AwaitingWire, Command, CrewAsk, CrewEvent, CrewMember, DeskState,
    Event, EvidenceEvent, HelloReply, HistoryReply, LogEvent, PROTO, PresenceEvent, Reply, ReplyEvent, Request,
    SayEvent, SendReply, SessionsEvent, Settings, Snapshot, Summary, Target, UnreadEvent,
};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// One connected client, as the engine tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(pub u64);

/// A line for a client: a reply to one of them, or an event for every subscriber of its topic.
#[derive(Debug, Clone, PartialEq)]
pub enum Outgoing {
    /// The answer to one client's request.
    Reply {
        /// Who asked.
        client: ClientId,
        /// The answer.
        reply: Reply,
    },
    /// Something changed.
    Event(Event),
}

/// Something the engine wants done off its thread, since it takes seconds or a process.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Send a message and wait for it to show in the transcript.
    Send {
        /// Matches the [`Completion`].
        id: u64,
        /// Where.
        target: SendTarget,
        /// The text, files' paths included.
        message: String,
        /// Pictures to attach.
        pictures: Vec<String>,
    },
    /// Clear a session's conversation.
    Clear {
        /// Matches the [`Completion`].
        id: u64,
        /// Which session.
        target: SendTarget,
    },
    /// Stop a background session.
    Stop {
        /// Matches the [`Completion`].
        id: u64,
        /// Its short id.
        short_id: String,
    },
    /// Start a background session.
    StartBg {
        /// Matches the [`Completion`].
        id: u64,
        /// In this folder.
        cwd: PathBuf,
        /// With these arguments.
        argv: Vec<String>,
    },
    /// List the sessions now rather than at the next interval.
    ListNow,
    /// Look at the state files now.
    ReadStateNow,
    /// Stop the daemon.
    Shutdown,
}

/// How an effect ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Completion {
    /// A [`Effect::Send`] ended.
    Sent {
        /// The effect's id.
        id: u64,
        /// How.
        outcome: Outcome,
    },
    /// A [`Effect::Clear`] ended.
    Cleared {
        /// The effect's id.
        id: u64,
        /// How.
        outcome: Outcome,
    },
    /// A [`Effect::Stop`] ended.
    Stopped {
        /// The effect's id.
        id: u64,
        /// How.
        result: Result<(), String>,
    },
    /// A [`Effect::StartBg`] ended.
    Started {
        /// The effect's id.
        id: u64,
        /// How.
        result: Result<Vec<String>, String>,
    },
}

/// Why an effect was started, kept until it completes.
#[derive(Debug, Clone)]
enum Pending {
    /// A message or an answer from a client; its reply waits on the outcome.
    Message {
        client: ClientId,
        request: u64,
        awaiting: Awaiting,
    },
    /// Autopilot answering a crew session's ask.
    Autopilot {
        session: String,
        name: String,
        reply: Option<String>,
        answer: String,
    },
    /// A clear from a client.
    Clear {
        client: ClientId,
        request: u64,
        target: Target,
    },
    /// An archive from a client: the stop of a crew session.
    Archive {
        client: ClientId,
        request: u64,
        session: String,
        name: String,
    },
    /// The keep-alive stopping the assistant before starting a fresh one.
    KeepaliveStop { reason: String },
    /// The keep-alive starting a fresh assistant.
    KeepaliveStart { reason: String },
}

/// A message sent through Budd whose answer is awaited.
#[derive(Debug, Clone, PartialEq)]
struct Awaiting {
    /// When it went.
    since: f64,
    /// The text alone, as sent.
    message: String,
    /// How the card shows it.
    label: String,
    /// Which session.
    target: Target,
    /// The assistant's session id when it went, to notice it replaced.
    bound_id: Option<String>,
    /// Claude Code's id for the prompt, once a hook event names it.
    prompt_id: Option<String>,
}

/// What the hook wrote for one session, as last read.
#[derive(Debug, Clone, Default, PartialEq)]
struct Files {
    report: Option<SessionReport>,
    reply: Option<(String, f64)>,
}

/// One crew session as the engine follows it.
struct Member {
    session: LiveSession,
    path: PathBuf,
    follower: TranscriptFollower,
    modified: Option<f64>,
    job: Option<JobInfo>,
    evidence: Option<Evidence>,
}

impl Member {
    fn new(session: LiveSession, path: PathBuf) -> Member {
        Member {
            session,
            path,
            follower: TranscriptFollower::new(),
            modified: None,
            job: None,
            evidence: None,
        }
    }

    fn summary(&self) -> &TranscriptSummary {
        self.follower.summary()
    }
}

/// What was last told to subscribers, so nothing is told twice.
#[derive(Default)]
struct Told {
    sessions: Option<SessionsEvent>,
    presence: Option<PresenceEvent>,
    crew: Option<CrewEvent>,
    ask: Option<AskEvent>,
    unread: Option<UnreadEvent>,
    evidence: HashMap<String, Option<Evidence>>,
    persisted: Option<Persisted>,
}

/// The engine. See the module's notes.
pub struct Engine {
    host: Arc<dyn ClaudeHost>,
    config: Config,
    sessions: Vec<LiveSession>,
    listed: bool,
    bound: Option<LiveSession>,
    bound_at: f64,
    files: HashMap<String, Files>,
    report: Option<SessionReport>,
    last_reply: Option<String>,
    last_reply_at: Option<f64>,
    presence: Presence,
    crew: Vec<Member>,
    board: BuddBoard,
    attention: CrewAttention,
    archived: ArchivedCrew,
    pilot: Autopilot,
    pilot_mode: PilotMode,
    unread: UnreadReplies,
    office_open: bool,
    crew_seen: HashMap<String, f64>,
    awaiting: Option<Awaiting>,
    transcript_checked_at: Option<f64>,
    assistant_ask: Option<(BuddAsk, Option<String>, f64)>,
    piloting: HashSet<String>,
    sending: bool,
    pending: HashMap<u64, Pending>,
    next_effect: u64,
    effects: Vec<Effect>,
    outgoing: Vec<Outgoing>,
    retry_calls: Vec<(ClientId, u64, Call)>,
    histories: HashMap<String, ConversationHistory>,
    account: Option<AccountEvent>,
    told: Told,
    keepalive_busy: bool,
}

/// The line a crew session is given when it asks through Budd.
const ASK_TAKEN: &str =
    "The question is at your desk in Budd's office. End your turn; the answer arrives as your next message.";
/// The line the assistant is given when it asks through Budd.
const ASSISTANT_ASK_TAKEN: &str =
    "The question is on Budd's card with its answers. End your turn; the answer arrives as your next message.";
/// The line the assistant is given when it sets a status.
const ASSISTANT_STATUS: &str =
    "The assistant's step already shows on Budd's desk from its hook; status is for crew sessions.";
/// The line a caller Budd does not know is given.
const UNKNOWN_CALLER: &str =
    "Budd only takes calls from background sessions in his office, and this one is not among them.";
/// How long a sent message waits for its answer before Budd gives up.
const GIVE_UP: f64 = 15.0 * 60.0;
/// How long after a send an idle assistant means the turn ended without a hook.
const IDLE_MEANS_DONE: f64 = 6.0;
/// How long a crew transcript must be quiet before its reply counts as finished.
const CREW_QUIET: f64 = 2.0;
/// How long the assistant's pinned ask stands.
const ASK_LINGERS: f64 = 10.0 * 60.0;

impl Engine {
    /// An engine over `host` with `config`, remembering what the last run saved.
    pub fn new(host: Arc<dyn ClaudeHost>, config: Config) -> Engine {
        let persisted = Persisted::load(&config.state_path());
        Self::with_state(host, config, persisted)
    }

    /// An engine starting from `persisted` rather than the file.
    pub fn with_state(host: Arc<dyn ClaudeHost>, config: Config, persisted: Persisted) -> Engine {
        Engine {
            host,
            config,
            sessions: Vec::new(),
            listed: false,
            bound: None,
            bound_at: 0.0,
            files: HashMap::new(),
            report: None,
            last_reply: None,
            last_reply_at: None,
            presence: Presence::with_detail(Mood::Sleeping, "No assistant session running"),
            crew: Vec::new(),
            board: BuddBoard::default(),
            attention: CrewAttention::new(persisted.crew_attention.clone()),
            archived: ArchivedCrew::new(),
            pilot: Autopilot::from_plist(&persisted.autopilot_answers),
            pilot_mode: persisted.pilot_mode,
            unread: UnreadReplies::restored(persisted.unread_count, persisted.unread_newest),
            office_open: persisted.office_open,
            crew_seen: HashMap::new(),
            awaiting: None,
            transcript_checked_at: None,
            assistant_ask: None,
            piloting: HashSet::new(),
            sending: false,
            pending: HashMap::new(),
            next_effect: 1,
            effects: Vec::new(),
            outgoing: Vec::new(),
            retry_calls: Vec::new(),
            histories: HashMap::new(),
            account: None,
            told: Told {
                persisted: Some(persisted),
                ..Told::default()
            },
            keepalive_busy: false,
        }
    }

    /// The configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The effects queued since the last take, oldest first.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// The lines queued since the last take, oldest first.
    pub fn take_outgoing(&mut self) -> Vec<Outgoing> {
        std::mem::take(&mut self.outgoing)
    }

    /// How quickly the sessions should be listed right now.
    pub fn cadence(&self) -> Cadence {
        let lost = match &self.bound {
            Some(bound) => !bound.pid.is_some_and(|pid| self.host.is_alive(pid)),
            None => true,
        };
        Cadence {
            urgent: self.awaiting.is_some(),
            lost,
        }
    }

    /// Everything a client learns from `hello`.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            sessions: self.sessions_event(),
            presence: self.presence_event(),
            crew: self.crew_event(),
            ask: self.ask_event(),
            unread: self.unread_event(),
            account: self.account.clone(),
            settings: self.settings(),
        }
    }

    // Inputs.

    /// The sessions were listed, or the listing failed. A failed listing keeps what was known.
    pub fn on_sessions(&mut self, rows: Result<Vec<AgentRow>, String>) {
        let rows = match rows {
            Ok(rows) => rows,
            Err(error) => {
                self.log("warn", &format!("listing the sessions failed: {error}"));
                return;
            }
        };
        let host = Arc::clone(&self.host);
        let sessions = live_sessions(&rows, &self.config.assistant_name, |pid| host.is_alive(pid));
        self.listed = true;
        let now = self.host.now();
        let bound = sessions
            .iter()
            .filter(|s| s.name == self.config.assistant_name && !s.asleep())
            .max_by(|a, b| a.started_at.total_cmp(&b.started_at))
            .cloned();
        let rebound = bound.as_ref().map(|b| b.session_id.clone()) != self.bound.as_ref().map(|b| b.session_id.clone());
        self.bound = bound;
        self.bound_at = now;
        if rebound {
            self.rebind();
        }
        let bound_id = self.bound.as_ref().map(|b| b.session_id.clone());
        let others: Vec<LiveSession> = sessions
            .iter()
            .filter(|s| Some(&s.session_id) != bound_id.as_ref())
            .cloned()
            .collect();
        let mut kept: Vec<Member> = Vec::with_capacity(others.len());
        let mut old: Vec<Member> = std::mem::take(&mut self.crew);
        for session in others {
            let path = self.host.locate_transcript(&session.session_id, &session.cwd);
            let member = match old.iter().position(|m| m.session.session_id == session.session_id) {
                Some(index) => {
                    let mut member = old.swap_remove(index);
                    member.session = session;
                    if member.path != path {
                        member.path = path;
                        member.follower = TranscriptFollower::new();
                        member.modified = None;
                    }
                    member
                }
                None => Member::new(session, path),
            };
            kept.push(member);
        }
        self.crew = kept;
        let ids: HashSet<String> = sessions.iter().map(|s| s.session_id.clone()).collect();
        self.board.keep(&ids);
        self.pilot.keep(&ids);
        let names: Vec<String> = ids.iter().cloned().collect();
        self.attention.keep_only(&names);
        self.crew_seen.retain(|id, _| ids.contains(id));
        self.histories.retain(|id, _| ids.contains(id));
        self.sessions = sessions;
        self.read_crew();
        let retries = std::mem::take(&mut self.retry_calls);
        for (client, request, call) in retries {
            self.tool_call(client, request, call, true);
        }
        self.settle();
    }

    /// The hook's files for `session_id` were read: its state file and its reply file, each `None`
    /// when gone. Only the bound session's files move the assistant, but every session's are kept, so
    /// a rebind finds the new session's files without waiting for them to change.
    pub fn on_state_file(&mut self, session_id: &str, report: Option<SessionReport>, reply: Option<(String, f64)>) {
        self.files.insert(session_id.to_owned(), Files { report, reply });
        if self.bound.as_ref().is_some_and(|b| b.session_id == session_id) {
            self.load_bound_files();
        }
        self.settle();
    }

    /// `claude auth status` was asked.
    pub fn on_account(&mut self, status: Result<budd_core::account::AccountStatus, String>) {
        let event = match status {
            Ok(status) => AccountEvent {
                status: Some(status),
                error: None,
            },
            Err(error) => AccountEvent {
                status: None,
                error: Some(error),
            },
        };
        if self.account.as_ref() != Some(&event) {
            self.account = Some(event.clone());
            self.outgoing.push(Outgoing::Event(Event::Account(event)));
        }
    }

    /// Time passed: the crew's transcripts are looked at, replies caught, autopilot flown, moods
    /// settled. Called about once a second.
    pub fn tick(&mut self) {
        self.read_crew();
        self.settle();
    }

    /// The keep-alive's run, every few minutes: keeps one fresh assistant session, as `bin/keepalive.sh` did.
    pub fn keepalive_tick(&mut self) {
        if self.keepalive_busy || !self.listed {
            return;
        }
        let now = self.host.now();
        let newest = self.bound.as_ref().map(|bound| {
            let transcript = self
                .report
                .as_ref()
                .and_then(|r| r.transcript.clone())
                .map(PathBuf::from)
                .unwrap_or_else(|| self.host.locate_transcript(&bound.session_id, &bound.cwd));
            let idle_minutes = self
                .host
                .file_modified(&transcript)
                .map(|at| (now - at).max(0.0) / 60.0);
            keepalive::Session {
                alive: bound.pid.is_some_and(|pid| self.host.is_alive(pid)),
                id: bound.id.clone(),
                status: bound.status.map(status_name).map(str::to_owned),
                idle_minutes,
                tokens: self.report.as_ref().and_then(|r| r.context).unwrap_or(0),
            }
        });
        let send_in_flight = self.pending.values().any(
            |pending| matches!(pending, Pending::Message { awaiting, .. } if awaiting.target == Target::Assistant),
        );
        let standing_ask = self.awaiting.as_ref().is_some_and(|a| a.target == Target::Assistant);
        let input = keepalive::Input {
            newest,
            rotate_at: self.config.rotate_at,
            idle_min: keepalive::Input::IDLE_MIN,
            quiet_min: keepalive::Input::QUIET_MIN,
            transcript_grew: false,
            send_in_flight,
            standing_ask,
        };
        match keepalive::decide(&input) {
            Decision::Nothing => {}
            Decision::Leave(reason) => self.log("info", &format!("keep-alive: {reason}")),
            Decision::StartFresh(reason) => {
                self.log("info", &format!("keep-alive: {reason}; starting a fresh assistant"));
                self.start_assistant(reason);
            }
            Decision::Refresh(reason) => {
                if let Some(short_id) = self.bound.as_ref().map(|bound| bound.id.clone()) {
                    self.log("info", &format!("keep-alive: {reason}; refreshing"));
                    self.keepalive_busy = true;
                    self.start_effect(Pending::KeepaliveStop { reason }, |id| Effect::Stop { id, short_id });
                }
            }
        }
    }

    /// An effect ended.
    pub fn complete(&mut self, completion: Completion) {
        let now = self.host.now();
        match completion {
            Completion::Sent { id, outcome } => match self.pending.remove(&id) {
                Some(Pending::Message {
                    client,
                    request,
                    awaiting,
                }) => {
                    self.sending = false;
                    match outcome {
                        Outcome::Delivered => {
                            self.awaiting = Some(awaiting);
                            self.transcript_checked_at = None;
                            self.reply(
                                client,
                                Reply::ok(
                                    request,
                                    &SendReply {
                                        delivered: true,
                                        reason: None,
                                    },
                                ),
                            );
                        }
                        Outcome::Unconfirmed(reason) => self.reply(
                            client,
                            Reply::ok(
                                request,
                                &SendReply {
                                    delivered: false,
                                    reason: Some(reason),
                                },
                            ),
                        ),
                    }
                }
                Some(Pending::Autopilot {
                    session,
                    name,
                    reply,
                    answer,
                }) => {
                    self.piloting.remove(&session);
                    let failed = outcome != Outcome::Delivered;
                    self.pilot.record(&session, reply.as_deref(), &answer, now, failed);
                    self.log("info", &format!("autopilot {name}: {answer} - {outcome:?}"));
                }
                _ => {}
            },
            Completion::Cleared { id, outcome } => {
                if let Some(Pending::Clear {
                    client,
                    request,
                    target,
                }) = self.pending.remove(&id)
                {
                    self.sending = false;
                    match outcome {
                        Outcome::Delivered => {
                            if self.awaiting.as_ref().is_some_and(|a| a.target == target) {
                                self.awaiting = None;
                            }
                            self.effects.push(Effect::ListNow);
                            self.reply(
                                client,
                                Reply::ok(
                                    request,
                                    &SendReply {
                                        delivered: true,
                                        reason: None,
                                    },
                                ),
                            );
                        }
                        Outcome::Unconfirmed(reason) => self.reply(
                            client,
                            Reply::ok(
                                request,
                                &SendReply {
                                    delivered: false,
                                    reason: Some(reason),
                                },
                            ),
                        ),
                    }
                }
            }
            Completion::Stopped { id, result } => match self.pending.remove(&id) {
                Some(Pending::Archive {
                    client,
                    request,
                    session,
                    name,
                }) => {
                    match result {
                        Ok(()) => {
                            self.log("info", &format!("archived {name}"));
                            self.reply(
                                client,
                                Reply::ok(
                                    request,
                                    &ArchiveReply {
                                        stopped: true,
                                        asks_first: false,
                                    },
                                ),
                            );
                        }
                        Err(failure) => {
                            self.archived.restore(&session);
                            self.reply(
                                client,
                                Reply::error(request, format!("Couldn't archive {name}: {failure}")),
                            );
                        }
                    }
                    self.effects.push(Effect::ListNow);
                }
                Some(Pending::KeepaliveStop { reason }) => match result {
                    Ok(()) => self.start_assistant(reason),
                    Err(failure) => {
                        self.keepalive_busy = false;
                        self.log("warn", &format!("keep-alive could not stop the assistant: {failure}"));
                    }
                },
                _ => {}
            },
            Completion::Started { id, result } => {
                if let Some(Pending::KeepaliveStart { reason }) = self.pending.remove(&id) {
                    self.keepalive_busy = false;
                    match result {
                        Ok(_) => self.log("info", &format!("keep-alive started a fresh assistant ({reason})")),
                        Err(failure) => {
                            self.log("warn", &format!("keep-alive could not start the assistant: {failure}"))
                        }
                    }
                    self.effects.push(Effect::ListNow);
                }
            }
        }
        self.settle();
    }

    /// A client's request. Replies go to the outgoing lines, now or when the work it starts ends.
    pub fn command(&mut self, client: ClientId, request: Request) {
        let id = request.id.unwrap_or(0);
        match request.command {
            Command::Hello {
                client: name, proto, ..
            } => {
                if proto != PROTO {
                    self.reply(
                        client,
                        Reply::error(id, format!("this daemon speaks proto {PROTO}, not {proto}")),
                    );
                } else {
                    self.log("info", &format!("client {name} connected"));
                    let hello = HelloReply {
                        proto: PROTO,
                        daemon_version: self.config.version.clone(),
                        snapshot: self.snapshot(),
                    };
                    self.reply(client, Reply::ok(id, &hello));
                }
            }
            Command::Send {
                target,
                message,
                pictures,
                files,
            } => self.deliver(client, id, target, message, pictures, files),
            Command::Answer { target, answer } => self.deliver(client, id, target, answer, Vec::new(), Vec::new()),
            Command::Archive { session, force } => self.archive(client, id, &session, force),
            Command::StartTickets { .. } | Command::CrewStart { .. } | Command::Chat { .. } => {
                // TODO: start crew and chats through `claude --bg` once the messenger lands beside this crate.
                self.reply(client, Reply::error(id, "not yet"));
            }
            Command::Clear { target } => self.clear(client, id, target),
            Command::Mode { pilot, office_open } => {
                if let Some(pilot) = pilot {
                    self.pilot_mode = pilot;
                }
                if let Some(open) = office_open {
                    self.office_open = open;
                }
                self.reply(client, Reply::done(id));
            }
            Command::AttentionSeen { session } => {
                if let Some(member) = self.crew.iter().find(|m| m.session.session_id == session) {
                    let reply = self.member_reply(member);
                    if let Some(at) = member.modified {
                        self.crew_seen.insert(session.clone(), at);
                    }
                    self.attention.see(&session, reply.as_deref());
                    self.reply(client, Reply::done(id));
                } else {
                    self.reply(client, Reply::error(id, "no such crew session"));
                }
            }
            Command::UnreadRead => {
                self.unread.read();
                self.reply(client, Reply::done(id));
            }
            Command::HistoryPage { target, earlier } => self.history_page(client, id, target, earlier),
            Command::HookPoke { .. } => self.effects.push(Effect::ReadStateNow),
            Command::ToolCall { call } => self.tool_call(client, id, call, false),
            Command::SettingsGet => {
                let settings = self.settings();
                self.reply(client, Reply::ok(id, &settings));
            }
            Command::Shutdown => {
                self.reply(client, Reply::done(id));
                self.effects.push(Effect::Shutdown);
            }
        }
        self.settle();
    }

    /// Carries out every queued effect against the host, on this thread, and feeds the results back;
    /// for tests and for callers that have no runtime. `ListNow`, `ReadStateNow` and `Shutdown` are
    /// returned, as they are the caller's.
    pub fn drive(&mut self) -> Vec<Effect> {
        let mut left = Vec::new();
        let mut done = false;
        while !done {
            done = true;
            for effect in self.take_effects() {
                done = false;
                let host = Arc::clone(&self.host);
                match effect {
                    Effect::Send {
                        id,
                        target,
                        message,
                        pictures,
                    } => {
                        let outcome = host.send(&target, &message, &pictures);
                        self.complete(Completion::Sent { id, outcome });
                    }
                    Effect::Clear { id, target } => {
                        let outcome = host.clear(&target);
                        self.complete(Completion::Cleared { id, outcome });
                    }
                    Effect::Stop { id, short_id } => {
                        let result = host.stop(&short_id);
                        self.complete(Completion::Stopped { id, result });
                    }
                    Effect::StartBg { id, cwd, argv } => {
                        let result = host.start_bg(&cwd, &argv);
                        self.complete(Completion::Started { id, result });
                    }
                    other => left.push(other),
                }
            }
        }
        left
    }

    // Commands.

    fn deliver(
        &mut self,
        client: ClientId,
        request: u64,
        target: Target,
        text: String,
        pictures: Vec<String>,
        files: Vec<String>,
    ) {
        if self.sending {
            self.reply(client, Reply::error(request, "Still sending the last message."));
            return;
        }
        let send_target = match self.send_target(&target) {
            Ok(send_target) => send_target,
            Err(error) => {
                self.reply(client, Reply::error(request, error));
                return;
            }
        };
        if let Some(session) = target.crew()
            && let Some(member) = self.crew.iter().find(|m| m.session.session_id == session)
            && self.member_blocked(member)
        {
            let reply = self.member_reply(member);
            self.attention.see(session, reply.as_deref());
        }
        let style = if cfg!(windows) {
            PathStyle::WindowsQuoted
        } else {
            PathStyle::Posix
        };
        let message = with_files(&text, &files, style);
        let names: Vec<String> = files
            .iter()
            .map(|path| budd_core::delivery::last_path_component(path).to_owned())
            .collect();
        let note = attachment_note(pictures.len(), &names);
        let label = [sendable(&text), note]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let awaiting = Awaiting {
            since: self.host.now(),
            message: sendable(&message),
            label,
            bound_id: match target {
                Target::Assistant => self.bound.as_ref().map(|b| b.session_id.clone()),
                Target::Crew(_) => None,
            },
            target,
            prompt_id: None,
        };
        self.sending = true;
        self.start_effect(
            Pending::Message {
                client,
                request,
                awaiting,
            },
            |id| Effect::Send {
                id,
                target: send_target,
                message,
                pictures,
            },
        );
    }

    fn clear(&mut self, client: ClientId, request: u64, target: Target) {
        if self.sending {
            self.reply(client, Reply::error(request, "Still sending the last message."));
            return;
        }
        let send_target = match self.send_target(&target) {
            Ok(send_target) => send_target,
            Err(_) => {
                self.reply(
                    client,
                    Reply::error(request, "That session has stopped, so there is nothing to clear."),
                );
                return;
            }
        };
        self.sending = true;
        self.start_effect(
            Pending::Clear {
                client,
                request,
                target,
            },
            |id| Effect::Clear {
                id,
                target: send_target,
            },
        );
    }

    fn archive(&mut self, client: ClientId, request: u64, session: &str, force: bool) {
        let Some(member) = self.crew.iter().find(|m| m.session.session_id == session) else {
            self.reply(client, Reply::error(request, "no such crew session"));
            return;
        };
        let status = member.session.status.map(status_name);
        if ArchivedCrew::asks_first(status) && !force {
            self.reply(
                client,
                Reply::ok(
                    request,
                    &ArchiveReply {
                        stopped: false,
                        asks_first: true,
                    },
                ),
            );
            return;
        }
        let short_id = member.session.id.clone();
        let name = member.session.name.clone();
        let now = self.host.now();
        self.archived.archive(session, now);
        if self.awaiting.as_ref().is_some_and(|a| a.target.crew() == Some(session)) {
            self.awaiting = None;
        }
        self.start_effect(
            Pending::Archive {
                client,
                request,
                session: session.to_owned(),
                name,
            },
            |id| Effect::Stop { id, short_id },
        );
    }

    fn history_page(&mut self, client: ClientId, request: u64, target: Target, earlier: bool) {
        let (key, path, name) = match &target {
            Target::Assistant => match &self.bound {
                Some(bound) => {
                    let path = self
                        .report
                        .as_ref()
                        .and_then(|r| r.transcript.clone())
                        .map(PathBuf::from)
                        .unwrap_or_else(|| self.host.locate_transcript(&bound.session_id, &bound.cwd));
                    (bound.session_id.clone(), path, bound.name.clone())
                }
                None => {
                    self.reply(client, Reply::error(request, "The assistant is not running."));
                    return;
                }
            },
            Target::Crew(session) => match self.crew.iter().find(|m| m.session.session_id == *session) {
                Some(member) => (session.clone(), member.path.clone(), member.session.name.clone()),
                None => {
                    self.reply(client, Reply::error(request, "no such crew session"));
                    return;
                }
            },
        };
        let host = Arc::clone(&self.host);
        let current = path.to_string_lossy().into_owned();
        let history = self
            .histories
            .entry(key)
            .or_insert_with(|| ConversationHistory::new(&current, &name));
        if history.current != current {
            *history = ConversationHistory::new(&current, &name);
        }
        history.update(host.transcripts());
        if earlier {
            history.load_earlier(host.transcripts());
        }
        let reply = HistoryReply {
            turns: history.turns(),
            has_earlier: history.has_earlier(),
        };
        self.reply(client, Reply::ok(request, &reply));
    }

    /// A session called one of Budd's tools. Only a listed session may: the caller is known by the
    /// processes above its MCP server. One not listed yet has the sessions listed again before it is
    /// refused, which is what `retried` says has happened.
    fn tool_call(&mut self, client: ClientId, request: u64, call: Call, retried: bool) {
        let made = match call.request() {
            Ok(made) => made,
            Err(refused) => {
                self.reply(client, Reply::ok(request, &ToolResult::error(refused.message())));
                return;
            }
        };
        let session = call
            .callers
            .iter()
            .find_map(|pid| self.sessions.iter().find(|s| s.pid == Some(*pid)))
            .cloned();
        let Some(session) = session else {
            if retried {
                self.reply(client, Reply::ok(request, &ToolResult::error(UNKNOWN_CALLER)));
            } else {
                self.retry_calls.push((client, request, call));
                self.effects.push(Effect::ListNow);
            }
            return;
        };
        let is_assistant = self.bound.as_ref().is_some_and(|b| b.session_id == session.session_id);
        let target = if is_assistant {
            Target::Assistant
        } else {
            Target::Crew(session.session_id.clone())
        };
        let result = match made {
            BuddRequest::Say(text) => {
                self.outgoing.push(Outgoing::Event(Event::Say(SayEvent {
                    text,
                    target,
                    name: session.name.clone(),
                })));
                ToolResult::ok("Said.")
            }
            BuddRequest::Status(_) if is_assistant => ToolResult::ok(ASSISTANT_STATUS),
            BuddRequest::Ask(ask) if is_assistant => {
                let now = self.host.now();
                let turn = self.report.as_ref().and_then(|r| r.turn.clone());
                self.outgoing.push(Outgoing::Event(Event::Reply(ReplyEvent {
                    target: Target::Assistant,
                    prompt: turn.clone().unwrap_or_default(),
                    reply: Some(ask.as_reply_after(self.last_reply.as_deref())),
                    lost: None,
                    at: now,
                })));
                self.assistant_ask = Some((ask, turn, now));
                ToolResult::ok(ASSISTANT_ASK_TAKEN)
            }
            BuddRequest::Ask(ask) => {
                let turn = self.current_turn(&session.session_id);
                self.board.ask(ask, &session.session_id, turn.as_deref());
                self.effects.push(Effect::ListNow);
                ToolResult::ok(ASK_TAKEN)
            }
            BuddRequest::Status(line) => {
                let turn = self.current_turn(&session.session_id);
                self.board.status(&line, &session.session_id, turn.as_deref());
                self.effects.push(Effect::ListNow);
                ToolResult::ok("Shown at your desk in Budd's office.")
            }
        };
        self.reply(client, Reply::ok(request, &result));
    }

    // Reading.

    /// The bound session changed: its files are read afresh, and a message awaiting the old one is lost.
    fn rebind(&mut self) {
        self.load_bound_files();
        self.assistant_ask = None;
        self.transcript_checked_at = None;
    }

    fn load_bound_files(&mut self) {
        let files = self
            .bound
            .as_ref()
            .and_then(|b| self.files.get(&b.session_id).cloned())
            .unwrap_or_default();
        self.report = files.report;
        match files.reply {
            Some((text, at)) if !text.is_empty() => {
                self.last_reply = Some(text);
                self.last_reply_at = Some(at);
            }
            _ => {
                self.last_reply = None;
                self.last_reply_at = None;
            }
        }
    }

    /// Re-reads each crew member's transcript, job file and evidence that changed since last time.
    fn read_crew(&mut self) {
        let host = Arc::clone(&self.host);
        let config_dir = self.config.config_dir.clone();
        let _ = config_dir;
        for member in &mut self.crew {
            member.job = host.read_job(&member.session.id);
            let intent = member.job.as_ref().and_then(|job| job.state.intent.clone());
            member.evidence = Evidence::ticket(intent.as_deref()).and_then(|ticket| host.read_evidence(&ticket));
            let modified = host.file_modified(&member.path);
            if modified.is_some() && modified != member.modified {
                if let Some(source) = host.transcripts().open(&member.path.to_string_lossy()) {
                    member.follower.update(&*source);
                }
                member.modified = modified;
            }
        }
    }

    /// The turn a crew session is on, read now rather than at the last tick, so an ask made a moment
    /// into a turn is tied to that turn and not the one before.
    fn current_turn(&mut self, session_id: &str) -> Option<String> {
        let host = Arc::clone(&self.host);
        let member = self.crew.iter_mut().find(|m| m.session.session_id == session_id)?;
        if let Some(source) = host.transcripts().open(&member.path.to_string_lossy()) {
            member.follower.update(&*source);
            member.modified = host.file_modified(&member.path);
        }
        member.summary().turn().map(str::to_owned)
    }

    // Settling: what follows from the state after any input.

    fn settle(&mut self) {
        let now = self.host.now();
        self.resolve_presence(now);
        if let Some(written) = self.last_reply_at {
            self.unread.see(written, now);
        }
        if let Some((_, turn, at)) = &self.assistant_ask
            && (now - at > ASK_LINGERS || self.report.as_ref().and_then(|r| r.turn.as_ref()) != turn.as_ref())
        {
            self.assistant_ask = None;
        }
        self.catch_reply(now);
        self.fly_autopilot(now);
        self.publish(now);
        self.persist();
    }

    fn resolve_presence(&mut self, now: f64) {
        let host = Arc::clone(&self.host);
        self.presence = match (&self.bound, &self.report) {
            (None, _) => Presence::with_detail(Mood::Sleeping, "No assistant session running"),
            // Bound to a live session that has not had a hook event since the files moved: quiet, not gone.
            (Some(_), None) => Presence::new(Mood::Idle),
            (Some(bound), Some(report)) => {
                let fresh = self.bound_at > report.at + 1.0;
                let session_idle = fresh.then(|| bound.status == Some(Status::Idle));
                PresenceResolver::resolve(Some(report), now, session_idle, |pid| host.is_alive(pid))
            }
        };
    }

    /// After a send, the session's next finished turn is the answer.
    fn catch_reply(&mut self, now: f64) {
        let Some(awaiting) = self.awaiting.clone() else {
            return;
        };
        if now - awaiting.since > GIVE_UP {
            self.lose(&awaiting, "No answer in 15 minutes", now);
            return;
        }
        if let Target::Crew(session) = &awaiting.target {
            self.catch_crew_reply(&awaiting, session, now);
            return;
        }
        // The hook names the prompt's id once the turn starts; from then on the id decides.
        let mut awaiting = awaiting;
        if awaiting.prompt_id.is_none()
            && let Some(report) = &self.report
            && report.at >= awaiting.since
            && let Some(prompt_id) = &report.prompt_id
            && report
                .turn
                .as_deref()
                .is_some_and(|turn| SessionReport::turn_is_for(turn, &awaiting.message))
        {
            awaiting.prompt_id = Some(prompt_id.clone());
            self.awaiting = Some(awaiting.clone());
        }
        let mut reply = self.report.as_ref().and_then(|report| {
            (report.at >= awaiting.since && report.finishes_turn(&awaiting.message, awaiting.prompt_id.as_deref()))
                .then(|| self.last_reply.clone())
                .flatten()
        });
        if reply.is_none()
            && let Some(report) = &self.report
            && report.at >= awaiting.since
            && (report.state == "done" || report.state == "failed")
            && self.transcript_checked_at != Some(report.at)
            && let Some(transcript) = &report.transcript
        {
            // Something started another turn straight after the answer (a background task finishing):
            // the transcript still has it. Read once for each report, not every tick.
            self.transcript_checked_at = Some(report.at);
            reply = self
                .host
                .transcripts()
                .open(transcript)
                .and_then(|source| answer_in(&*source, &awaiting.message));
        }
        let Some(reply) = reply else {
            let bound_id = self.bound.as_ref().map(|b| b.session_id.clone());
            if awaiting.bound_id.is_some() && bound_id != awaiting.bound_id {
                // Replaced before it answered (a keep-alive refresh, a restart): the turn died with it.
                self.lose(&awaiting, "Refreshed before it answered", now);
                return;
            }
            // Ended without a hook (Esc, or a failure older Claude versions do not report): stop waiting.
            if self.presence.mood == Mood::Idle && now - awaiting.since > IDLE_MEANS_DONE {
                self.lose(&awaiting, "The turn ended without a reply", now);
            }
            return;
        };
        self.awaiting = None;
        self.outgoing.push(Outgoing::Event(Event::Reply(ReplyEvent {
            target: Target::Assistant,
            prompt: awaiting.label,
            reply: Some(reply),
            lost: None,
            at: self.last_reply_at.unwrap_or(now),
        })));
    }

    /// A crew session has answered when `claude agents` calls it idle again, its transcript's last
    /// prompt is the message sent, it has replied, nothing is still running, and the transcript has
    /// been quiet for two seconds (the agents list is a few seconds behind the transcript).
    fn catch_crew_reply(&mut self, awaiting: &Awaiting, session: &str, now: f64) {
        let Some(member) = self.crew.iter().find(|m| m.session.session_id == session) else {
            if self.listed {
                self.lose(awaiting, "The session stopped before it answered", now);
            }
            return;
        };
        let summary = member.summary();
        let answered = member.session.status == Some(Status::Idle)
            && summary.pending.is_none()
            && summary.reply.is_some()
            && summary
                .prompt
                .as_deref()
                .is_some_and(|prompt| SessionReport::turn_is_for(prompt, &awaiting.message))
            && member.modified.is_some_and(|updated| now - updated > CREW_QUIET);
        if !answered {
            return;
        }
        let reply = summary.reply.clone().unwrap_or_default();
        let updated = member.modified.unwrap_or(now);
        self.crew_seen.insert(session.to_owned(), updated);
        self.awaiting = None;
        self.outgoing.push(Outgoing::Event(Event::Reply(ReplyEvent {
            target: Target::Crew(session.to_owned()),
            prompt: awaiting.label.clone(),
            reply: Some(reply),
            lost: None,
            at: updated,
        })));
    }

    fn lose(&mut self, awaiting: &Awaiting, why: &str, now: f64) {
        self.awaiting = None;
        self.outgoing.push(Outgoing::Event(Event::Reply(ReplyEvent {
            target: awaiting.target.clone(),
            prompt: awaiting.label.clone(),
            reply: None,
            lost: Some(why.to_owned()),
            at: now,
        })));
    }

    /// Autopilot: answers each crew session blocked on a `Decide:` line with the answer it recommends,
    /// once the ask has stood for the grace, quietly in the background.
    fn fly_autopilot(&mut self, now: f64) {
        if self.pilot_mode != PilotMode::Autopilot || self.sending {
            return;
        }
        let candidates: Vec<(String, String, Option<String>, SendTarget)> = self
            .crew
            .iter()
            .filter(|m| self.member_blocked(m) && !self.piloting.contains(&m.session.session_id))
            .map(|m| {
                (
                    m.session.session_id.clone(),
                    m.session.name.clone(),
                    self.member_reply(m),
                    self.crew_target(m),
                )
            })
            .collect();
        for (session, name, reply, target) in candidates {
            let Some(answer) = self.pilot.due(&session, reply.as_deref(), now) else {
                continue;
            };
            self.piloting.insert(session.clone());
            let message = format!("{}{answer}", Autopilot::PREFIX);
            self.start_effect(
                Pending::Autopilot {
                    session,
                    name,
                    reply,
                    answer,
                },
                |id| Effect::Send {
                    id,
                    target,
                    message,
                    pictures: Vec::new(),
                },
            );
        }
    }

    fn start_assistant(&mut self, reason: String) {
        self.keepalive_busy = true;
        let argv = assistant_flags(
            &self.config.assistant_name,
            &self.config.assistant_model,
            &self.config.home.to_string_lossy(),
            false,
        );
        let cwd = self.config.assistant_dir();
        self.start_effect(Pending::KeepaliveStart { reason }, |id| Effect::StartBg {
            id,
            cwd,
            argv,
        });
    }

    fn start_effect(&mut self, pending: Pending, effect: impl FnOnce(u64) -> Effect) {
        let id = self.next_effect;
        self.next_effect += 1;
        self.pending.insert(id, pending);
        self.effects.push(effect(id));
    }

    fn reply(&mut self, client: ClientId, reply: Reply) {
        self.outgoing.push(Outgoing::Reply { client, reply });
    }

    fn log(&mut self, level: &str, message: &str) {
        match level {
            "warn" => tracing::warn!("{message}"),
            "error" => tracing::error!("{message}"),
            _ => tracing::info!("{message}"),
        }
        self.outgoing.push(Outgoing::Event(Event::Log(LogEvent {
            level: level.to_owned(),
            message: message.to_owned(),
        })));
    }

    // Telling.

    fn publish(&mut self, now: f64) {
        let sessions = self.sessions_event();
        if self.told.sessions.as_ref() != Some(&sessions) {
            self.told.sessions = Some(sessions.clone());
            self.outgoing.push(Outgoing::Event(Event::Sessions(sessions)));
        }
        let presence = self.presence_event();
        if self.told.presence.as_ref() != Some(&presence) {
            self.told.presence = Some(presence.clone());
            self.outgoing.push(Outgoing::Event(Event::Presence(presence)));
        }
        let crew = self.crew_event();
        if self.told.crew.as_ref() != Some(&crew) {
            for member in &crew.members {
                let id = &member.session.session_id;
                if self.told.evidence.get(id) != Some(&member.evidence) {
                    self.told.evidence.insert(id.clone(), member.evidence.clone());
                    self.outgoing.push(Outgoing::Event(Event::Evidence(EvidenceEvent {
                        session: id.clone(),
                        evidence: member.evidence.clone(),
                    })));
                }
            }
            let present: HashSet<&String> = crew.members.iter().map(|m| &m.session.session_id).collect();
            self.told.evidence.retain(|id, _| present.contains(id));
            self.told.crew = Some(crew.clone());
            self.outgoing.push(Outgoing::Event(Event::Crew(crew)));
        }
        let ask = self.ask_event();
        if self.told.ask.as_ref() != Some(&ask) {
            self.told.ask = Some(ask.clone());
            self.outgoing.push(Outgoing::Event(Event::Ask(ask)));
        }
        let unread = self.unread_event();
        if self.told.unread.as_ref() != Some(&unread) {
            self.told.unread = Some(unread.clone());
            self.outgoing.push(Outgoing::Event(Event::Unread(unread)));
        }
        let _ = now;
    }

    fn persist(&mut self) {
        let persisted = Persisted {
            autopilot_answers: self.pilot.plist(),
            crew_attention: self.attention.seen().clone(),
            unread_count: self.unread.count(),
            unread_newest: self.unread.newest(),
            pilot_mode: self.pilot_mode,
            office_open: self.office_open,
        };
        if self.told.persisted.as_ref() == Some(&persisted) {
            return;
        }
        if let Err(error) = persisted.save(&self.config.state_path()) {
            tracing::warn!(%error, "the state file could not be written");
        }
        self.told.persisted = Some(persisted);
    }

    fn settings(&self) -> Settings {
        Settings {
            home: self.config.home.to_string_lossy().into_owned(),
            config_dir: self.config.config_dir.to_string_lossy().into_owned(),
            assistant_name: self.config.assistant_name.clone(),
            pilot_mode: self.pilot_mode,
            office_open: self.office_open,
            crew_context_limit: self.config.crew_context_limit,
            rotate_at: self.config.rotate_at,
            claude: self.config.claude.as_ref().map(|p| p.to_string_lossy().into_owned()),
            version: self.config.version.clone(),
        }
    }

    fn sessions_event(&self) -> SessionsEvent {
        SessionsEvent {
            bound: self.bound.clone(),
            sessions: self.sessions.clone(),
            listed: self.listed,
        }
    }

    fn presence_event(&self) -> PresenceEvent {
        PresenceEvent {
            presence: self.presence.clone(),
            report: self.report.clone(),
            fullness: if self.presence.mood == Mood::Sleeping {
                None
            } else {
                self.report.as_ref().and_then(SessionReport::fullness)
            },
            last_reply: self.last_reply.clone(),
            last_reply_at: self.last_reply_at,
            pilot_mode: self.pilot_mode,
            office_open: self.office_open,
            awaiting: self.awaiting.as_ref().map(|a| AwaitingWire {
                target: a.target.clone(),
                label: a.label.clone(),
                since: a.since,
            }),
        }
    }

    fn crew_event(&self) -> CrewEvent {
        let now = self.host.now();
        CrewEvent {
            members: self
                .crew
                .iter()
                .filter(|m| !self.archived.hides(&m.session.session_id, now))
                .map(|m| self.view(m, now))
                .collect(),
        }
    }

    fn ask_event(&self) -> AskEvent {
        let now = self.host.now();
        let mut asks: Vec<CrewAsk> = Vec::new();
        if let (Some((ask, _, _)), Some(bound)) = (&self.assistant_ask, &self.bound) {
            asks.push(CrewAsk {
                target: Target::Assistant,
                short_id: bound.id.clone(),
                name: bound.name.clone(),
                what: Some(ask.question.clone()),
                picker: false,
                answers: ask.answers.clone(),
                asleep: None,
            });
        }
        for member in &self.crew {
            if self.archived.hides(&member.session.session_id, now) {
                continue;
            }
            let view = self.view(member, now);
            if !view.loud {
                continue;
            }
            asks.push(self.crew_ask(member, &view, now));
        }
        AskEvent { asks }
    }

    fn unread_event(&self) -> UnreadEvent {
        UnreadEvent {
            count: self.unread.count(),
            newest: self.unread.newest(),
            arrived_at: self.unread.arrived_at(),
        }
    }

    // The crew, derived.

    fn member_ask(&self, member: &Member) -> Option<&BuddAsk> {
        self.board.ask_of(&member.session.session_id, member.summary().turn())
    }

    fn member_reply(&self, member: &Member) -> Option<String> {
        match self.member_ask(member) {
            Some(ask) => Some(ask.as_reply_after(member.summary().reply.as_deref())),
            None => member.summary().reply.clone(),
        }
    }

    fn member_asks(member: &Member) -> bool {
        member.session.status == Some(Status::Waiting)
    }

    fn member_blocked(&self, member: &Member) -> bool {
        let job_state = member
            .job
            .as_ref()
            .and_then(|job| job.state.state.clone())
            .or_else(|| member.session.state.map(|s| format!("{s:?}").to_lowercase()));
        !Self::member_asks(member)
            && member.session.status != Some(Status::Busy)
            && (self.member_ask(member).is_some() || job_state.as_deref() == Some("blocked"))
    }

    fn crew_target(&self, member: &Member) -> SendTarget {
        SendTarget {
            short_id: member.session.id.clone(),
            session_id: member.session.session_id.clone(),
            transcript: Some(member.path.clone()),
            asleep: member.session.asleep(),
        }
    }

    fn send_target(&self, target: &Target) -> Result<SendTarget, String> {
        match target {
            Target::Assistant => {
                let bound = self
                    .bound
                    .as_ref()
                    .ok_or_else(|| "The assistant is not running, so this was not sent.".to_owned())?;
                let transcript = self
                    .report
                    .as_ref()
                    .and_then(|r| r.transcript.clone())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.host.locate_transcript(&bound.session_id, &bound.cwd));
                Ok(SendTarget {
                    short_id: bound.id.clone(),
                    session_id: bound.session_id.clone(),
                    transcript: Some(transcript),
                    asleep: false,
                })
            }
            Target::Crew(session) => self
                .crew
                .iter()
                .find(|m| m.session.session_id == *session)
                .map(|m| self.crew_target(m))
                .ok_or_else(|| "That session has stopped, so this was not sent.".to_owned()),
        }
    }

    /// One crew member as the office shows it, as `CrewMember` and `crewRows` in the Swift app derived it.
    fn view(&self, member: &Member, now: f64) -> CrewMember {
        let summary = member.summary();
        let id = &member.session.session_id;
        let ask = self.member_ask(member).cloned();
        let status = self.board.status_of(id, summary.turn()).map(str::to_owned);
        let asleep = member.session.asleep();
        let asks = Self::member_asks(member);
        let blocked = self.member_blocked(member);
        let reply = self.member_reply(member);
        let detail = status
            .clone()
            .or_else(|| member.job.as_ref().and_then(|job| job.state.detail.clone()));
        let busy = member.session.status == Some(Status::Busy);
        let fullness = summary
            .context
            .map(|context| context as f64 / self.config.crew_context_limit.max(1) as f64);
        let is_new = summary.reply.is_some()
            && member
                .modified
                .is_some_and(|updated| updated > self.crew_seen.get(id).copied().unwrap_or(f64::NEG_INFINITY));
        let given = self.pilot.given(id, reply.as_deref());
        let handles = self.pilot_mode == PilotMode::Autopilot && blocked && self.pilot.handles(id, reply.as_deref());
        let mut autopilot = None;
        let (desk, line) = if asks {
            (
                DeskState::NeedsYou,
                summary
                    .pending
                    .as_ref()
                    .map_or("needs you".to_owned(), |pending| format!("needs you: {pending}")),
            )
        } else if blocked && let Some(given) = given {
            autopilot = Some(if given.failed {
                format!(
                    "Autopilot couldn't send \u{201C}{}\u{201D}, so this one is yours.",
                    given.answer
                )
            } else {
                format!("Autopilot answered \u{201C}{}\u{201D}.", given.answer)
            });
            if given.failed {
                (
                    DeskState::NeedsYou,
                    format!("autopilot couldn't send \u{201C}{}\u{201D}; needs you", given.answer),
                )
            } else {
                (
                    DeskState::Working,
                    format!("autopilot answered \u{201C}{}\u{201D}", given.answer),
                )
            }
        } else if handles && let Some(answer) = Autopilot::answer(reply.as_deref()) {
            autopilot = Some(format!("Autopilot answers \u{201C}{answer}\u{201D} in a moment."));
            (
                DeskState::Working,
                format!("autopilot answers \u{201C}{answer}\u{201D} in a moment"),
            )
        } else if blocked {
            (
                DeskState::NeedsYou,
                ask.as_ref()
                    .map(|ask| ask.question.clone())
                    .or_else(|| detail.clone())
                    .unwrap_or_else(|| "needs you".to_owned()),
            )
        } else if busy {
            (
                DeskState::Working,
                summary
                    .pending
                    .clone()
                    .or_else(|| detail.clone())
                    .unwrap_or_else(|| "working".to_owned()),
            )
        } else {
            (
                DeskState::Idle,
                detail
                    .clone()
                    .unwrap_or_else(|| if is_new { "new reply" } else { "idle" }.to_owned()),
            )
        };
        let stage = if OfficeChat::is_chat(&member.session.name) {
            Some("chat".to_owned())
        } else {
            CrewBoard::stage(
                summary.command.as_deref(),
                member.job.as_ref().and_then(|job| job.state.intent.as_deref()),
            )
        };
        let quiet = if desk == DeskState::Working {
            None
        } else {
            member
                .modified
                .map(|updated| CrewBoard::quiet_or_asleep(now - updated, asleep))
        };
        let loud = asks || (blocked && self.attention.is_loud(id, reply.as_deref()) && !handles);
        CrewMember {
            session: member.session.clone(),
            summary: Summary::from(summary),
            job: member.job.as_ref().map(|job| job.state.clone()),
            evidence: member.evidence.clone(),
            ask,
            status,
            reply,
            detail,
            asleep,
            asks,
            blocked,
            needs_you: asks || blocked,
            stage,
            quiet,
            fullness,
            autopilot,
            desk,
            line,
            is_new,
            loud,
            links: ReplyLink::collect(&summary.texts),
            updated: member.modified,
        }
    }

    /// A crew session waiting on the human, as its ask says it.
    fn crew_ask(&self, member: &Member, view: &CrewMember, now: f64) -> CrewAsk {
        let target = Target::Crew(member.session.session_id.clone());
        if view.asks {
            let what = view.summary.pending.as_ref().map(|pending| {
                let mut chars = pending.chars();
                let first: String = chars.next().map(|c| c.to_lowercase().collect()).unwrap_or_default();
                format!("wants to {first}{}", chars.as_str())
            });
            return CrewAsk {
                target,
                short_id: member.session.id.clone(),
                name: member.session.name.clone(),
                what,
                picker: true,
                answers: Vec::new(),
                asleep: None,
            };
        }
        CrewAsk {
            target,
            short_id: member.session.id.clone(),
            name: member.session.name.clone(),
            what: view
                .ask
                .as_ref()
                .map(|ask| ask.question.clone())
                .or_else(|| view.detail.clone()),
            picker: false,
            answers: view
                .reply
                .as_deref()
                .and_then(ReplyOptions::parse)
                .map(|options| options.answers)
                .unwrap_or_default(),
            asleep: view.asleep.then(|| {
                member.modified.map_or("asleep".to_owned(), |updated| {
                    CrewBoard::quiet_or_asleep(now - updated, true)
                })
            }),
        }
    }
}

/// The name `claude agents` uses for a status.
fn status_name(status: Status) -> &'static str {
    match status {
        Status::Idle => "idle",
        Status::Busy => "busy",
        Status::Waiting => "waiting",
    }
}

/// The answer to `message` in a transcript, read back from the end as far as it takes: a long turn
/// writes megabytes after the message it answers.
fn answer_in(source: &dyn TranscriptSource, message: &str) -> Option<String> {
    let size = source.len();
    let mut tail = FIRST_TAIL;
    loop {
        let start = tail_start(size, tail);
        let (mut lines, _) = whole_lines(&source.read(start..size));
        if start > 0 && !lines.is_empty() {
            lines.remove(0);
        }
        if let Some(answer) = TranscriptSummary::answer(message, &lines) {
            return Some(answer);
        }
        if start == 0 {
            return None;
        }
        tail = grown_tail(tail);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::mock::{Did, MockHost};
    use budd_core::crew::JobState;
    use serde_json::json;

    const ASSISTANT_SID: &str = "aaaaaaaa-0000-4000-8000-000000000001";
    const CREW_SID: &str = "cccccccc-0000-4000-8000-000000000002";
    const OTHER_SID: &str = "dddddddd-0000-4000-8000-000000000003";

    fn row(
        id: &str,
        session_id: &str,
        name: &str,
        pid: Option<u32>,
        status: &str,
        state: &str,
        started: f64,
    ) -> AgentRow {
        AgentRow {
            id: Some(id.to_owned()),
            session_id: Some(session_id.to_owned()),
            name: Some(name.to_owned()),
            cwd: Some("/w/repo".to_owned()),
            pid,
            status: Some(status.to_owned()),
            state: Some(state.to_owned()),
            kind: Some("background".to_owned()),
            started_at: Some(started),
            ..AgentRow::default()
        }
    }

    fn assistant_row() -> AgentRow {
        row("a1", ASSISTANT_SID, "assistant", Some(101), "idle", "working", 1.0)
    }

    fn crew_row(status: &str, state: &str) -> AgentRow {
        row("c2", CREW_SID, "casbo-1", Some(202), status, state, 2.0)
    }

    fn user(text: &str, uuid: &str) -> String {
        json!({"type": "user", "uuid": uuid, "message": {"content": text}}).to_string()
    }

    fn said(text: &str, id: &str) -> String {
        json!({"type": "assistant", "message": {"id": id, "content": [{"type": "text", "text": text}]}}).to_string()
    }

    struct Rig {
        host: Arc<MockHost>,
        engine: Engine,
        /// The home folder, kept until the rig goes.
        _home: tempfile::TempDir,
        /// Lines taken from the engine and not yet looked at.
        out: Vec<Outgoing>,
    }

    impl Rig {
        fn new() -> Rig {
            let host = Arc::new(MockHost::new());
            let home = tempfile::tempdir().expect("tempdir");
            let config = Config::new(home.path(), host.config_dir());
            let engine = Engine::with_state(host.clone(), config, Persisted::default());
            Rig {
                host,
                engine,
                _home: home,
                out: Vec::new(),
            }
        }

        fn gather(&mut self) {
            self.out.extend(self.engine.take_outgoing());
        }

        fn list(&mut self, rows: Vec<AgentRow>) {
            self.host.set_rows(rows.clone());
            self.engine.on_sessions(Ok(rows));
        }

        fn crew_path(&self) -> PathBuf {
            self.host.locate_transcript(CREW_SID, "/w/repo")
        }

        /// The events queued so far, leaving the replies for `replies`.
        fn events(&mut self) -> Vec<Event> {
            self.gather();
            let (events, rest): (Vec<Outgoing>, Vec<Outgoing>) = std::mem::take(&mut self.out)
                .into_iter()
                .partition(|o| matches!(o, Outgoing::Event(_)));
            self.out = rest;
            events
                .into_iter()
                .filter_map(|out| match out {
                    Outgoing::Event(event) => Some(event),
                    Outgoing::Reply { .. } => None,
                })
                .collect()
        }

        /// The replies queued so far, leaving the events for `events`.
        fn replies(&mut self) -> Vec<Reply> {
            self.gather();
            let (replies, rest): (Vec<Outgoing>, Vec<Outgoing>) = std::mem::take(&mut self.out)
                .into_iter()
                .partition(|o| matches!(o, Outgoing::Reply { .. }));
            self.out = rest;
            replies
                .into_iter()
                .filter_map(|out| match out {
                    Outgoing::Reply { reply, .. } => Some(reply),
                    Outgoing::Event(_) => None,
                })
                .collect()
        }

        fn command(&mut self, id: u64, command: Command) {
            self.engine.command(ClientId(1), Request { id: Some(id), command });
        }

        fn crew_view(&self) -> CrewMember {
            self.engine
                .crew_event()
                .members
                .into_iter()
                .find(|m| m.session.session_id == CREW_SID)
                .expect("the crew member is on the bridge")
        }
    }

    #[test]
    fn the_bound_session_is_the_newest_live_one_with_the_assistants_name() {
        let mut rig = Rig::new();
        assert_eq!(
            rig.engine.cadence(),
            Cadence {
                urgent: false,
                lost: true
            }
        );
        let old = row("a0", OTHER_SID, "assistant", Some(100), "idle", "working", 0.5);
        let mut asleep = row("a9", "eeee", "assistant", None, "idle", "blocked", 9.0);
        asleep.pid = None;
        rig.list(vec![old, assistant_row(), asleep, crew_row("busy", "working")]);
        let bound = rig.engine.bound.clone().expect("bound");
        assert_eq!(bound.session_id, ASSISTANT_SID);
        // The older assistant is crew to nobody: it is listed, but not followed as crew.
        assert_eq!(rig.engine.sessions.len(), 3);
        assert_eq!(rig.engine.crew.len(), 2);
        assert_eq!(
            rig.engine.cadence(),
            Cadence {
                urgent: false,
                lost: false
            }
        );
        let events = rig.events();
        assert!(
            matches!(&events[0], Event::Sessions(s) if s.listed && s.bound.as_ref().map(|b| b.id.as_str()) == Some("a1"))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Presence(p) if p.presence.mood == Mood::Idle))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Crew(c) if c.members.len() == 2))
        );
        // Nothing changed: nothing is said again.
        rig.engine.tick();
        assert!(rig.events().is_empty());
        // A failed listing keeps what was known.
        rig.engine.on_sessions(Err("boom".to_owned()));
        assert!(rig.engine.bound.is_some());
        assert!(rig.events().iter().all(|e| matches!(e, Event::Log(_))));
        // The assistant's own files move its presence.
        let mut report = SessionReport::new("working", rig.host.now());
        report.detail = Some("Run tests".to_owned());
        report.pid = Some(101);
        rig.engine
            .on_state_file(ASSISTANT_SID, Some(report), Some(("hello there".to_owned(), 999.0)));
        assert_eq!(rig.engine.presence, Presence::with_detail(Mood::Working, "Run tests"));
        assert_eq!(rig.engine.last_reply.as_deref(), Some("hello there"));
        // Another session's files do not.
        rig.engine
            .on_state_file(CREW_SID, Some(SessionReport::new("waiting", 1.0)), None);
        assert_eq!(rig.engine.presence.mood, Mood::Working);
        // The process dying puts it to sleep, and quickens the listing.
        rig.host.kill(101);
        rig.engine.tick();
        assert_eq!(rig.engine.presence.mood, Mood::Sleeping);
        assert!(rig.engine.cadence().lost);
    }

    #[test]
    fn crew_flags_are_derived_as_the_swift_app_derived_them() {
        let mut rig = Rig::new();
        let path = rig.crew_path();
        rig.host.write_transcript(
            &path,
            100,
            &[
                user(
                    "<command-name>/lets-do-it-v2</command-name><command-args>CASBO-1</command-args>",
                    "u1",
                ),
                said("Open the tab and log in.\n\nReply: `tab is up` or `skip`", "m1"),
            ],
        );
        rig.host.set_job(
            "c2",
            Some(JobInfo {
                state: JobState {
                    state: Some("blocked".to_owned()),
                    detail: Some("needs the login".to_owned()),
                    intent: Some("/lets-do-it-v2 CASBO-1".to_owned()),
                },
                needs: None,
            }),
        );
        rig.host.advance(130.0);
        rig.list(vec![assistant_row(), crew_row("idle", "blocked")]);
        let view = rig.crew_view();
        assert!(view.blocked && view.needs_you && !view.asks && !view.asleep);
        assert_eq!(view.desk, DeskState::NeedsYou);
        assert_eq!(view.line, "needs the login");
        assert_eq!(view.stage.as_deref(), Some("do-it-v2"));
        assert_eq!(view.quiet.as_deref(), Some("2m"));
        assert!(view.is_new);
        assert!(view.loud);
        assert_eq!(view.summary.prompt.as_deref(), Some("/lets-do-it-v2 CASBO-1"));
        let asks = rig.engine.ask_event().asks;
        assert_eq!(asks.len(), 1);
        assert_eq!(asks[0].answers, ["tab is up", "skip"]);
        assert_eq!(asks[0].what.as_deref(), Some("needs the login"));
        assert!(!asks[0].picker);
        // A picker: stopped on a permission prompt.
        rig.host.append_transcript(
            &path,
            &[json!({"type": "assistant", "message": {"id": "m2", "content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"description": "Look up the round"}}]}}).to_string()],
        );
        rig.list(vec![assistant_row(), crew_row("waiting", "working")]);
        let view = rig.crew_view();
        assert!(view.asks && !view.blocked && view.needs_you);
        assert_eq!(view.line, "needs you: Look up the round");
        let asks = rig.engine.ask_event().asks;
        assert!(asks[0].picker);
        assert_eq!(asks[0].what.as_deref(), Some("wants to look up the round"));
        // Working: the pending step is its line, and quiet is not shown.
        rig.list(vec![assistant_row(), crew_row("busy", "working")]);
        let view = rig.crew_view();
        assert_eq!(view.desk, DeskState::Working);
        assert_eq!(view.line, "Look up the round");
        assert_eq!(view.quiet, None);
        assert!(!view.loud);
        // Asleep while blocked: still on the bridge, with its sleep on its ask.
        let mut sleeper = crew_row("idle", "blocked");
        sleeper.pid = None;
        rig.host.set_job("c2", None);
        rig.list(vec![assistant_row(), sleeper]);
        let view = rig.crew_view();
        assert!(view.asleep && view.blocked);
        assert!(view.quiet.as_deref().is_some_and(|q| q.starts_with("asleep")));
        let asks = rig.engine.ask_event().asks;
        assert!(asks[0].asleep.as_deref().is_some_and(|a| a.starts_with("asleep")));
        // A chat's stage is "chat", and idle with nothing to say is "idle".
        let chat = row("c3", OTHER_SID, "chat-where-is-it", Some(303), "idle", "working", 3.0);
        rig.list(vec![assistant_row(), chat]);
        let members = rig.engine.crew_event().members;
        assert_eq!(members[0].stage.as_deref(), Some("chat"));
        assert_eq!(members[0].desk, DeskState::Idle);
        assert_eq!(members[0].line, "idle");
    }

    #[test]
    fn a_blocked_crew_member_calls_once_and_goes_quiet_after_attention_seen() {
        let mut rig = Rig::new();
        let path = rig.crew_path();
        rig.host.write_transcript(
            &path,
            100,
            &[user("go", "u1"), said("Log in please.\n\nReply: `logged in`", "m1")],
        );
        rig.list(vec![assistant_row(), crew_row("idle", "blocked")]);
        let events = rig.events();
        assert!(events.iter().any(|e| matches!(e, Event::Ask(a) if a.asks.len() == 1)));
        assert!(rig.crew_view().loud);
        rig.command(
            5,
            Command::AttentionSeen {
                session: CREW_SID.to_owned(),
            },
        );
        assert!(rig.replies().iter().any(|r| r.id == 5 && r.ok));
        let view = rig.crew_view();
        assert!(
            view.blocked && view.needs_you && !view.loud,
            "still pink, no longer calling"
        );
        assert!(!view.is_new, "seen counts as looked at");
        assert!(rig.engine.ask_event().asks.is_empty());
        assert!(
            rig.events()
                .iter()
                .any(|e| matches!(e, Event::Ask(a) if a.asks.is_empty()))
        );
        // What was seen is kept, so a restart stays quiet; a new ask calls again.
        let saved = Persisted::load(&rig.engine.config.state_path());
        assert_eq!(saved.crew_attention.len(), 1);
        rig.host
            .append_transcript(&path, &[said("Now approve it.\n\nReply: `approved`", "m2")]);
        rig.engine.tick();
        assert!(rig.crew_view().loud);
    }

    #[test]
    fn autopilot_answers_a_decide_line_once_after_the_grace_and_never_a_reply_line() {
        let mut rig = Rig::new();
        let path = rig.crew_path();
        rig.host.write_transcript(
            &path,
            100,
            &[
                user("go", "u1"),
                said("Two layouts fit.\n\nDecide: `compact` or `roomy`", "m1"),
            ],
        );
        rig.list(vec![assistant_row(), crew_row("idle", "blocked")]);
        // In human mode the ask is the human's.
        assert!(rig.crew_view().loud);
        assert!(rig.host.take_did().is_empty());
        rig.command(
            1,
            Command::Mode {
                pilot: Some(PilotMode::Autopilot),
                office_open: None,
            },
        );
        let view = rig.crew_view();
        assert!(!view.loud);
        assert_eq!(view.desk, DeskState::Working);
        assert_eq!(view.line, "autopilot answers \u{201C}compact\u{201D} in a moment");
        rig.host.advance(Autopilot::GRACE - 1.0);
        rig.engine.tick();
        assert!(rig.engine.drive().is_empty());
        assert!(rig.host.take_did().is_empty(), "not before the grace");
        rig.host.advance(1.0);
        rig.engine.tick();
        rig.engine.drive();
        assert_eq!(
            rig.host.take_did(),
            vec![Did::Send("c2".to_owned(), "Autopilot: compact".to_owned(), vec![])]
        );
        let view = rig.crew_view();
        assert_eq!(view.line, "autopilot answered \u{201C}compact\u{201D}");
        assert_eq!(
            view.autopilot.as_deref(),
            Some("Autopilot answered \u{201C}compact\u{201D}.")
        );
        // Never twice, however long it stands.
        rig.host.advance(100.0);
        rig.engine.tick();
        rig.engine.drive();
        assert!(rig.host.take_did().is_empty());
        assert_eq!(
            Persisted::load(&rig.engine.config.state_path()).autopilot_answers["cccccccc-0000-4000-8000-000000000002"]
                ["answer"],
            "compact"
        );
        // A Reply line is the human's, in either mode.
        rig.host
            .append_transcript(&path, &[said("Merge it?\n\nReply: `merged`", "m2")]);
        rig.engine.tick();
        rig.host.advance(100.0);
        rig.engine.tick();
        rig.engine.drive();
        assert!(rig.host.take_did().is_empty());
        assert!(rig.crew_view().loud);
        // A send that cannot be confirmed hands the ask back to the human.
        rig.host.set_send_outcome(Outcome::Unconfirmed("timed out".to_owned()));
        rig.host
            .append_transcript(&path, &[said("Which?\n\nDecide: `a` or `b`", "m3")]);
        rig.engine.tick();
        rig.host.advance(Autopilot::GRACE);
        rig.engine.tick();
        rig.engine.drive();
        assert_eq!(rig.host.take_did().len(), 1);
        let view = rig.crew_view();
        assert_eq!(view.desk, DeskState::NeedsYou);
        assert!(view.loud);
        assert!(
            view.autopilot
                .as_deref()
                .is_some_and(|a| a.contains("so this one is yours"))
        );
    }

    #[test]
    fn unread_replies_count_and_read() {
        let mut rig = Rig::new();
        rig.list(vec![assistant_row()]);
        rig.engine.on_state_file(
            ASSISTANT_SID,
            Some(SessionReport::new("idle", 1.0)),
            Some(("first".to_owned(), 100.0)),
        );
        assert_eq!(
            rig.engine.unread_event().count,
            0,
            "the first reply seen is the baseline"
        );
        rig.engine.on_state_file(
            ASSISTANT_SID,
            Some(SessionReport::new("idle", 1.0)),
            Some(("second".to_owned(), 200.0)),
        );
        rig.engine.on_state_file(
            ASSISTANT_SID,
            Some(SessionReport::new("idle", 1.0)),
            Some(("third".to_owned(), 300.0)),
        );
        let unread = rig.engine.unread_event();
        assert_eq!(unread.count, 2);
        assert_eq!(unread.newest, Some(300.0));
        assert!(
            rig.events()
                .iter()
                .any(|e| matches!(e, Event::Unread(u) if u.count == 2))
        );
        rig.command(3, Command::UnreadRead);
        assert_eq!(rig.engine.unread_event().count, 0);
        rig.engine.tick();
        assert_eq!(
            rig.engine.unread_event().count,
            0,
            "reading does not forget what was seen"
        );
        let saved = Persisted::load(&rig.engine.config.state_path());
        assert_eq!((saved.unread_count, saved.unread_newest), (0, Some(300.0)));
    }

    #[test]
    fn a_reply_is_caught_by_prompt_id() {
        let mut rig = Rig::new();
        rig.list(vec![assistant_row()]);
        rig.engine
            .on_state_file(ASSISTANT_SID, Some(SessionReport::new("idle", 900.0)), None);
        rig.command(
            9,
            Command::Send {
                target: Target::Assistant,
                message: "What is a chibi robot?".to_owned(),
                pictures: vec![],
                files: vec![],
            },
        );
        assert!(rig.replies().is_empty(), "the reply waits on the send");
        rig.engine.drive();
        let replies = rig.replies();
        assert_eq!(replies[0].id, 9);
        assert_eq!(
            replies[0].decode::<SendReply>(),
            Some(SendReply {
                delivered: true,
                reason: None
            })
        );
        assert!(rig.engine.cadence().urgent);
        assert_eq!(
            rig.host.take_did(),
            vec![Did::Send("a1".to_owned(), "What is a chibi robot?".to_owned(), vec![])]
        );
        // The turn starts: the hook names the prompt's id.
        rig.host.advance(1.0);
        let mut thinking = SessionReport::new("thinking", rig.host.now());
        thinking.turn = Some("What is a chibi robot?".to_owned());
        thinking.prompt_id = Some("p1".to_owned());
        thinking.pid = Some(101);
        rig.engine.on_state_file(ASSISTANT_SID, Some(thinking), None);
        assert_eq!(
            rig.engine
                .awaiting
                .as_ref()
                .and_then(|a| a.prompt_id.clone())
                .as_deref(),
            Some("p1")
        );
        // Another turn finishes first, with another id: not the answer.
        rig.host.advance(1.0);
        let mut other = SessionReport::new("done", rig.host.now());
        other.turn = Some("What is a chibi robot?".to_owned());
        other.prompt_id = Some("p0".to_owned());
        other.pid = Some(101);
        rig.engine.on_state_file(
            ASSISTANT_SID,
            Some(other),
            Some(("an older answer".to_owned(), rig.host.now())),
        );
        assert!(rig.engine.awaiting.is_some());
        assert!(!rig.events().iter().any(|e| matches!(e, Event::Reply(_))));
        // The turn with the id finishes: that is the answer, however the text was cut.
        rig.host.advance(1.0);
        let mut done = SessionReport::new("done", rig.host.now());
        done.turn = Some("something else".to_owned());
        done.prompt_id = Some("p1".to_owned());
        done.pid = Some(101);
        rig.engine.on_state_file(
            ASSISTANT_SID,
            Some(done),
            Some(("A small round robot.".to_owned(), rig.host.now())),
        );
        assert!(rig.engine.awaiting.is_none());
        let events = rig.events();
        let reply = events.iter().find_map(|e| match e {
            Event::Reply(r) => Some(r.clone()),
            _ => None,
        });
        let reply = reply.expect("the answer is pinned");
        assert_eq!(reply.prompt, "What is a chibi robot?");
        assert_eq!(reply.reply.as_deref(), Some("A small round robot."));
        assert!(!rig.engine.cadence().urgent);
    }

    #[test]
    fn a_message_is_lost_when_the_assistant_is_refreshed_or_the_crew_member_stops() {
        let mut rig = Rig::new();
        rig.list(vec![assistant_row(), crew_row("idle", "working")]);
        rig.command(
            1,
            Command::Send {
                target: Target::Assistant,
                message: "hello".to_owned(),
                pictures: vec![],
                files: vec![],
            },
        );
        rig.engine.drive();
        rig.events();
        let fresh = row("a2", OTHER_SID, "assistant", Some(102), "idle", "working", 5.0);
        rig.list(vec![fresh, crew_row("idle", "working")]);
        let lost = rig.events().into_iter().find_map(|e| match e {
            Event::Reply(r) => r.lost,
            _ => None,
        });
        assert_eq!(lost.as_deref(), Some("Refreshed before it answered"));
        // A crew member that vanishes from the list stopped before it answered.
        rig.command(
            2,
            Command::Send {
                target: Target::Crew(CREW_SID.to_owned()),
                message: "and you?".to_owned(),
                pictures: vec![],
                files: vec![],
            },
        );
        rig.engine.drive();
        rig.events();
        rig.list(vec![row(
            "a2",
            OTHER_SID,
            "assistant",
            Some(102),
            "idle",
            "working",
            5.0,
        )]);
        let lost = rig.events().into_iter().find_map(|e| match e {
            Event::Reply(r) => r.lost,
            _ => None,
        });
        assert_eq!(lost.as_deref(), Some("The session stopped before it answered"));
        // A crew member's answer: idle, replied to that prompt, quiet for two seconds.
        rig.list(vec![
            row("a2", OTHER_SID, "assistant", Some(102), "idle", "working", 5.0),
            crew_row("idle", "working"),
        ]);
        rig.command(
            3,
            Command::Answer {
                target: Target::Crew(CREW_SID.to_owned()),
                answer: "skip".to_owned(),
            },
        );
        rig.engine.drive();
        rig.events();
        let path = rig.crew_path();
        rig.host
            .write_transcript(&path, 1, &[user("skip", "u1"), said("Skipped.", "m1")]);
        rig.engine.tick();
        assert!(rig.engine.awaiting.is_some(), "the transcript just changed");
        rig.host.advance(3.0);
        rig.engine.tick();
        assert!(rig.engine.awaiting.is_none());
        let answered = rig.events().into_iter().find_map(|e| match e {
            Event::Reply(r) => r.reply,
            _ => None,
        });
        assert_eq!(answered.as_deref(), Some("Skipped."));
        // A session that is gone is never sent to.
        rig.command(
            4,
            Command::Send {
                target: Target::Crew("nobody".to_owned()),
                message: "x".to_owned(),
                pictures: vec![],
                files: vec![],
            },
        );
        assert!(rig.replies().iter().any(|r| r.id == 4 && !r.ok));
    }

    #[test]
    fn the_keep_alive_refreshes_an_idle_session_after_thirty_minutes_but_not_while_a_message_is_in_play() {
        let mut rig = Rig::new();
        // Before the first listing the keep-alive does nothing.
        rig.engine.keepalive_tick();
        assert!(rig.engine.drive().is_empty());
        rig.list(vec![assistant_row()]);
        let transcript = rig.host.locate_transcript(ASSISTANT_SID, "/w/repo");
        rig.host.write_transcript(&transcript, 1, &[user("hi", "u1")]);
        rig.engine.keepalive_tick();
        rig.engine.drive();
        assert!(rig.host.take_did().is_empty(), "fresh: left alone");
        // Idle for 31 minutes with a send on its way: left.
        rig.host.advance(31.0 * 60.0);
        rig.command(
            1,
            Command::Send {
                target: Target::Assistant,
                message: "hello".to_owned(),
                pictures: vec![],
                files: vec![],
            },
        );
        rig.engine.keepalive_tick();
        let effects = rig.engine.take_effects();
        assert!(effects.iter().all(|e| matches!(e, Effect::Send { .. })), "{effects:?}");
        rig.engine.effects = effects;
        rig.engine.drive();
        rig.host.take_did();
        // Delivered, so a standing ask: still left.
        assert!(rig.engine.awaiting.is_some());
        rig.engine.keepalive_tick();
        rig.engine.drive();
        assert!(rig.host.take_did().is_empty());
        // Answered: due, so refreshed: stop, then a fresh start with the assistant's flags.
        rig.engine.awaiting = None;
        rig.engine.keepalive_tick();
        rig.engine.drive();
        let did = rig.host.take_did();
        assert_eq!(did[0], Did::Stop("a1".to_owned()));
        match &did[1] {
            Did::Start(cwd, argv) => {
                assert_eq!(cwd, &rig.engine.config.assistant_dir());
                assert_eq!(
                    argv,
                    &assistant_flags("assistant", "opus", &rig.engine.config.home.to_string_lossy(), false)
                );
            }
            other => panic!("{other:?}"),
        }
        // No session at all: a fresh start.
        let mut rig = Rig::new();
        rig.list(vec![]);
        rig.engine.keepalive_tick();
        rig.engine.drive();
        assert!(matches!(rig.host.take_did().as_slice(), [Did::Start(_, _)]));
    }

    #[test]
    fn tool_calls_refuse_unknown_pids_and_serve_known_ones() {
        let mut rig = Rig::new();
        rig.list(vec![assistant_row(), crew_row("busy", "working")]);
        let call = |tool: &str, arguments: serde_json::Value, callers: Vec<u32>| Command::ToolCall {
            call: Call {
                tool: tool.to_owned(),
                arguments,
                callers,
            },
        };
        // An unknown caller has the sessions listed again, then is refused.
        rig.command(1, call("status", json!({"line": "on it"}), vec![999, 1]));
        assert!(rig.replies().is_empty());
        assert!(rig.engine.take_effects().contains(&Effect::ListNow));
        rig.list(vec![assistant_row(), crew_row("busy", "working")]);
        let replies = rig.replies();
        let refused = replies.iter().find(|r| r.id == 1).expect("answered after the listing");
        let result: ToolResult = refused.decode().expect("a tool result");
        assert!(result.is_error);
        assert_eq!(result.text, UNKNOWN_CALLER);
        // A crew session's status and ask are taken for its current turn.
        let path = rig.crew_path();
        rig.host.write_transcript(&path, 1, &[user("go", "u1")]);
        rig.command(2, call("status", json!({"line": "writing tests"}), vec![5000, 202]));
        rig.command(
            3,
            call(
                "ask",
                json!({"question": "Merge it?", "answers": ["yes", "no"]}),
                vec![202],
            ),
        );
        let replies = rig.replies();
        assert!(
            replies
                .iter()
                .all(|r| r.decode::<ToolResult>().is_some_and(|t| !t.is_error))
        );
        rig.list(vec![assistant_row(), crew_row("idle", "working")]);
        let view = rig.crew_view();
        assert_eq!(view.status.as_deref(), Some("writing tests"));
        assert_eq!(view.ask.as_ref().map(|a| a.question.as_str()), Some("Merge it?"));
        assert!(view.blocked, "an ask through Budd blocks the session on the human");
        assert!(
            view.reply
                .as_deref()
                .is_some_and(|r| r.ends_with("Reply: `yes` or `no`"))
        );
        // The next turn clears both.
        rig.host.append_transcript(&path, &[user("yes", "u2")]);
        rig.engine.tick();
        let view = rig.crew_view();
        assert_eq!(view.status, None);
        assert_eq!(view.ask, None);
        // The assistant's status is not shown; its ask is pinned on its card; say is an event.
        rig.command(4, call("status", json!({"line": "x"}), vec![101]));
        rig.command(
            5,
            call("ask", json!({"question": "Ship it?", "answers": ["ship"]}), vec![101]),
        );
        rig.command(6, call("say", json!({"text": "shipped"}), vec![101]));
        rig.command(7, call("dance", json!({}), vec![101]));
        rig.gather();
        let out = std::mem::take(&mut rig.out);
        let replies: Vec<Reply> = out
            .iter()
            .filter_map(|o| match o {
                Outgoing::Reply { reply, .. } => Some(reply.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            replies[0].decode::<ToolResult>().map(|t| t.text).as_deref(),
            Some(ASSISTANT_STATUS)
        );
        assert_eq!(
            replies[1].decode::<ToolResult>().map(|t| t.text).as_deref(),
            Some(ASSISTANT_ASK_TAKEN)
        );
        assert_eq!(
            replies[2].decode::<ToolResult>().map(|t| t.text).as_deref(),
            Some("Said.")
        );
        assert!(replies[3].decode::<ToolResult>().is_some_and(|t| t.is_error));
        assert!(out.iter().any(
            |o| matches!(o, Outgoing::Event(Event::Say(s)) if s.text == "shipped" && s.target == Target::Assistant)
        ));
        assert!(out.iter().any(|o| matches!(o, Outgoing::Event(Event::Reply(r)) if r.reply.as_deref().is_some_and(|t| t.contains("Ship it?")))));
        assert!(
            rig.engine
                .ask_event()
                .asks
                .iter()
                .any(|a| a.target == Target::Assistant && a.answers == ["ship"])
        );
    }

    #[test]
    fn archive_asks_first_while_it_works_and_hides_the_session_until_the_listing_catches_up() {
        let mut rig = Rig::new();
        rig.list(vec![assistant_row(), crew_row("busy", "working")]);
        rig.command(
            1,
            Command::Archive {
                session: CREW_SID.to_owned(),
                force: false,
            },
        );
        let replies = rig.replies();
        assert_eq!(
            replies[0].decode::<ArchiveReply>(),
            Some(ArchiveReply {
                stopped: false,
                asks_first: true
            })
        );
        assert!(rig.host.take_did().is_empty());
        rig.command(
            2,
            Command::Archive {
                session: CREW_SID.to_owned(),
                force: true,
            },
        );
        assert!(rig.engine.crew_event().members.is_empty(), "off the bridge at once");
        rig.engine.drive();
        assert_eq!(rig.host.take_did(), vec![Did::Stop("c2".to_owned())]);
        assert!(
            rig.replies()
                .iter()
                .any(|r| r.id == 2 && r.decode::<ArchiveReply>().is_some_and(|a| a.stopped))
        );
        // A failed stop puts it back, with the reason.
        rig.list(vec![assistant_row(), crew_row("idle", "working")]);
        rig.host.set_stop_fails(true);
        rig.command(
            3,
            Command::Archive {
                session: CREW_SID.to_owned(),
                force: false,
            },
        );
        rig.engine.drive();
        let replies = rig.replies();
        let failed = replies.iter().find(|r| r.id == 3).expect("answered");
        assert!(!failed.ok);
        assert!(
            failed
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with("Couldn't archive casbo-1"))
        );
        assert_eq!(rig.engine.crew_event().members.len(), 1);
    }

    #[test]
    fn hello_history_settings_mode_and_shutdown() {
        let mut rig = Rig::new();
        rig.list(vec![assistant_row(), crew_row("idle", "working")]);
        rig.command(
            1,
            Command::Hello {
                client: "ui".to_owned(),
                proto: PROTO,
                subscribe: None,
            },
        );
        rig.command(
            2,
            Command::Hello {
                client: "old".to_owned(),
                proto: 99,
                subscribe: None,
            },
        );
        let replies = rig.replies();
        let hello: HelloReply = replies[0].decode().expect("hello");
        assert_eq!(hello.proto, PROTO);
        assert_eq!(hello.snapshot.crew.members.len(), 1);
        assert_eq!(hello.snapshot.settings.assistant_name, "assistant");
        assert!(!replies[1].ok);
        // History pages back through the crew member's transcript.
        let path = rig.crew_path();
        let lines: Vec<String> = (1..=15)
            .flat_map(|n| {
                [
                    user(&format!("message {n}"), &format!("u{n}")),
                    said(&format!("answer {n}"), &format!("m{n}")),
                ]
            })
            .collect();
        rig.host.write_transcript(&path, 1, &lines);
        rig.command(
            3,
            Command::HistoryPage {
                target: Target::Crew(CREW_SID.to_owned()),
                earlier: false,
            },
        );
        rig.command(
            4,
            Command::HistoryPage {
                target: Target::Crew(CREW_SID.to_owned()),
                earlier: true,
            },
        );
        rig.command(
            5,
            Command::HistoryPage {
                target: Target::Crew("nobody".to_owned()),
                earlier: false,
            },
        );
        let replies = rig.replies();
        let first: HistoryReply = replies[0].decode().expect("history");
        assert!(
            first.turns.len() >= 10 && first.turns.len() <= 15,
            "{}",
            first.turns.len()
        );
        assert_eq!(
            first.turns.last().and_then(|t| t.prompt.clone()).as_deref(),
            Some("message 15")
        );
        let all: HistoryReply = replies[1].decode().expect("history");
        assert_eq!(all.turns.len(), 15);
        assert!(!all.has_earlier);
        assert!(!replies[2].ok);
        // Mode and settings persist; shutdown is an effect.
        rig.command(
            6,
            Command::Mode {
                pilot: Some(PilotMode::Autopilot),
                office_open: Some(false),
            },
        );
        rig.command(7, Command::SettingsGet);
        rig.command(8, Command::Shutdown);
        rig.command(
            9,
            Command::StartTickets {
                text: "CASBO-1".to_owned(),
            },
        );
        let replies = rig.replies();
        let settings: Settings = replies[1].decode().expect("settings");
        assert_eq!(settings.pilot_mode, PilotMode::Autopilot);
        assert!(!settings.office_open);
        assert_eq!(replies[3].error.as_deref(), Some("not yet"));
        assert!(rig.engine.take_effects().contains(&Effect::Shutdown));
        let saved = Persisted::load(&rig.engine.config.state_path());
        assert_eq!(saved.pilot_mode, PilotMode::Autopilot);
        assert!(!saved.office_open);
        // A poke reads the state files now.
        rig.command(10, Command::HookPoke { session: None });
        assert!(rig.engine.take_effects().contains(&Effect::ReadStateNow));
        assert!(rig.replies().is_empty());
    }

    #[test]
    fn the_answer_is_found_in_a_long_transcript() {
        let bulk = json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t", "content": "x".repeat(300_000)}]}}).to_string();
        let mut lines = vec![
            user("work on this", "u1"),
            said("Started it.", "m1"),
            user("next", "u2"),
        ];
        lines.extend(std::iter::repeat_n(bulk, 8));
        let source = budd_core::history::MemorySource::new(1, format!("{}\n", lines.join("\n")).into_bytes());
        assert_eq!(answer_in(&source, "work on this").as_deref(), Some("Started it."));
        assert_eq!(answer_in(&source, "never sent"), None);
    }
}
