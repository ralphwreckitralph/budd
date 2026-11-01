//! The lifecycle hook: event to state, the state file v2, the recent-context bridge (from assistant/.claude/hooks/avatar-state.py, session_size.py, recent-context.py).
//!
//! Everything here is pure: the binary that Claude Code runs on each event parses its stdin into a
//! [`HookInput`] and its environment into a [`HookEnv`], reads the previous state file, and hands them
//! to [`next_report`]; what comes back is the new state file. The transcript's size comes in as
//! `context`, already read by the transcript module, so this module never opens a file.

use crate::fingerprint::fingerprint;
use crate::presence::{Inbox, Mood, SessionReport};
use crate::text::{collapsed, is_work_command};
use serde::Deserialize;
use serde_json::Value;

/// The session is replaced at the next quiet moment once its context passes this many tokens.
///
/// The window is 1M, but each message re-reads the whole context, so a long one is slow and costly,
/// and a turn that starts near the window's edge can run into a compaction before the keep-alive acts.
/// One constant for the hook, the bridge and the keep-alive, so the three agree.
pub const ROTATE_AT: u64 = 400_000;

/// A detail line is cut to this many characters.
pub const DETAIL_CHARS: usize = 140;
/// A reply kept for the reply bubble is cut to this many characters.
pub const REPLY_CHARS: usize = 8000;

/// What each tool is doing, by tool name, for the detail line.
pub const TOOL_LABELS: &[(&str, &str)] = &[
    ("Read", "Reading"),
    ("Edit", "Editing"),
    ("MultiEdit", "Editing"),
    ("Write", "Writing"),
    ("NotebookEdit", "Editing a notebook"),
    ("WebFetch", "Reading the web"),
    ("WebSearch", "Searching the web"),
    ("Agent", "Asking a helper"),
    ("Task", "Asking a helper"),
    ("Skill", "Using a skill"),
    ("ToolSearch", "Finding tools"),
];

/// The JSON Claude Code writes to a hook's stdin. Every field is optional, as each event sends its own set.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct HookInput {
    /// The event: SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Notification, Stop, StopFailure, SessionEnd.
    pub hook_event_name: Option<String>,
    /// The session's id, the name of its transcript.
    pub session_id: Option<String>,
    /// The session's transcript.
    pub transcript_path: Option<String>,
    /// The folder the session runs in.
    pub cwd: Option<String>,
    /// The tool about to run or just run, on PreToolUse and PostToolUse.
    pub tool_name: Option<String>,
    /// The tool's input, whose shape depends on the tool.
    pub tool_input: Option<Value>,
    /// The tool call's id.
    pub tool_use_id: Option<String>,
    /// What kind of notification, such as "idle_prompt" or "permission_prompt".
    pub notification_type: Option<String>,
    /// The notification's text.
    pub message: Option<String>,
    /// Claude Code's id for the prompt, on UserPromptSubmit and the turn's later events.
    pub prompt_id: Option<String>,
    /// The prompt, on UserPromptSubmit.
    pub prompt: Option<String>,
    /// The reply, on Stop.
    pub last_assistant_message: Option<String>,
    /// The error, on StopFailure.
    pub error: Option<String>,
    /// How the session started: "startup", "resume", "clear", "compact", "fork".
    pub source: Option<String>,
    /// The session's name, on SessionStart and UserPromptSubmit.
    pub session_title: Option<String>,
    /// The permission mode the session runs in.
    pub permission_mode: Option<String>,
    /// The model the session runs on, on SessionStart.
    pub model: Option<String>,
    /// Whether a Stop hook already continued this turn.
    pub stop_hook_active: Option<bool>,
    /// Why the session ended, on SessionEnd.
    pub reason: Option<String>,
}

impl HookInput {
    /// The input in a hook's stdin text, or `None` when it is not JSON.
    pub fn decode(text: &str) -> Option<HookInput> {
        serde_json::from_str(text).ok()
    }

    fn event(&self) -> &str {
        self.hook_event_name.as_deref().unwrap_or("")
    }

    fn tool(&self) -> &str {
        self.tool_name.as_deref().unwrap_or("")
    }
}

/// The variables Claude Code sets in a hook's environment, as far as the state file needs them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookEnv {
    /// The Claude process, as text.
    pub claude_pid: Option<String>,
    /// The session's folder under `jobs/`, whose name is its short id.
    pub claude_job_dir: Option<String>,
    /// The config folder the session runs under.
    pub claude_config_dir: Option<String>,
    /// The session's id.
    pub claude_code_session_id: Option<String>,
    /// The socket that takes messages for the session.
    pub claude_code_messaging_socket: Option<String>,
    /// The token the socket wants.
    pub claude_code_messaging_token: Option<String>,
    /// The project folder.
    pub claude_project_dir: Option<String>,
    /// A file a SessionStart hook may write environment variables to.
    pub claude_env_file: Option<String>,
}

