//! The body of `budd hook`: read the event, update the session's state file, answer Claude Code.
//!
//! Claude Code runs `budd hook` on every lifecycle event of a session, with the event as JSON on stdin
//! and the session's facts in the environment. The logic is `budd_core::hook`; this module does the
//! reading and writing around it: the previous state file, the transcript's size, the process start time,
//! the new state file and reply file, and on a fresh start the recent-context bridge printed for the
//! session to read. It replaces the Python hooks avatar-state.py, session_size.py and recent-context.py.
//!
//! The hook never fails: whatever goes wrong is written to `<home>/logs/hook.log` and the process exits 0,
//! since a broken hook must not disturb the session.

pub mod transcript_io;

use crate::procs;
use crate::state_files::Dir;
use budd_core::hook::{
    ASSISTANT, Candidate, HookEnv, HookInput, NAME_WITHIN, ROTATE_AT, agent_name, assistant_transcripts, bridge_text,
    next_report, reply_text, report,
};
use budd_core::presence::SessionReport;
use budd_core::text::content_text;
use budd_core::transcript::Entry;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use transcript_io::{context_tokens, locate_transcript};

/// State files untouched for this long are removed when a session starts.
pub const PRUNE_AFTER: Duration = Duration::from_secs(7 * 24 * 3600);
/// The variable that names the assistant session; "assistant" when unset.
pub const ASSISTANT_NAME_VAR: &str = "BUDD_ASSISTANT_NAME";
/// The variable naming a second folder the state files also go to, for the macOS app that reads it.
pub const LEGACY_STATE_DIR_VAR: &str = "BUDD_LEGACY_STATE_DIR";
/// The variable that moves the Budd home folder from `~/.budd`.
pub const HOME_VAR: &str = "BUDD_HOME";
/// A session whose name starts with this is a chat Budd opened, which starts with the assistant's recent past.
pub const CHAT_PREFIX: &str = "chat-";

/// Where the hook reads and writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dirs {
    /// Budd's own state files.
    pub sessions: Dir,
    /// A second folder that gets the same files, for the existing macOS app, when configured.
    pub legacy: Option<Dir>,
}

/// What one run of the hook did: the report it wrote, if any, and what went wrong on the side.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    /// The state file written, or `None` when the event leaves the file alone or names no session.
    pub report: Option<SessionReport>,
    /// Failures that did not stop the report from being written: the legacy copy, pruning, the bridge.
    pub warnings: Vec<String>,
}

/// Why the hook could not do its main job.
#[derive(Debug, thiserror::Error)]
pub enum HookError {
    /// Stdin did not hold a JSON object.
    #[error("the event on stdin is not JSON")]
    NotJson,
    /// The state file could not be written.
    #[error("writing the state file: {0}")]
    Io(#[from] io::Error),
}

/// The Budd home folder: `BUDD_HOME`, else `~/.budd`, as `paths::BuddHome` resolves it; with neither set,
/// `./.budd`, so a hook never fails for want of a home.
pub(crate) fn budd_home(lookup: impl Fn(&str) -> Option<String>) -> PathBuf {
    let user_home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    crate::paths::BuddHome::from_vars(lookup(HOME_VAR).map(Into::into), lookup(user_home).map(Into::into))
        .map(|home| home.root().to_path_buf())
        .unwrap_or_else(|_| PathBuf::from(".").join(".budd"))
}

/// Appends one line about `message` to `<home>/logs/hook.log`, with the time. Failing to log is ignored.
pub fn log(home: &Path, message: &str) {
    let logs = home.join("logs");
    if fs::create_dir_all(&logs).is_err() {
        return;
    }
    if let Ok(mut file) = OpenOptions::new().append(true).create(true).open(logs.join("hook.log")) {
        let _ = writeln!(file, "{:.3} {}", now(), message.replace('\n', " "));
    }
}

/// Seconds since the Unix epoch, as the state file records time.
fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Runs the hook as the binary does: the event on stdin, the real environment, the folders under the Budd
/// home. Never fails: errors go to the log, and the process exits 0 either way.
pub fn main() {
    let env: Vec<(String, String)> = std::env::vars().collect();
    let home = budd_home(|key| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()));
    let dirs = Dirs {
        sessions: Dir(home.join("state").join("sessions")),
        legacy: env
            .iter()
            .find(|(k, _)| k == LEGACY_STATE_DIR_VAR)
            .map(|(_, v)| v.trim())
            .filter(|v| !v.is_empty())
            .map(|v| Dir(PathBuf::from(v))),
    };
    let mut input = String::new();
    if let Err(error) = io::stdin().lock().read_to_string(&mut input) {
        log(&home, &format!("reading stdin: {error}"));
    }
    let outcome = std::panic::catch_unwind(|| {
        let mut out = io::stdout().lock();
        let result = run(&input, &env, &dirs, &mut out);
        let _ = out.flush();
        result
    });
    match outcome {
        Ok(Ok(outcome)) => {
            for warning in outcome.warnings {
                log(&home, &warning);
            }
        }
        Ok(Err(error)) => log(&home, &error.to_string()),
        Err(_) => log(&home, "the hook panicked"),
    }
}

