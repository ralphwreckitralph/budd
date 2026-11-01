//! The delivery path end to end against `fake-claude`: a real pty, a real child, a real transcript file.
//!
//! The fake is found next to this test binary in `target/<profile>/`; `cargo test -p budd-claude` builds
//! it when `fake-claude` is in the invocation (`cargo test -p budd-claude -p fake-claude` or
//! `--workspace`), and otherwise these tests build it themselves with `cargo build -p fake-claude`.
//! `FAKE_CLAUDE=<path>` points them at another build. When no fake can be had, each test says so and
//! passes without checking anything, so look for "skipping" in the output.

use budd_claude::claude_cli::Claude;
use budd_claude::messenger::{self, Message, Outcome, Target, Via};
use budd_claude::pty::{Attach, DEFAULT_SIZE};
use budd_core::delivery::PathStyle;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const FAKE: &str = if cfg!(windows) {
    "fake-claude.exe"
} else {
    "fake-claude"
};
const STYLE: PathStyle = if cfg!(windows) {
    PathStyle::WindowsQuoted
} else {
    PathStyle::Posix
};

/// Where the fake is, built if need be, or `None` with the reason printed.
fn fake_claude() -> Option<PathBuf> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            if let Some(given) = std::env::var_os("FAKE_CLAUDE") {
                return Some(PathBuf::from(given));
            }
            let candidate = beside_this_binary()?;
            if !candidate.is_file() {
                let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
                let built = Command::new(cargo)
                    .args(["build", "-p", "fake-claude", "--quiet"])
                    .current_dir(env!("CARGO_MANIFEST_DIR"))
                    .status()
                    .is_ok_and(|status| status.success());
                if !built || !candidate.is_file() {
                    eprintln!(
                        "skipping: {} is missing; run `cargo build -p fake-claude` first",
                        candidate.display()
                    );
                    return None;
                }
            }
            Some(candidate)
        })
        .clone()
}

/// `target/<profile>/fake-claude`, from this test binary's place in `target/<profile>/deps/`.
fn beside_this_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let deps = exe.parent()?;
    let profile = if deps.file_name().is_some_and(|name| name == "deps") {
        deps.parent()?
    } else {
        deps
    };
    Some(profile.join(FAKE))
}

/// One test's world: a config folder with the fake's settings, a transcript, and an agents listing.
struct World {
    temp: tempfile::TempDir,
    claude: Claude,
}

impl World {
    fn new(fake: &Path) -> World {
        let temp = tempfile::tempdir().expect("a temp folder");
        let config = temp.path().join("config");
        fs::create_dir_all(&config).expect("the config folder");
        let claude = Claude::new(fake, &config);
        let world = World { temp, claude };
        world.settings(&[]);
        world
    }

    fn path(&self, name: &str) -> PathBuf {
        self.temp.path().join(name)
    }

    fn transcript(&self) -> PathBuf {
        self.path("transcript.jsonl")
    }

    fn agents(&self) -> PathBuf {
        self.path("agents.json")
    }

    /// Writes the fake's settings: the transcript and agents file unless `extra` says otherwise, plus `extra`.
    fn settings(&self, extra: &[(&str, &str)]) {
        let transcript = self.transcript().display().to_string();
        let agents = self.agents().display().to_string();
        let defaults = [
            ("FAKE_CLAUDE_TRANSCRIPT", transcript.as_str()),
            ("FAKE_CLAUDE_AGENTS", agents.as_str()),
        ];
        let lines: Vec<String> = extra
            .iter()
            .chain(
                defaults
                    .iter()
                    .filter(|(name, _)| extra.iter().all(|(given, _)| given != name)),
            )
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        fs::write(self.claude.config_dir.join("fake-claude.env"), lines.join("\n")).expect("the settings");
    }

    fn write_agents(&self, path: &Path, pid: Option<u32>, status: &str, session_id: &str) {
        let row = serde_json::json!({
            "id": "abc12345",
            "sessionId": session_id,
            "name": "toy",
            "cwd": "/tmp",
            "pid": pid,
            "status": status,
            "state": if pid.is_some() { "working" } else { "blocked" },
            "kind": "background",
        });
        fs::write(path, serde_json::json!([row]).to_string()).expect("the listing");
    }

    fn target(&self) -> Target {
        Target {
            short_id: "abc12345".to_owned(),
            session_id: "11111111-1111-4111-8111-111111111111".to_owned(),
            transcript: Some(self.transcript()),
            asleep: false,
        }
    }