impl HookEnv {
    /// The variables among `pairs` this module reads; other variables are ignored.
    pub fn from_pairs<K: AsRef<str>, V: Into<String>>(pairs: impl IntoIterator<Item = (K, V)>) -> HookEnv {
        let mut env = HookEnv::default();
        for (key, value) in pairs {
            let slot = match key.as_ref() {
                "CLAUDE_PID" => &mut env.claude_pid,
                "CLAUDE_JOB_DIR" => &mut env.claude_job_dir,
                "CLAUDE_CONFIG_DIR" => &mut env.claude_config_dir,
                "CLAUDE_CODE_SESSION_ID" => &mut env.claude_code_session_id,
                "CLAUDE_CODE_MESSAGING_SOCKET" => &mut env.claude_code_messaging_socket,
                "CLAUDE_CODE_MESSAGING_TOKEN" => &mut env.claude_code_messaging_token,
                "CLAUDE_PROJECT_DIR" => &mut env.claude_project_dir,
                "CLAUDE_ENV_FILE" => &mut env.claude_env_file,
                _ => continue,
            };
            *slot = Some(value.into());
        }
        env
    }

    /// The Claude process id, when the environment names one.
    pub fn pid(&self) -> Option<u32> {
        self.claude_pid.as_deref().and_then(|pid| pid.trim().parse().ok())
    }

    /// The session's short id: the name of its job folder.
    pub fn short_id(&self) -> Option<String> {
        self.claude_job_dir
            .as_deref()
            .map(basename)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
    }

    /// Where the session takes messages, when the environment says.
    pub fn inbox(&self) -> Option<Inbox> {
        Some(Inbox {
            path: self.claude_code_messaging_socket.clone()?,
            token: self.claude_code_messaging_token.clone(),
        })
    }
}

/// What one hook event says about the session: the state and its detail line, and the tool while working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// The mood the state file records.
    pub state: Mood,
    /// One line about it, already shortened.
    pub detail: Option<String>,
    /// The running tool's name, only while working.
    pub tool: Option<String>,
}

/// The part of a path after its last slash, as `os.path.basename` gives it.
fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// `text` with its whitespace collapsed and cut to [`DETAIL_CHARS`]: 139 characters, trailing spaces
/// dropped, then "…".
pub fn short(text: &str) -> String {
    let text = collapsed(text);
    if text.chars().count() <= DETAIL_CHARS {
        return text;
    }
    let head: String = text.chars().take(DETAIL_CHARS - 1).collect();
    format!("{}…", head.trim_end())
}