/// Runs the hook over `input`, the event as Claude Code writes it to stdin, with the environment `env`.
/// Writes the session's state file (and reply file on Stop and StopFailure) under `dirs`, and prints the
/// recent-context bridge to `out` on a fresh start of the assistant or a chat.
pub fn run(input: &str, env: &[(String, String)], dirs: &Dirs, out: &mut dyn Write) -> Result<Outcome, HookError> {
    let input = HookInput::decode(input).ok_or(HookError::NotJson)?;
    let hook_env = HookEnv::from_pairs(env.iter().map(|(k, v)| (k.as_str(), v.clone())));
    let lookup = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
    let mut outcome = Outcome::default();
    if report(&input).is_none() {
        return Ok(outcome);
    }
    let Some(session_id) = input
        .session_id
        .clone()
        .or_else(|| hook_env.claude_code_session_id.clone())
        .filter(|id| !id.is_empty())
    else {
        return Ok(outcome);
    };
    let event = input.hook_event_name.as_deref().unwrap_or("");
    let previous = dirs.sessions.read_report(&session_id);

    let config_dir = lookup("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| config_dir_of(input.transcript_path.as_deref()));
    let transcript = transcript_of(&input, &session_id, config_dir.as_deref());
    let context = if matches!(event, "PostToolUse" | "Stop" | "StopFailure") {
        transcript.as_deref().and_then(context_tokens)
    } else {
        previous.as_ref().and_then(|p| p.context)
    };

    let mut next = next_report(previous.as_ref(), &input, &hook_env, context, Some(ROTATE_AT), now());
    if next.pid_start.is_none()
        && let Some(pid) = next.pid
    {
        next.pid_start = procs::start_time(pid).map(|seconds| seconds as f64);
    }
    if next.transcript.is_none() {
        next.transcript = transcript.as_ref().map(|path| path.to_string_lossy().into_owned());
    }

    dirs.sessions.write_report(&next)?;
    if let Some(legacy) = &dirs.legacy
        && let Err(error) = legacy.write_report(&next)
    {
        outcome.warnings.push(format!("legacy state file: {error}"));
    }
    if matches!(event, "Stop" | "StopFailure")
        && let Some(reply) = reply_text(&input)
    {
        dirs.sessions.write_reply(&session_id, &reply)?;
        if let Some(legacy) = &dirs.legacy
            && let Err(error) = legacy.write_reply(&session_id, &reply)
        {
            outcome.warnings.push(format!("legacy reply file: {error}"));
        }
    }
    if event == "SessionStart" {
        for dir in std::iter::once(&dirs.sessions).chain(dirs.legacy.iter()) {
            if let Err(error) = dir.prune(PRUNE_AFTER) {
                outcome
                    .warnings
                    .push(format!("pruning {}: {error}", dir.path().display()));
            }
        }
        let assistant = lookup(ASSISTANT_NAME_VAR)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| ASSISTANT.to_owned());
        if wants_bridge(&input, &assistant)
            && let Some(folder) = transcript.as_deref().and_then(Path::parent)
        {
            match bridge(folder, &session_id, &assistant) {
                Ok(Some(text)) => {
                    if let Err(error) = writeln!(out, "{text}") {
                        outcome.warnings.push(format!("printing the bridge: {error}"));
                    }
                }
                Ok(None) => {}
                Err(error) => outcome.warnings.push(format!("the bridge: {error}")),
            }
        }
    }
    outcome.report = Some(next);
    Ok(outcome)
}