    fn transcript_lines(&self) -> Vec<serde_json::Value> {
        fs::read_to_string(self.transcript())
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("a JSON line"))
            .collect()
    }
}

fn message(text: &str) -> Message {
    Message {
        text: text.to_owned(),
        ..Message::default()
    }
}

#[test]
fn an_idle_session_records_a_user_entry() {
    let Some(fake) = fake_claude() else { return };
    let world = World::new(&fake);
    fs::write(
        world.transcript(),
        "{\"type\":\"user\",\"message\":{\"content\":\"earlier\"}}\n",
    )
    .expect("the transcript");
    let outcome = messenger::send(
        &world.claude,
        &world.target(),
        &message("Reply with exactly: pineapple"),
        STYLE,
    );
    assert_eq!(outcome, Outcome::Delivered { via: Via::Pty });
    let lines = world.transcript_lines();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["type"], "user");
    assert_eq!(lines[1]["message"]["content"], "Reply with exactly: pineapple");
}

#[test]
fn a_busy_session_queues_it() {
    let Some(fake) = fake_claude() else { return };
    let world = World::new(&fake);
    world.settings(&[("FAKE_CLAUDE_BUSY", "1")]);
    let outcome = messenger::send(&world.claude, &world.target(), &message("second-done please"), STYLE);
    assert_eq!(outcome, Outcome::Delivered { via: Via::Pty });
    let lines = world.transcript_lines();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["type"], "queue-operation");
    assert_eq!(lines[0]["operation"], "enqueue");
    assert_eq!(lines[0]["content"], "second-done please");
}

#[test]
fn a_picture_goes_in_as_a_path_and_is_counted_as_an_image() {
    let Some(fake) = fake_claude() else { return };
    let world = World::new(&fake);
    world.settings(&[("FAKE_CLAUDE_PICTURES", "1")]);
    let picture = world.path("shot.png");
    assert!(
        !picture.to_string_lossy().contains(' '),
        "picture paths must have no spaces: {}",
        picture.display()
    );
    let message = Message {
        text: "what colour is this, one word?".to_owned(),
        pictures: vec![picture],
        files: Vec::new(),
    };
    let outcome = messenger::send(&world.claude, &world.target(), &message, STYLE);
    assert_eq!(outcome, Outcome::Delivered { via: Via::Pty });
    let lines = world.transcript_lines();
    let parts = lines[0]["message"]["content"].as_array().expect("parts");
    assert_eq!(parts[0]["text"], "[Image #1]what colour is this, one word?");
    assert_eq!(parts[1]["type"], "image");
}

#[test]
fn line_breaks_arrive_inside_one_message() {
    let Some(fake) = fake_claude() else { return };
    let world = World::new(&fake);
    let text = "first line\nsecond line\n\nfourth, after a blank";
    let outcome = messenger::send(&world.claude, &world.target(), &message(text), STYLE);
    assert_eq!(outcome, Outcome::Delivered { via: Via::Pty });
    let lines = world.transcript_lines();
    assert_eq!(lines.len(), 1, "one message, not one per line");
    assert_eq!(lines[0]["message"]["content"], text);
}

#[test]
fn dropped_files_stay_in_the_text_as_written() {
    let Some(fake) = fake_claude() else { return };
    let world = World::new(&fake);
    let file = world.path("My Docs").join("notes v2.txt");
    let message = Message {
        text: "what is in this?".to_owned(),
        pictures: Vec::new(),
        files: vec![file.clone()],
    };
    let outcome = messenger::send(&world.claude, &world.target(), &message, STYLE);
    assert_eq!(outcome, Outcome::Delivered { via: Via::Pty });
    let lines = world.transcript_lines();
    let content = lines[0]["message"]["content"].as_str().expect("text");
    let expected = budd_core::delivery::escaped_path(&file.to_string_lossy(), STYLE);
    assert_eq!(content, format!("what is in this? {expected}"));
}

#[test]
fn a_client_without_a_prompt_fails_with_what_it_said() {
    let Some(fake) = fake_claude() else { return };
    let world = World::new(&fake);
    world.settings(&[("FAKE_CLAUDE_NO_PROMPT", "exit")]);
    let started = Instant::now();
    let outcome = messenger::send(&world.claude, &world.target(), &message("hello"), STYLE);
    assert_eq!(
        outcome,
        Outcome::Unconfirmed(
            "the session's prompt never appeared: claude said \"No session found matching abc12345\"".to_owned()
        )
    );
    assert!(
        started.elapsed() < messenger::PROMPT_TIMEOUT,
        "a client that exited is not waited for"
    );
    assert!(world.transcript_lines().is_empty());
}