/// What a tool call is doing, for the detail line while working.
pub fn tool_detail(input: &HookInput) -> String {
    let name = input.tool();
    let field = |key: &str| {
        input
            .tool_input
            .as_ref()
            .and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let label = |name: &str| {
        TOOL_LABELS
            .iter()
            .find(|(tool, _)| *tool == name)
            .map(|(_, label)| *label)
    };
    if name == "Bash" {
        return field("description").unwrap_or("Running a command").to_owned();
    }
    if matches!(name, "Read" | "Edit" | "MultiEdit" | "Write")
        && let Some(path) = field("file_path")
    {
        return format!("{} {}", label(name).unwrap_or("Working…"), basename(path));
    }
    if name.starts_with("mcp__claude-in-chrome__") {
        return "Using Chrome".to_owned();
    }
    if name.starts_with("mcp__") {
        let server = name.split("__").nth(1).unwrap_or("");
        return format!("Using {}", server.replace("claude_ai_", "").replace('_', " "));
    }
    label(name).unwrap_or("Working…").to_owned()
}

/// The state one hook event puts the session in, or `None` to leave the state file alone.
pub fn report(input: &HookInput) -> Option<Verdict> {
    let verdict = |state: Mood, detail: Option<String>| {
        let tool = if state == Mood::Working {
            input.tool_name.clone().filter(|t| !t.is_empty())
        } else {
            None
        };
        Some(Verdict { state, detail, tool })
    };
    let non_empty = |text: Option<&str>| text.filter(|t| !t.is_empty()).map(str::to_owned);
    match input.event() {
        "SessionStart" => verdict(Mood::Idle, None),
        "UserPromptSubmit" => verdict(Mood::Thinking, None),
        "PreToolUse" => {
            if input.tool() == "AskUserQuestion" {
                let question = input
                    .tool_input
                    .as_ref()
                    .and_then(|v| v.get("questions"))
                    .and_then(Value::as_array)
                    .and_then(|qs| qs.first())
                    .and_then(|q| q.get("question"))
                    .and_then(Value::as_str)
                    .filter(|q| !q.is_empty())
                    .unwrap_or("Has a question for you");
                return verdict(Mood::Waiting, Some(short(question)));
            }
            verdict(Mood::Working, Some(short(&tool_detail(input))))
        }
        "PostToolUse" => verdict(Mood::Thinking, None),
        "Notification" => {
            // idle_prompt only means the turn ended a while ago, which Stop already covered.
            if input.notification_type.as_deref() == Some("idle_prompt") {
                return None;
            }
            let message = non_empty(input.message.as_deref()).unwrap_or_else(|| "Needs you".to_owned());
            verdict(Mood::Waiting, Some(short(&message)))
        }
        "Stop" => {
            let detail = input
                .last_assistant_message
                .as_deref()
                .map(short)
                .filter(|d| !d.is_empty());
            verdict(Mood::Done, detail)
        }
        "StopFailure" => {
            let text = non_empty(input.last_assistant_message.as_deref())
                .or_else(|| non_empty(input.error.as_deref()))
                .unwrap_or_else(|| "The turn failed".to_owned());
            verdict(Mood::Failed, Some(short(&text)))
        }
        "SessionEnd" => verdict(Mood::Sleeping, Some("Session ended".to_owned())),
        _ => None,
    }
}

/// The state file after `input`, given the file as it was (`previous`), the hook's environment, the
/// session's context size and limit, and the time.
///
/// `turn`, `prompt_id`, `turn_hash` and `command` are set on UserPromptSubmit and carried forward
/// through the turn's other events; `name`, `model`, `permission_mode` and the inbox are carried forward
/// from the previous file when the event does not say them; `pid_start` is carried while the pid stays
/// the same, and is the caller's to fill in when it changes, as only the OS knows it. The report is
/// built even when [`report`] says to leave the file alone, so check that first.
pub fn next_report(
    previous: Option<&SessionReport>,
    input: &HookInput,
    env: &HookEnv,
    context: Option<u64>,
    context_limit: Option<u64>,
    now: f64,
) -> SessionReport {
    let verdict = report(input).unwrap_or(Verdict {
        state: Mood::Idle,
        detail: None,
        tool: None,
    });
    let carried = |mine: Option<&String>, theirs: fn(&SessionReport) -> Option<&String>| {
        mine.cloned().or_else(|| previous.and_then(theirs).cloned())
    };
    let mut next = SessionReport::new(verdict.state.name(), now);
    next.detail = verdict.detail;
    next.tool = verdict.tool;
    next.pid = env.pid();
    next.transcript = input.transcript_path.clone();
    next.context = context;
    next.context_limit = context_limit;
    next.v = Some(2);
    next.session_id = input.session_id.clone().or_else(|| env.claude_code_session_id.clone());
    next.short_id = env.short_id().or_else(|| previous.and_then(|p| p.short_id.clone()));
    next.name = carried(input.session_title.as_ref(), |p| p.name.as_ref());
    next.cwd = input.cwd.clone().or_else(|| env.claude_project_dir.clone());
    next.permission_mode = carried(input.permission_mode.as_ref(), |p| p.permission_mode.as_ref());
    next.model = carried(input.model.as_ref(), |p| p.model.as_ref());
    next.inbox = env.inbox().or_else(|| previous.and_then(|p| p.inbox.clone()));
    next.pid_start = previous
        .filter(|p| p.pid == next.pid && p.pid.is_some())
        .and_then(|p| p.pid_start);
    if input.event() == "UserPromptSubmit" {
        let prompt = input.prompt.as_deref().unwrap_or("");
        let whole = collapsed(prompt);
        next.turn = Some(short(prompt));
        next.prompt_id = input.prompt_id.clone();
        next.turn_hash = (!whole.is_empty()).then(|| fingerprint(&whole));
        next.command = if is_work_command(&whole) {
            Some(whole)
        } else {
            previous.and_then(|p| p.command.clone())
        };
        next.links = Vec::new();
    } else if let Some(previous) = previous {
        next.turn = previous.turn.clone();
        next.prompt_id = previous.prompt_id.clone().or_else(|| input.prompt_id.clone());
        next.turn_hash = previous.turn_hash.clone();
        next.command = previous.command.clone();
        next.links = previous.links.clone();
    } else {
        next.prompt_id = input.prompt_id.clone();
    }
    if input.event() == "SessionEnd" {
        next.end_reason = input.reason.clone().or_else(|| Some("other".to_owned()));
    }
    next
}

/// The reply a Stop or StopFailure event carries, for the reply file: the last assistant message, else
/// the error, trimmed and cut to [`REPLY_CHARS`]; `None` when there is nothing to keep.
pub fn reply_text(input: &HookInput) -> Option<String> {
    let text = input
        .last_assistant_message
        .as_deref()
        .filter(|t| !t.is_empty())
        .or(input.error.as_deref())
        .unwrap_or("")
        .trim();
    if text.is_empty() {
        return None;
    }
    Some(text.chars().take(REPLY_CHARS).collect())
}

/// The name of the session every bridge is from.
pub const ASSISTANT: &str = "assistant";
/// Claude Code writes the agent-name entry within the first few lines of a transcript.
pub const NAME_WITHIN: usize = 20;
/// (messages, characters per message, characters in all) the bridge carries after a quiet spell.
pub const AFTER_QUIET: (usize, usize, usize) = (8, 600, 3000);
/// The same after a conversation that was replaced for growing too long.
pub const AFTER_LONG: (usize, usize, usize) = (40, 4000, 20000);

/// A transcript that might be the previous conversation: its file name, its modification time, and its
/// first lines (the first [`NAME_WITHIN`] are enough).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    /// The transcript's file name, such as "f754f44d-….jsonl".
    pub name: String,
    /// When it was last written, in seconds since the Unix epoch.
    pub mtime: f64,
    /// Its first lines, as written.
    pub first_lines: Vec<String>,
}

