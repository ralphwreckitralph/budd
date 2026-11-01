//! What the engine asks of the machine: the `claude` binary, the processes, the transcripts, the clock.
//!
//! The engine never touches the OS itself; it goes through [`ClaudeHost`], so the whole state machine
//! runs against [`mock::MockHost`] in tests, with sessions, transcripts and a clock set by hand. The
//! real host, [`RealHost`], is `budd_claude` behind the trait.

use budd_claude::claude_cli::Claude;
use budd_claude::hookrun::transcript_io::{FsTranscriptStore, locate_transcript};
use budd_claude::procs;
use budd_claude::watch::{JobFile, JobInfo, modified};
use budd_core::account::AccountStatus;
use budd_core::crew::AgentRow;
use budd_core::history::TranscriptStore;
use budd_core::reply::Evidence;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// How a send or a clear ended: seen in the transcript, or typed but not confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The message showed up in the session's transcript.
    Delivered,
    /// It could not be confirmed, with why.
    Unconfirmed(String),
}

/// Where a message goes: a session by its short id, with what the messenger needs to confirm it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendTarget {
    /// The short id, what `claude attach` takes.
    pub short_id: String,
    /// The full session id, the conversation it has now.
    pub session_id: String,
    /// Its transcript, where the message is looked for.
    pub transcript: Option<PathBuf>,
    /// It was retired while it waited on the human, so `claude attach` wakes it first.
    pub asleep: bool,
}

/// Everything the engine needs from outside itself.
pub trait ClaudeHost: TranscriptStore + Send + Sync + 'static {
    /// The rows of `claude agents --json`; with `all`, the completed sessions too.
    fn agents(&self, all: bool) -> Result<Vec<AgentRow>, String>;
    /// Who the config folder is logged in as.
    fn auth_status(&self) -> Result<AccountStatus, String>;
    /// Stops the background session with `short_id`.
    fn stop(&self, short_id: &str) -> Result<(), String>;
    /// Starts `claude argv` in `cwd` as a background session; the lines it printed.
    fn start_bg(&self, cwd: &Path, argv: &[String]) -> Result<Vec<String>, String>;
    /// Sends `message` with `pictures` to `target` and waits for it to show in the transcript.
    fn send(&self, target: &SendTarget, message: &str, pictures: &[String]) -> Outcome;
    /// Clears `target`'s conversation, as `/clear` would.
    fn clear(&self, target: &SendTarget) -> Outcome;
    /// Whether the process `pid` is running.
    fn is_alive(&self, pid: u32) -> bool;
    /// Where the session's transcript is, or will be.
    fn locate_transcript(&self, session_id: &str, cwd: &str) -> PathBuf;
    /// When the file at `path` was last written, in seconds, or `None` when it is not there.
    fn file_modified(&self, path: &Path) -> Option<f64>;
    /// What the job runner keeps about the session with `short_id`.
    fn read_job(&self, short_id: &str) -> Option<JobInfo>;
    /// The evidence set of `ticket`, when the pipeline keeps one.
    fn read_evidence(&self, ticket: &str) -> Option<Evidence>;
    /// Seconds since the Unix epoch.
    fn now(&self) -> f64;
    /// The transcripts, for following and for history.
    fn transcripts(&self) -> &dyn TranscriptStore;
}

/// The machine as it is: `budd_claude` behind the trait.
pub struct RealHost {
    claude: Claude,
    store: FsTranscriptStore,
}

impl RealHost {
    /// A host running `claude` against `config_dir`.
    pub fn new(claude: Claude) -> RealHost {
        let store = FsTranscriptStore::new(claude.config_dir.clone());
        RealHost { claude, store }
    }

    /// The binary and config folder.
    pub fn claude(&self) -> &Claude {
        &self.claude
    }
}

impl TranscriptStore for RealHost {
    fn open(&self, path: &str) -> Option<Box<dyn budd_core::history::TranscriptSource + '_>> {
        self.store.open(path)
    }

    fn candidates(&self) -> Vec<budd_core::history::Candidate> {
        self.store.candidates()
    }
}

impl ClaudeHost for RealHost {
    fn agents(&self, all: bool) -> Result<Vec<AgentRow>, String> {
        self.claude
            .agents(all)
            .map(budd_claude::compat::agents_json::from_values)
            .map_err(|error| error.to_string())
    }

    fn auth_status(&self) -> Result<AccountStatus, String> {
        self.claude.auth_status().map_err(|error| error.to_string())
    }

    fn stop(&self, short_id: &str) -> Result<(), String> {
        self.claude.stop(short_id).map_err(|error| error.to_string())
    }

    fn start_bg(&self, cwd: &Path, argv: &[String]) -> Result<Vec<String>, String> {
        self.claude.start_bg(cwd, argv).map_err(|error| error.to_string())
    }

    fn send(&self, _target: &SendTarget, _message: &str, _pictures: &[String]) -> Outcome {
        // The pseudo-terminal messenger is being written beside this crate; it is wired in here at merge.
        Outcome::Unconfirmed("message delivery is wired at merge".to_owned())
    }