#[test]
fn a_client_that_hangs_times_out_with_its_last_line() {
    let Some(fake) = fake_claude() else { return };
    let world = World::new(&fake);
    world.settings(&[("FAKE_CLAUDE_NO_PROMPT", "hang")]);
    let mut attach = Attach::spawn(&world.claude, "abc12345", DEFAULT_SIZE).expect("the fake starts");
    let started = Instant::now();
    let error = attach
        .wait_prompt(Duration::from_secs(2))
        .expect_err("no prompt is drawn");
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert!(!error.exited);
    assert_eq!(
        error.to_string(),
        "the session's prompt never appeared: claude said \"Connecting to abc12345...\""
    );
    attach.close();
    assert!(attach.exited());
}

#[test]
fn without_a_transcript_a_send_is_typed_but_unconfirmed() {
    let Some(fake) = fake_claude() else { return };
    let world = World::new(&fake);
    let target = Target {
        transcript: None,
        ..world.target()
    };
    let outcome = messenger::send(&world.claude, &target, &message("blind"), STYLE);
    assert_eq!(
        outcome,
        Outcome::Unconfirmed("no transcript to check against".to_owned())
    );
    // It was typed all the same: the fake recorded it where its own settings point.
    assert_eq!(world.transcript_lines()[0]["message"]["content"], "blind");
}

#[test]
fn a_message_that_never_shows_up_is_unconfirmed_after_the_timeout() {
    let Some(fake) = fake_claude() else { return };
    let world = World::new(&fake);
    // The fake writes somewhere else, so the watched transcript never gets the message.
    world.settings(&[(
        "FAKE_CLAUDE_TRANSCRIPT",
        &world.path("elsewhere.jsonl").display().to_string(),
    )]);
    let started = Instant::now();
    let outcome = messenger::send(&world.claude, &world.target(), &message("lost"), STYLE);
    assert_eq!(
        outcome,
        Outcome::Unconfirmed("it never showed up in the transcript".to_owned())
    );
    assert!(started.elapsed() >= messenger::CONFIRM_TIMEOUT);
}

#[test]
fn a_clear_counts_once_the_session_id_changes() {
    let Some(fake) = fake_claude() else { return };
    let world = World::new(&fake);
    let target = world.target();
    world.write_agents(&world.agents(), Some(4242), "idle", &target.session_id);
    let outcome = messenger::clear(&world.claude, &target);
    assert_eq!(outcome, Outcome::Delivered { via: Via::Pty });
    let listing: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(world.agents()).expect("the listing")).expect("JSON");
    let renewed = listing[0]["sessionId"].as_str().expect("a session id");
    assert_ne!(renewed, target.session_id);
    assert_eq!(renewed.len(), 36);
    assert!(
        world.transcript_lines().is_empty(),
        "a clear writes nothing to the old transcript"
    );
}

#[test]
fn a_sleeping_session_is_waited_for_until_it_is_listed_awake() {
    let Some(fake) = fake_claude() else { return };
    let world = World::new(&fake);
    let target = Target {
        asleep: true,
        ..world.target()
    };
    let later = world.path("agents-awake.json");
    world.write_agents(&world.agents(), None, "waiting", &target.session_id);
    world.write_agents(&later, Some(4242), "idle", &target.session_id);
    world.settings(&[
        ("FAKE_CLAUDE_AGENTS_AFTER", "2"),
        ("FAKE_CLAUDE_AGENTS_LATER", &later.display().to_string()),
    ]);
    let started = Instant::now();
    let outcome = messenger::send(&world.claude, &target, &message("good morning"), STYLE);
    assert_eq!(outcome, Outcome::Delivered { via: Via::Pty });
    let calls = fs::read_to_string(world.path("agents.json.calls")).expect("the call count");
    assert_eq!(calls, "3", "two polls saw it asleep, the third awake");
    // Three polls a second apart, then the settling time, before anything was typed.
    assert!(
        started.elapsed() >= Duration::from_millis(4500),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(world.transcript_lines()[0]["message"]["content"], "good morning");
}