/// The session's name from a transcript's first lines, or `None` for an unnamed session.
pub fn agent_name(first_lines: &[String]) -> Option<String> {
    first_lines
        .iter()
        .take(NAME_WITHIN)
        .filter(|line| line.contains("\"agent-name\""))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|entry| entry.get("type").and_then(Value::as_str) == Some("agent-name"))
        .and_then(|entry| entry.get("agentName").and_then(Value::as_str).map(str::to_owned))
}

/// The indexes of the assistant's own earlier transcripts among `candidates`, newest first, leaving out
/// the session's own (`own` is its id). The caller walks them until one has messages to bridge.
pub fn assistant_transcripts(candidates: &[Candidate], own: &str) -> Vec<usize> {
    let own_file = format!("{own}.jsonl");
    let mut found: Vec<usize> = (0..candidates.len())
        .filter(|&i| candidates[i].name != own_file)
        .filter(|&i| agent_name(&candidates[i].first_lines).as_deref() == Some(ASSISTANT))
        .collect();
    found.sort_by(|&a, &b| candidates[b].mtime.total_cmp(&candidates[a].mtime));
    found
}

/// The newest of the assistant's earlier transcripts among `candidates`, or `None` without one.
pub fn select_previous(candidates: &[Candidate], own: &str) -> Option<usize> {
    assistant_transcripts(candidates, own).into_iter().next()
}