    fn clear(&self, _target: &SendTarget) -> Outcome {
        Outcome::Unconfirmed("message delivery is wired at merge".to_owned())
    }

    fn is_alive(&self, pid: u32) -> bool {
        procs::is_alive(pid)
    }

    fn locate_transcript(&self, session_id: &str, cwd: &str) -> PathBuf {
        locate_transcript(&self.claude.config_dir, session_id, cwd)
    }

    fn file_modified(&self, path: &Path) -> Option<f64> {
        modified(path)
    }

    fn read_job(&self, short_id: &str) -> Option<JobInfo> {
        let mut job = JobFile::new(&self.claude.config_dir, short_id);
        job.update();
        job.info().cloned()
    }

    fn read_evidence(&self, ticket: &str) -> Option<Evidence> {
        let manifest = Evidence::manifest(ticket, &self.claude.config_dir);
        let data = std::fs::read(&manifest).ok()?;
        let folder = manifest.parent()?;
        Evidence::decode(&data, folder, |path| path.is_file())
    }

    fn now(&self) -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }

    fn transcripts(&self) -> &dyn TranscriptStore {
        self
    }
}

/// A host held in memory, for tests: sessions, transcripts, jobs and a clock set by hand, and a record
/// of everything the engine asked it to do.
pub mod mock {
    use super::*;
    use budd_core::history::{Candidate, MemoryStore, TranscriptSource};
    use std::collections::{HashMap, HashSet};
    use std::sync::Mutex;