/// The config folder a transcript path is under: `<config>/projects/<slug>/<id>.jsonl`.
fn config_dir_of(transcript_path: Option<&str>) -> Option<PathBuf> {
    let path = Path::new(transcript_path?);
    let projects = path.parent()?.parent()?;
    if projects.file_name()?.to_str()? != "projects" {
        return None;
    }
    Some(projects.parent()?.to_path_buf())
}

/// The session's transcript: the path the event names when that file exists, else the newest file named
/// after the session under the config folder, or where one would go.
fn transcript_of(input: &HookInput, session_id: &str, config_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = input.transcript_path.as_deref().map(PathBuf::from)
        && path.is_file()
    {
        return Some(path);
    }
    let cwd = input.cwd.as_deref().unwrap_or("");
    Some(locate_transcript(config_dir?, session_id, cwd))
}

/// Whether this SessionStart gets the recent-context bridge: a startup or a clear, of the assistant
/// itself or of a chat, which starts with the assistant's recent stretch. A resumed, compacted or forked
/// session already has its context.
fn wants_bridge(input: &HookInput, assistant: &str) -> bool {
    if !matches!(input.source.as_deref(), Some("startup") | Some("clear")) {
        return false;
    }
    input
        .session_title
        .as_deref()
        .is_some_and(|name| name == assistant || name.starts_with(CHAT_PREFIX))
}

/// The bridge text from the newest earlier conversation of `assistant` in `folder` that has messages,
/// or `None` when there is none. `own` is this session's id, whose transcript is never its own past.
fn bridge(folder: &Path, own: &str, assistant: &str) -> io::Result<Option<String>> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        // No project folder yet: the very first session here has no past.
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            continue;
        };
        if !name.ends_with(".jsonl") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else { continue };
        if !metadata.is_file() {
            continue;
        }
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let first_lines = first_lines(&path, NAME_WITHIN);
        candidates.push((
            path,
            Candidate {
                name,
                mtime,
                first_lines,
            },
        ));
    }
    let heads: Vec<Candidate> = candidates.iter().map(|(_, c)| c.clone()).collect();
    for index in named_transcripts(&heads, own, assistant) {
        let path = &candidates[index].0;
        let messages = messages_of(path);
        if messages.is_empty() {
            continue;
        }
        let long = context_tokens(path).unwrap_or(0) >= ROTATE_AT;
        return Ok(Some(bridge_text(&messages, long)));
    }
    Ok(None)
}

/// The indexes of `assistant`'s earlier transcripts among `candidates`, newest first, leaving out `own`'s.
/// The core's rule for the default name, and the same rule by another name otherwise.
fn named_transcripts(candidates: &[Candidate], own: &str, assistant: &str) -> Vec<usize> {
    if assistant == ASSISTANT {
        return assistant_transcripts(candidates, own);
    }
    let own_file = format!("{own}.jsonl");
    let mut found: Vec<usize> = (0..candidates.len())
        .filter(|&i| candidates[i].name != own_file)
        .filter(|&i| agent_name(&candidates[i].first_lines).as_deref() == Some(assistant))
        .collect();
    found.sort_by(|&a, &b| candidates[b].mtime.total_cmp(&candidates[a].mtime));
    found
}