/// What a fresh session is told about the previous conversation, from its messages as `(role, text,
/// timestamp)` with roles "user" and "assistant". `long` is whether that conversation was replaced for
/// size, which makes the bridge much larger. Empty when there are no messages.
pub fn bridge_text(messages: &[(String, String, String)], long: bool) -> String {
    let Some(last) = messages.last() else {
        return String::new();
    };
    let (max_messages, max_message_chars, max_total_chars) = if long { AFTER_LONG } else { AFTER_QUIET };
    let start = messages.len().saturating_sub(max_messages);
    let lines: Vec<String> = messages[start..]
        .iter()
        .map(|(role, text, _)| {
            let text = if text.chars().count() > max_message_chars {
                format!("{} [...]", text.chars().take(max_message_chars).collect::<String>())
            } else {
                text.clone()
            };
            format!("{}{}", if role == "user" { "human: " } else { "assistant: " }, text)
        })
        .collect();
    let mut body = lines.join("\n\n");
    let total = body.chars().count();
    if total > max_total_chars {
        body = format!(
            "[...] {}",
            body.chars().skip(total - max_total_chars).collect::<String>()
        );
    }
    let why = if long {
        " because it grew too long; this is its recent stretch word for word"
    } else {
        ""
    };
    format!(
        "Previous conversation (refreshed{why}; last message at {}). \
         Use it only for continuity, and ask the human when it is not enough:\n\n{body}",
        last.2
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// One recorded hook call: the event's stdin and environment.
    fn fixture(name: &str) -> (HookInput, HookEnv) {
        let text = match name {
            "00-SessionStart" => include_str!("../tests/fixtures/hooks/00-SessionStart.json"),
            "01-UserPromptSubmit" => include_str!("../tests/fixtures/hooks/01-UserPromptSubmit.json"),
            "02-PreToolUse" => include_str!("../tests/fixtures/hooks/02-PreToolUse.json"),
            "03-PostToolUse" => include_str!("../tests/fixtures/hooks/03-PostToolUse.json"),
            "04-Stop" => include_str!("../tests/fixtures/hooks/04-Stop.json"),
            "05-Notification" => include_str!("../tests/fixtures/hooks/05-Notification.json"),
            "09-SessionEnd" => include_str!("../tests/fixtures/hooks/09-SessionEnd.json"),
            "10-SessionStart" => include_str!("../tests/fixtures/hooks/10-SessionStart.json"),
            _ => panic!("no fixture {name}"),
        };
        let recorded: Value = serde_json::from_str(text).unwrap();
        let input: HookInput = serde_json::from_value(recorded["stdin"].clone()).unwrap();
        let env = HookEnv::from_pairs(
            recorded["env"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str().unwrap().to_owned())),
        );
        (input, env)
    }

    fn input(event: &str) -> HookInput {
        HookInput {
            hook_event_name: Some(event.to_owned()),
            ..HookInput::default()
        }
    }

    fn tool(name: &str, tool_input: Value) -> HookInput {
        HookInput {
            tool_name: Some(name.to_owned()),
            tool_input: Some(tool_input),
            ..input("PreToolUse")
        }
    }

    #[test]
    fn each_event_has_its_state() {
        assert_eq!(report(&input("SessionStart")).unwrap().state, Mood::Idle);
        assert_eq!(report(&input("UserPromptSubmit")).unwrap().state, Mood::Thinking);
        assert_eq!(report(&input("PostToolUse")).unwrap().state, Mood::Thinking);
        let end = report(&input("SessionEnd")).unwrap();
        assert_eq!(
            (end.state, end.detail.as_deref()),
            (Mood::Sleeping, Some("Session ended"))
        );
        assert_eq!(report(&input("SomethingNew")), None);
        assert_eq!(report(&HookInput::default()), None);
    }

    #[test]
    fn the_recorded_events_read_as_the_python_hook_read_them() {
        let (start, _) = fixture("00-SessionStart");
        assert_eq!(report(&start).unwrap().state, Mood::Idle);
        let (prompt, _) = fixture("01-UserPromptSubmit");
        assert_eq!(report(&prompt).unwrap().state, Mood::Thinking);
        let (pre, _) = fixture("02-PreToolUse");
        assert_eq!(
            report(&pre).unwrap(),
            Verdict {
                state: Mood::Working,
                detail: Some("Print pong".to_owned()),
                tool: Some("Bash".to_owned())
            }
        );
        let (post, _) = fixture("03-PostToolUse");
        assert_eq!(report(&post).unwrap().tool, None);
        let (stop, _) = fixture("04-Stop");
        assert_eq!(report(&stop).unwrap().detail.as_deref(), Some("pong"));
        assert_eq!(reply_text(&stop).as_deref(), Some("pong"));
        // idle_prompt leaves the file alone.
        let (notification, _) = fixture("05-Notification");
        assert_eq!(report(&notification), None);
    }

    #[test]
    fn tool_details_name_the_step() {
        assert_eq!(
            tool_detail(&tool("Bash", json!({"command": "ls"}))),
            "Running a command"
        );
        assert_eq!(
            tool_detail(&tool("Bash", json!({"description": "List files"}))),
            "List files"
        );
        assert_eq!(
            tool_detail(&tool("Read", json!({"file_path": "/a/b/main.rs"}))),
            "Reading main.rs"
        );
        assert_eq!(tool_detail(&tool("Edit", json!({"file_path": "x.py"}))), "Editing x.py");
        assert_eq!(tool_detail(&tool("Write", json!({}))), "Writing");
        assert_eq!(
            tool_detail(&tool("mcp__claude-in-chrome__navigate", json!({}))),
            "Using Chrome"
        );
        assert_eq!(
            tool_detail(&tool("mcp__claude_ai_Atlassian__getJiraIssue", json!({}))),
            "Using Atlassian"
        );
        assert_eq!(
            tool_detail(&tool("mcp__plugin_pdf-viewer_pdf__open", json!({}))),
            "Using plugin pdf-viewer pdf"
        );
        assert_eq!(tool_detail(&tool("WebSearch", json!({}))), "Searching the web");
        assert_eq!(tool_detail(&tool("Grep", json!({}))), "Working…");
    }

    #[test]
    fn a_question_is_waiting() {
        let asking = tool("AskUserQuestion", json!({"questions": [{"question": "Which one?"}]}));
        let verdict = report(&asking).unwrap();
        assert_eq!(
            (verdict.state, verdict.detail.as_deref()),
            (Mood::Waiting, Some("Which one?"))
        );
        assert_eq!(verdict.tool, None);
        let blank = tool("AskUserQuestion", json!({"questions": []}));
        assert_eq!(
            report(&blank).unwrap().detail.as_deref(),
            Some("Has a question for you")
        );
    }

    #[test]
    fn notifications_and_failures() {
        let permission = HookInput {
            notification_type: Some("permission_prompt".to_owned()),
            message: Some("Claude needs your permission to use Bash".to_owned()),
            ..input("Notification")
        };
        let verdict = report(&permission).unwrap();
        assert_eq!(verdict.state, Mood::Waiting);
        assert_eq!(
            verdict.detail.as_deref(),
            Some("Claude needs your permission to use Bash")
        );
        let silent = HookInput {
            notification_type: Some("other".to_owned()),
            ..input("Notification")
        };
        assert_eq!(report(&silent).unwrap().detail.as_deref(), Some("Needs you"));
        let failure = HookInput {
            error: Some("  Rate limit reached  ".to_owned()),
            ..input("StopFailure")
        };
        let verdict = report(&failure).unwrap();
        assert_eq!(
            (verdict.state, verdict.detail.as_deref()),
            (Mood::Failed, Some("Rate limit reached"))
        );
        assert_eq!(reply_text(&failure).as_deref(), Some("Rate limit reached"));
        assert_eq!(
            report(&input("StopFailure")).unwrap().detail.as_deref(),
            Some("The turn failed")
        );
        // A Stop with nothing said has no detail and no reply.
        let quiet = input("Stop");
        assert_eq!(report(&quiet).unwrap().detail, None);
        assert_eq!(reply_text(&quiet), None);
    }

    #[test]
    fn details_are_cut_and_replies_capped() {
        assert_eq!(short("  a \n b  "), "a b");
        let long = "x".repeat(200);
        let cut = short(&long);
        assert_eq!(cut.chars().count(), 140);
        assert!(cut.ends_with('…'));
        // Trailing spaces before the cut are dropped.
        let spaced = format!("{} {}", "y".repeat(138), "z".repeat(50));
        assert_eq!(short(&spaced), format!("{}…", "y".repeat(138)));
        let reply = HookInput {
            last_assistant_message: Some(format!("  {}  ", "r".repeat(9000))),
            ..input("Stop")
        };
        assert_eq!(reply_text(&reply).unwrap().chars().count(), REPLY_CHARS);
    }

    #[test]
    fn the_environment_is_read_by_name() {
        let (_, env) = fixture("00-SessionStart");
        assert_eq!(env.pid(), Some(5054));
        assert_eq!(env.short_id().as_deref(), Some("f754f44d"));
        assert_eq!(env.claude_config_dir.as_deref(), Some("/Users/alice/.claude-budd"));
        assert_eq!(env.inbox().unwrap().path, "/tmp/cc-socks/5054.sock");
        assert_eq!(env.inbox().unwrap().token, None);
        assert!(env.claude_env_file.is_some());
        let bare = HookEnv::from_pairs([("CLAUDE_PID", "nope"), ("CLAUDE_JOB_DIR", "/jobs/")]);
        assert_eq!(bare.pid(), None);
        assert_eq!(bare.short_id(), None);
        assert_eq!(bare.inbox(), None);
    }

    #[test]
    fn the_state_file_follows_a_turn() {
        let (start, env) = fixture("00-SessionStart");
        let first = next_report(None, &start, &env, None, Some(ROTATE_AT), 10.0);
        assert_eq!(first.state, "idle");
        assert_eq!(first.v, Some(2));
        assert_eq!(first.pid, Some(5054));
        assert_eq!(first.short_id.as_deref(), Some("f754f44d"));
        assert_eq!(
            first.session_id.as_deref(),
            Some("f754f44d-10ef-4d93-9284-042d19dc90e5")
        );
        assert_eq!(first.name.as_deref(), Some("spike0"));
        assert_eq!(first.model.as_deref(), Some("claude-haiku-5-5"));
        assert_eq!(first.cwd.as_deref(), Some("/Users/alice/webserver/assistant-avatar"));
        assert_eq!(first.context_limit, Some(ROTATE_AT));
        assert_eq!(first.turn, None);
        assert_eq!(first.at, 10.0);

        let (prompt, env) = fixture("01-UserPromptSubmit");
        let second = next_report(Some(&first), &prompt, &env, Some(1200), Some(ROTATE_AT), 11.0);
        assert_eq!(second.state, "thinking");
        assert_eq!(
            second.turn.as_deref(),
            Some("Run the shell command: echo pong. Then reply with the single word pong.")
        );
        assert_eq!(
            second.prompt_id.as_deref(),
            Some("b6ede332-f5f1-4c29-8dbc-c8737dd6743e")
        );
        assert!(second.turn_hash.is_some());
        assert_eq!(second.permission_mode.as_deref(), Some("auto"));
        // The model is not in this event; it is carried from the file.
        assert_eq!(second.model.as_deref(), Some("claude-haiku-5-5"));
        assert_eq!(second.context, Some(1200));

        let (pre, env) = fixture("02-PreToolUse");
        let third = next_report(Some(&second), &pre, &env, Some(1300), Some(ROTATE_AT), 12.0);
        assert_eq!(third.state, "working");
        assert_eq!(third.detail.as_deref(), Some("Print pong"));
        assert_eq!(third.tool.as_deref(), Some("Bash"));
        // The turn carries through the tool calls, as does the name the event does not repeat.
        assert_eq!(third.turn, second.turn);
        assert_eq!(third.turn_hash, second.turn_hash);
        assert_eq!(third.name.as_deref(), Some("spike0"));

        let (stop, env) = fixture("04-Stop");
        let fourth = next_report(Some(&third), &stop, &env, Some(1400), Some(ROTATE_AT), 13.0);
        assert_eq!(fourth.state, "done");
        assert_eq!(fourth.tool, None);
        assert_eq!(fourth.turn, second.turn);
        assert!(fourth.finishes_turn(
            "Run the shell command: echo pong. Then reply with the single word pong.",
            None
        ));
        assert!(fourth.finishes_turn("anything", Some("b6ede332-f5f1-4c29-8dbc-c8737dd6743e")));

        let (end, env) = fixture("09-SessionEnd");
        let last = next_report(Some(&fourth), &end, &env, Some(1400), Some(ROTATE_AT), 14.0);
        assert_eq!(last.state, "sleeping");
        assert_eq!(last.end_reason.as_deref(), Some("other"));
        // The inbox outlives the event that stopped naming it.
        assert_eq!(
            last.inbox.as_ref().map(|i| i.path.as_str()),
            Some("/tmp/cc-socks/5054.sock")
        );
    }

    #[test]
    fn the_work_command_and_the_pid_start() {
        let env = HookEnv::from_pairs([("CLAUDE_PID", "7")]);
        let mut previous = SessionReport::new("idle", 0.0);
        previous.pid = Some(7);
        previous.pid_start = Some(123.0);
        let work = HookInput {
            prompt: Some("/lets-do-it  CASBO-1".to_owned()),
            ..input("UserPromptSubmit")
        };
        let next = next_report(Some(&previous), &work, &env, None, None, 1.0);
        assert_eq!(next.command.as_deref(), Some("/lets-do-it CASBO-1"));
        assert_eq!(next.pid_start, Some(123.0));
        let chat = HookInput {
            prompt: Some("and the other one?".to_owned()),
            ..input("UserPromptSubmit")
        };
        let after = next_report(Some(&next), &chat, &env, None, None, 2.0);
        assert_eq!(after.command.as_deref(), Some("/lets-do-it CASBO-1"));
        let builtin = HookInput {
            prompt: Some("/clear".to_owned()),
            ..input("UserPromptSubmit")
        };
        assert_eq!(
            next_report(Some(&after), &builtin, &env, None, None, 3.0)
                .command
                .as_deref(),
            Some("/lets-do-it CASBO-1")
        );
        // A new process does not inherit the old one's start time.
        let other = HookEnv::from_pairs([("CLAUDE_PID", "8")]);
        assert_eq!(next_report(Some(&next), &chat, &other, None, None, 2.0).pid_start, None);
        // A prompt without text still starts a turn, with nothing to match.
        let empty = next_report(None, &input("UserPromptSubmit"), &env, None, None, 1.0);
        assert_eq!(empty.turn.as_deref(), Some(""));
        assert_eq!(empty.turn_hash, None);
    }

    #[test]
    fn a_forked_session_starts_its_own_file() {
        let (start, env) = fixture("10-SessionStart");
        let report = next_report(None, &start, &env, None, Some(ROTATE_AT), 1.0);
        assert_eq!(report.short_id.as_deref(), Some("cc7d868c"));
        assert_eq!(report.pid, Some(1943));
        assert_eq!(report.model, None);
        assert_eq!(report.turn, None);
        assert_eq!(report.command, None);
    }

    fn candidate(name: &str, mtime: f64, agent: Option<&str>) -> Candidate {
        let mut first_lines = vec![json!({"type": "user", "message": {"content": "hi"}}).to_string()];
        if let Some(agent) = agent {
            first_lines.push(json!({"type": "agent-name", "agentName": agent}).to_string());
        }
        Candidate {
            name: name.to_owned(),
            mtime,
            first_lines,
        }
    }

    #[test]
    fn the_previous_conversation_is_the_newest_of_the_assistants_own() {
        let candidates = [
            candidate("own.jsonl", 500.0, Some("assistant")),
            candidate("chat.jsonl", 400.0, Some("chat-where-is-it")),
            candidate("old.jsonl", 100.0, Some("assistant")),
            candidate("newer.jsonl", 300.0, Some("assistant")),
            candidate("unnamed.jsonl", 450.0, None),
        ];
        assert_eq!(select_previous(&candidates, "own"), Some(3));
        assert_eq!(assistant_transcripts(&candidates, "own"), vec![3, 2]);
        assert_eq!(select_previous(&candidates[..2], "own"), None);
        assert_eq!(
            agent_name(&candidates[1].first_lines).as_deref(),
            Some("chat-where-is-it")
        );
        // The name must be within the first lines; a mention of the key alone is not it.
        let mut late = candidate("late.jsonl", 1.0, None);
        late.first_lines
            .extend((0..NAME_WITHIN).map(|_| json!({"type": "user", "message": {"content": "x"}}).to_string()));
        late.first_lines
            .push(json!({"type": "agent-name", "agentName": "assistant"}).to_string());
        assert_eq!(agent_name(&late.first_lines), None);
        let spoof = Candidate {
            name: "s.jsonl".to_owned(),
            mtime: 1.0,
            first_lines: vec![json!({"type": "user", "message": {"content": "\"agent-name\""}}).to_string()],
        };
        assert_eq!(agent_name(&spoof.first_lines), None);
    }

    fn messages(count: usize, chars: usize) -> Vec<(String, String, String)> {
        (0..count)
            .map(|i| {
                let role = if i % 2 == 0 { "user" } else { "assistant" };
                (role.to_owned(), format!("{i}:{}", "m".repeat(chars)), format!("t{i}"))
            })
            .collect()
    }

    #[test]
    fn the_bridge_after_a_quiet_spell_is_short() {
        let text = bridge_text(&messages(2, 3), false);
        assert_eq!(
            text,
            "Previous conversation (refreshed; last message at t1). \
             Use it only for continuity, and ask the human when it is not enough:\n\n\
             human: 0:mmm\n\nassistant: 1:mmm"
        );
        // Eight messages at most, each cut to 600 characters, the whole to 3000 from the end.
        let text = bridge_text(&messages(20, 1000), false);
        assert!(text.contains("last message at t19"));
        let body = text.split_once(":\n\n").unwrap().1;
        assert!(body.starts_with("[...] "));
        assert_eq!(body.chars().count(), 3000 + "[...] ".len());
        assert!(body.contains(" [...]\n\nhuman: 18:"));
        assert!(!text.contains("\n\nhuman: 10:"));
        assert!(body.ends_with(&format!("{} [...]", "m".repeat(600 - 3))));
        assert_eq!(bridge_text(&[], false), "");
    }

    #[test]
    fn the_bridge_after_a_long_conversation_is_large() {
        let text = bridge_text(&messages(50, 100), true);
        assert!(text.starts_with(
            "Previous conversation (refreshed because it grew too long; this is its recent stretch word for word; \
             last message at t49). Use it only for continuity, and ask the human when it is not enough:\n\n\
             human: 10:"
        ));
        assert!(!text.contains("[...]"));
        // Forty messages at most, each cut to 4000, the whole to 20000.
        let text = bridge_text(&messages(50, 5000), true);
        let body = text.split_once(":\n\n").unwrap().1;
        assert!(body.starts_with("[...] "));
        assert_eq!(body.chars().count(), 20000 + "[...] ".len());
        assert!(body.ends_with(&format!("{} [...]", "m".repeat(4000 - 3))));
    }
}