    /// One call the engine made that changed something.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Did {
        /// `stop(short_id)`.
        Stop(String),
        /// `start_bg(cwd, argv)`.
        Start(PathBuf, Vec<String>),
        /// `send(target.short_id, message, pictures)`.
        Send(String, String, Vec<String>),
        /// `clear(target.short_id)`.
        Clear(String),
    }

    /// What the mock holds.
    struct Inner {
        rows: Vec<AgentRow>,
        agents_error: Option<String>,
        auth: Option<Result<AccountStatus, String>>,
        dead: HashSet<u32>,
        store: MemoryStore,
        modified: HashMap<PathBuf, f64>,
        jobs: HashMap<String, JobInfo>,
        evidence: HashMap<String, Evidence>,
        now: f64,
        send_outcome: Outcome,
        stop_fails: bool,
        did: Vec<Did>,
    }

    /// The host of the tests.
    pub struct MockHost {
        config_dir: PathBuf,
        inner: Mutex<Inner>,
    }

    impl Default for MockHost {
        fn default() -> Self {
            MockHost::new()
        }
    }

    impl MockHost {
        /// An empty host whose clock reads 1000 and whose sends are delivered.
        pub fn new() -> MockHost {
            MockHost {
                config_dir: PathBuf::from("/c"),
                inner: Mutex::new(Inner {
                    rows: Vec::new(),
                    agents_error: None,
                    auth: None,
                    dead: HashSet::new(),
                    store: MemoryStore::new(),
                    modified: HashMap::new(),
                    jobs: HashMap::new(),
                    evidence: HashMap::new(),
                    now: 1000.0,
                    send_outcome: Outcome::Delivered,
                    stop_fails: false,
                    did: Vec::new(),
                }),
            }
        }

        fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
            let mut inner = self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            f(&mut inner)
        }

        /// The config folder the mock pretends to be under.
        pub fn config_dir(&self) -> &Path {
            &self.config_dir
        }

        /// Sets what `agents` lists.
        pub fn set_rows(&self, rows: Vec<AgentRow>) {
            self.with(|inner| inner.rows = rows);
        }

        /// Makes `agents` fail with `error`, or succeed again with `None`.
        pub fn set_agents_error(&self, error: Option<&str>) {
            self.with(|inner| inner.agents_error = error.map(str::to_owned));
        }

        /// Sets what `auth_status` says.
        pub fn set_auth(&self, auth: Result<AccountStatus, String>) {
            self.with(|inner| inner.auth = Some(auth));
        }

        /// Marks `pid` dead.
        pub fn kill(&self, pid: u32) {
            self.with(|inner| {
                inner.dead.insert(pid);
            });
        }

        /// Moves the clock to `now`.
        pub fn set_now(&self, now: f64) {
            self.with(|inner| inner.now = now);
        }

        /// Moves the clock forward by `seconds`.
        pub fn advance(&self, seconds: f64) {
            self.with(|inner| inner.now += seconds);
        }

        /// Writes a transcript at `path`, created at `created_at` and last modified now.
        pub fn write_transcript(&self, path: &Path, created_at: i64, lines: &[String]) {
            self.with(|inner| {
                let text = format!("{}\n", lines.join("\n"));
                inner
                    .store
                    .write(&path.to_string_lossy(), created_at, text.into_bytes());
                inner.modified.insert(path.to_path_buf(), inner.now);
            });
        }

        /// Appends `lines` to the transcript at `path`, which becomes modified now, or a moment after
        /// its last write when the clock has not moved, as a file system's clock would show.
        pub fn append_transcript(&self, path: &Path, lines: &[String]) {
            self.with(|inner| {
                let text = format!("{}\n", lines.join("\n"));
                inner.store.append(&path.to_string_lossy(), text.as_bytes());
                let previous = inner.modified.get(path).copied().unwrap_or(0.0);
                let at = if inner.now > previous {
                    inner.now
                } else {
                    previous + 0.001
                };
                inner.modified.insert(path.to_path_buf(), at);
            });
        }

        /// Sets the job file of the session with `short_id`.
        pub fn set_job(&self, short_id: &str, job: Option<JobInfo>) {
            self.with(|inner| match job {
                Some(job) => {
                    inner.jobs.insert(short_id.to_owned(), job);
                }
                None => {
                    inner.jobs.remove(short_id);
                }
            });
        }

        /// Sets the evidence set of `ticket`.
        pub fn set_evidence(&self, ticket: &str, evidence: Option<Evidence>) {
            self.with(|inner| match evidence {
                Some(evidence) => {
                    inner.evidence.insert(ticket.to_owned(), evidence);
                }
                None => {
                    inner.evidence.remove(ticket);
                }
            });
        }

        /// Makes every send and clear end with `outcome`.
        pub fn set_send_outcome(&self, outcome: Outcome) {
            self.with(|inner| inner.send_outcome = outcome);
        }

        /// Makes every stop fail.
        pub fn set_stop_fails(&self, fails: bool) {
            self.with(|inner| inner.stop_fails = fails);
        }

        /// Everything the engine did, in order, and forgets it.
        pub fn take_did(&self) -> Vec<Did> {
            self.with(|inner| std::mem::take(&mut inner.did))
        }

        /// Everything the engine did so far, in order.
        pub fn did(&self) -> Vec<Did> {
            self.with(|inner| inner.did.clone())
        }
    }

    impl TranscriptStore for MockHost {
        fn open(&self, path: &str) -> Option<Box<dyn TranscriptSource + '_>> {
            self.with(|inner| {
                let source = inner.store.open(path)?;
                let bytes = source.read(0..source.len());
                let identity = source.identity();
                Some(Box::new(budd_core::history::MemorySource::new(identity, bytes)) as Box<dyn TranscriptSource + '_>)
            })
        }

        fn candidates(&self) -> Vec<Candidate> {
            self.with(|inner| inner.store.candidates())
        }
    }

    impl ClaudeHost for MockHost {
        fn agents(&self, _all: bool) -> Result<Vec<AgentRow>, String> {
            self.with(|inner| match &inner.agents_error {
                Some(error) => Err(error.clone()),
                None => Ok(inner.rows.clone()),
            })
        }

        fn auth_status(&self) -> Result<AccountStatus, String> {
            self.with(|inner| {
                inner
                    .auth
                    .clone()
                    .unwrap_or_else(|| Err("no auth status set".to_owned()))
            })
        }

        fn stop(&self, short_id: &str) -> Result<(), String> {
            self.with(|inner| {
                inner.did.push(Did::Stop(short_id.to_owned()));
                if inner.stop_fails {
                    Err("No such session".to_owned())
                } else {
                    inner.rows.retain(|row| row.id.as_deref() != Some(short_id));
                    Ok(())
                }
            })
        }

        fn start_bg(&self, cwd: &Path, argv: &[String]) -> Result<Vec<String>, String> {
            self.with(|inner| {
                inner.did.push(Did::Start(cwd.to_path_buf(), argv.to_vec()));
                Ok(vec!["Started background session".to_owned()])
            })
        }

        fn send(&self, target: &SendTarget, message: &str, pictures: &[String]) -> Outcome {
            self.with(|inner| {
                inner.did.push(Did::Send(
                    target.short_id.clone(),
                    message.to_owned(),
                    pictures.to_vec(),
                ));
                inner.send_outcome.clone()
            })
        }

        fn clear(&self, target: &SendTarget) -> Outcome {
            self.with(|inner| {
                inner.did.push(Did::Clear(target.short_id.clone()));
                inner.send_outcome.clone()
            })
        }

        fn is_alive(&self, pid: u32) -> bool {
            self.with(|inner| !inner.dead.contains(&pid))
        }

        fn locate_transcript(&self, session_id: &str, cwd: &str) -> PathBuf {
            self.config_dir
                .join("projects")
                .join(budd_core::location::project_slug(cwd))
                .join(format!("{session_id}.jsonl"))
        }

        fn file_modified(&self, path: &Path) -> Option<f64> {
            self.with(|inner| inner.modified.get(path).copied())
        }

        fn read_job(&self, short_id: &str) -> Option<JobInfo> {
            self.with(|inner| inner.jobs.get(short_id).cloned())
        }

        fn read_evidence(&self, ticket: &str) -> Option<Evidence> {
            self.with(|inner| inner.evidence.get(ticket).cloned())
        }

        fn now(&self) -> f64 {
            self.with(|inner| inner.now)
        }

        fn transcripts(&self) -> &dyn TranscriptStore {
            self
        }
    }
}