/// The first `count` lines of the file at `path`, as written; nothing when it cannot be read.
fn first_lines(path: &Path, count: usize) -> Vec<String> {
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    // The name is within the first lines, which are short; 64 KiB is plenty and bounds the read.
    let mut head = Vec::new();
    if file.take(64 * 1024).read_to_end(&mut head).is_err() {
        return Vec::new();
    }
    String::from_utf8_lossy(&head)
        .split('\n')
        .take(count)
        .map(str::to_owned)
        .collect()
}

/// The conversation in the transcript at `path`, as (role, text, timestamp) for every human and assistant
/// message with plain text: no tool calls, commands, shell input or hook noise, which are wrapped in tags.
pub fn messages_of(path: &Path) -> Vec<(String, String, String)> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut found = Vec::new();
    for line in text.split('\n') {
        let Some(entry) = Entry::parse(line) else { continue };
        let Some(role) = entry.kind.as_deref().filter(|k| matches!(*k, "user" | "assistant")) else {
            continue;
        };
        if entry.is_meta() || entry.is_sidechain() {
            continue;
        }
        let Some(content) = entry.content().and_then(content_text) else {
            continue;
        };
        let content = content.trim();
        // Slash commands, ! shell input and system reminders are wrapped in tags; they are not conversation.
        if content.is_empty() || content.starts_with('<') {
            continue;
        }
        found.push((
            role.to_owned(),
            content.to_owned(),
            entry.timestamp.clone().unwrap_or_default(),
        ));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::fs::File;

    const HOOKS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/2.1.316/hooks");
    const TRANSCRIPTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/2.1.316/transcripts");
    const SPIKE0: &str = "f754f44d-10ef-4d93-9284-042d19dc90e5";
    const FORK: &str = "cc7d868c-70f8-4ba2-9c41-4352667c6a4a";

    /// One recorded hook call: its file name, its stdin as JSON text, and its environment.
    type Recorded = (String, String, Vec<(String, String)>);

    /// The recorded hook calls, in order.
    fn fixtures() -> Vec<Recorded> {
        let mut names: Vec<String> = fs::read_dir(HOOKS)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".json"))
            .collect();
        names.sort();
        names
            .into_iter()
            .map(|name| {
                let recorded: Value =
                    serde_json::from_str(&fs::read_to_string(Path::new(HOOKS).join(&name)).unwrap()).unwrap();
                let stdin = recorded["stdin"].to_string();
                let env = recorded["env"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
                    .collect();
                (name, stdin, env)
            })
            .collect()
    }

    /// A config folder holding both recorded transcripts under the project folder the fixtures name.
    fn config_with_transcripts(root: &Path) -> PathBuf {
        let config = root.join("claude-budd");
        let project = config.join("projects").join("-Users-alice-webserver-assistant-avatar");
        fs::create_dir_all(&project).unwrap();
        fs::copy(
            Path::new(TRANSCRIPTS).join("bg-haiku-f754f44d.jsonl"),
            project.join(format!("{SPIKE0}.jsonl")),
        )
        .unwrap();
        fs::copy(
            Path::new(TRANSCRIPTS).join("bg-haiku-cc7d868c.jsonl"),
            project.join(format!("{FORK}.jsonl")),
        )
        .unwrap();
        config
    }

    fn with_env(mut env: Vec<(String, String)>, extra: &[(&str, &str)]) -> Vec<(String, String)> {
        for (key, value) in extra {
            env.retain(|(k, _)| k != key);
            env.push(((*key).to_owned(), (*value).to_owned()));
        }
        env
    }

    #[test]
    fn the_recorded_session_runs_through_the_state_files() {
        let temp = tempfile::tempdir().unwrap();
        let config = config_with_transcripts(temp.path());
        let dirs = Dirs {
            sessions: Dir(temp.path().join("sessions")),
            legacy: Some(Dir(temp.path().join("legacy"))),
        };
        let extra = [
            ("CLAUDE_CONFIG_DIR", config.to_str().unwrap()),
            (ASSISTANT_NAME_VAR, "spike0"),
        ];
        let mut states = Vec::new();
        let mut bridges = Vec::new();
        for (name, stdin, env) in fixtures() {
            let mut out = Vec::new();
            let outcome =
                run(&stdin, &with_env(env, &extra), &dirs, &mut out).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(outcome.warnings.is_empty(), "{name}: {:?}", outcome.warnings);
            if let Some(report) = &outcome.report {
                states.push((name.clone(), report.state.clone()));
            }
            if !out.is_empty() {
                bridges.push((name, String::from_utf8(out).unwrap()));
            }
        }
        let by_name = |n: &str| {
            states
                .iter()
                .find(|(name, _)| name.starts_with(n))
                .map(|(_, s)| s.as_str())
        };
        assert_eq!(by_name("00"), Some("idle"));
        assert_eq!(by_name("01"), Some("thinking"));
        assert_eq!(by_name("02"), Some("working"));
        assert_eq!(by_name("03"), Some("thinking"));
        assert_eq!(by_name("04"), Some("done"));
        // idle_prompt notifications leave the file alone.
        assert_eq!(by_name("05"), None);
        assert_eq!(by_name("09"), Some("sleeping"));
        assert_eq!(by_name("10"), Some("idle"));
        assert_eq!(by_name("14"), Some("sleeping"));

        let last = dirs.sessions.read_report(SPIKE0).unwrap();
        assert_eq!(last.state, "sleeping");
        assert_eq!(last.pid, Some(5054));
        assert_eq!(last.short_id.as_deref(), Some("f754f44d"));
        assert_eq!(last.name.as_deref(), Some("spike0"));
        assert_eq!(last.v, Some(2));
        assert_eq!(last.model.as_deref(), Some("claude-haiku-5-5"));
        // The second turn's prompt id, carried from 06-UserPromptSubmit through the end.
        assert_eq!(last.prompt_id.as_deref(), Some("c5622bcc-a6b4-410a-bc91-72c49b4e1953"));
        assert_eq!(last.turn.as_deref(), Some("Reply with the single word pang."));
        assert_eq!(last.context, Some(47_422));
        assert_eq!(last.context_limit, Some(ROTATE_AT));
        assert_eq!(last.end_reason.as_deref(), Some("other"));
        assert_eq!(
            last.transcript.as_deref(),
            Some(
                format!("/Users/alice/.claude-budd/projects/-Users-alice-webserver-assistant-avatar/{SPIKE0}.jsonl")
                    .as_str()
            )
        );
        // The reply file holds the last reply, here from the second Stop.
        assert_eq!(fs::read_to_string(dirs.sessions.reply_path(SPIKE0)).unwrap(), "pang");
        // The legacy folder got the same files.
        let legacy = dirs.legacy.as_ref().unwrap();
        assert_eq!(legacy.read_report(SPIKE0), Some(last.clone()));
        assert_eq!(fs::read_to_string(legacy.reply_path(SPIKE0)).unwrap(), "pang");
        // The forked session has a file of its own.
        let fork = dirs.sessions.read_report(FORK).unwrap();
        assert_eq!(fork.pid, Some(1943));
        assert_eq!(fork.short_id.as_deref(), Some("cc7d868c"));
        assert_eq!(fork.state, "sleeping");
        // Its transcript was found under the config folder, since the recorded path does not exist here.
        assert_eq!(fork.context, Some(48_304));

        // The startup of spike0 got the bridge from the other spike0 transcript; the fork did not.
        assert_eq!(bridges.len(), 1, "{bridges:?}");
        assert!(bridges[0].0.starts_with("00-"));
        let text = &bridges[0].1;
        assert!(text.starts_with("Previous conversation (refreshed; last message at 2026-11-01T04:14:40.332Z)."));
        assert!(text.contains("human: Run the shell command: echo pong. Then reply with the single word pong.\n\nassistant: pong\n\nassistant: pang\n\nhuman: Reply with the single word ping.\n\nassistant: ping\n"));
    }

    #[test]
    fn the_first_stop_writes_pong_and_the_turn_is_carried() {
        let temp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            sessions: Dir(temp.path().join("sessions")),
            legacy: None,
        };
        let all = fixtures();
        let mut out = Vec::new();
        for (name, stdin, env) in all.iter().take(5) {
            let outcome = run(stdin, env, &dirs, &mut out).unwrap_or_else(|e| panic!("{name}: {e}"));
            let report = outcome.report.unwrap();
            assert_eq!(report.pid, Some(5054));
            assert_eq!(report.short_id.as_deref(), Some("f754f44d"));
            assert_eq!(report.name.as_deref(), Some("spike0"));
            if name.starts_with("00") {
                assert_eq!(report.turn, None);
            } else {
                assert_eq!(
                    report.turn.as_deref(),
                    Some("Run the shell command: echo pong. Then reply with the single word pong.")
                );
                assert_eq!(
                    report.prompt_id.as_deref(),
                    Some("b6ede332-f5f1-4c29-8dbc-c8737dd6743e")
                );
            }
            if name.starts_with("02") {
                assert_eq!(report.detail.as_deref(), Some("Print pong"));
                assert_eq!(report.tool.as_deref(), Some("Bash"));
            }
        }
        assert_eq!(fs::read_to_string(dirs.sessions.reply_path(SPIKE0)).unwrap(), "pong");
        assert_eq!(dirs.sessions.read_report(SPIKE0).unwrap().state, "done");
        // Nothing was printed: the transcript folder the fixture names does not exist here.
        assert!(out.is_empty());
    }

    #[test]
    fn garbage_and_eventless_input_do_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            sessions: Dir(temp.path().join("sessions")),
            legacy: None,
        };
        let mut out = Vec::new();
        assert!(matches!(run("not json", &[], &dirs, &mut out), Err(HookError::NotJson)));
        let quiet = run(
            r#"{"hook_event_name":"SomethingNew","session_id":"s"}"#,
            &[],
            &dirs,
            &mut out,
        )
        .unwrap();
        assert_eq!(quiet.report, None);
        let nameless = run(r#"{"hook_event_name":"Stop"}"#, &[], &dirs, &mut out).unwrap();
        assert_eq!(nameless.report, None);
        assert!(!dirs.sessions.path().exists());
        assert!(out.is_empty());
    }

    #[test]
    fn a_session_start_prunes_old_files() {
        let temp = tempfile::tempdir().unwrap();
        let dirs = Dirs {
            sessions: Dir(temp.path().join("sessions")),
            legacy: None,
        };
        fs::create_dir_all(dirs.sessions.path()).unwrap();
        let old = dirs.sessions.report_path("old");
        File::create(&old).unwrap();
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(SystemTime::now() - PRUNE_AFTER - Duration::from_secs(60))
            .unwrap();
        let (_, stdin, env) = fixtures().remove(0);
        let mut out = Vec::new();
        run(&stdin, &env, &dirs, &mut out).unwrap();
        assert!(!old.exists());
        assert!(dirs.sessions.report_path(SPIKE0).exists());
    }

    #[test]
    fn the_bridge_is_for_the_assistant_and_chats_on_a_fresh_start() {
        let start = |source: &str, title: &str| HookInput {
            hook_event_name: Some("SessionStart".to_owned()),
            source: Some(source.to_owned()),
            session_title: Some(title.to_owned()),
            ..HookInput::default()
        };
        assert!(wants_bridge(&start("startup", "assistant"), "assistant"));
        assert!(wants_bridge(&start("clear", "assistant"), "assistant"));
        assert!(wants_bridge(&start("startup", "chat-where-is-it"), "assistant"));
        assert!(!wants_bridge(&start("resume", "assistant"), "assistant"));
        assert!(!wants_bridge(&start("compact", "assistant"), "assistant"));
        assert!(!wants_bridge(&start("fork", "assistant"), "assistant"));
        assert!(!wants_bridge(&start("startup", "crew-1"), "assistant"));
        assert!(!wants_bridge(&start("startup", "assistant"), "spike0"));
        assert!(wants_bridge(&start("startup", "spike0"), "spike0"));
        assert!(!wants_bridge(&HookInput::default(), "assistant"));
    }

    #[test]
    fn the_bridge_reads_the_newest_earlier_transcript_with_messages() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path();
        let own = "own";
        let line = |type_: &str, text: &str, stamp: &str| {
            format!(r#"{{"type":"{type_}","timestamp":"{stamp}","message":{{"content":"{text}"}}}}"#)
        };
        let named = |name: &str| format!(r#"{{"type":"agent-name","agentName":"{name}"}}"#);
        // The assistant's older conversation.
        fs::write(
            folder.join("older.jsonl"),
            [
                named("assistant"),
                line("user", "first", "t1"),
                line("assistant", "one", "t2"),
            ]
            .join("\n"),
        )
        .unwrap();
        // Newer, but a chat's: never the assistant's past.
        fs::write(
            folder.join("chat.jsonl"),
            [named("chat-x"), line("user", "chat", "t9")].join("\n"),
        )
        .unwrap();
        // Newest of the assistant's, but empty of messages: skipped.
        fs::write(
            folder.join("empty.jsonl"),
            [
                named("assistant"),
                line("user", "<command-name>/clear</command-name>", "t8"),
                r#"{"type":"user","isMeta":true,"message":{"content":"reminder"}}"#.to_owned(),
            ]
            .join("\n"),
        )
        .unwrap();
        fs::write(folder.join(format!("{own}.jsonl")), named("assistant")).unwrap();
        let then = SystemTime::now() - Duration::from_secs(600);
        File::options()
            .write(true)
            .open(folder.join("older.jsonl"))
            .unwrap()
            .set_modified(then)
            .unwrap();

        let text = bridge(folder, own, "assistant").unwrap().unwrap();
        assert!(text.ends_with("human: first\n\nassistant: one"), "{text}");
        assert!(text.contains("last message at t2"));
        assert_eq!(bridge(folder, own, "someone-else").unwrap(), None);
        assert_eq!(bridge(&folder.join("missing"), own, "assistant").unwrap(), None);
        assert_eq!(
            messages_of(&folder.join("older.jsonl")),
            vec![
                ("user".to_owned(), "first".to_owned(), "t1".to_owned()),
                ("assistant".to_owned(), "one".to_owned(), "t2".to_owned()),
            ]
        );
        assert!(messages_of(&folder.join("empty.jsonl")).is_empty());
    }

    #[test]
    fn the_home_folder_comes_from_the_environment() {
        let lookup = |pairs: &'static [(&str, &str)]| {
            move |key: &str| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| (*v).to_owned())
        };
        assert_eq!(
            budd_home(lookup(&[("BUDD_HOME", "/opt/budd")])),
            PathBuf::from("/opt/budd")
        );
        let user_home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        assert_eq!(
            budd_home(lookup(if cfg!(windows) {
                &[("USERPROFILE", "/u"), ("BUDD_HOME", "")]
            } else {
                &[("HOME", "/u"), ("BUDD_HOME", "")]
            })),
            PathBuf::from("/u").join(".budd")
        );
        let _ = user_home;
        assert_eq!(budd_home(lookup(&[])), PathBuf::from(".").join(".budd"));
        assert_eq!(
            config_dir_of(Some("/c/projects/-x/id.jsonl")),
            Some(PathBuf::from("/c"))
        );
        assert_eq!(config_dir_of(Some("/c/other/-x/id.jsonl")), None);
        assert_eq!(config_dir_of(None), None);
    }
}
