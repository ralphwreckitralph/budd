//! Following the state files, the sessions listing and the transcripts as they change.
//!
//! Four watchers, each owning one source and saying when it changed, so the engine reads only what
//! moved: [`SessionPoller`] lists `claude agents --json` on a cadence that quickens while an answer is
//! awaited or the assistant is gone; [`StateFileWatcher`] follows the hook's state and reply files with
//! file-system events and a polling fallback; [`TranscriptFile`] follows one transcript on disk through
//! `budd_core::transcript::TranscriptFollower`; and [`JobFile`] reads what Claude Code's job runner
//! keeps about a session. The cadences are the ones `SessionWatcher.swift` used.

use crate::compat::agents_json;
use crate::hookrun::transcript_io::FsTranscriptSource;
use crate::state_files::Dir;
use budd_core::crew::{AgentRow, JobState};
use budd_core::presence::SessionReport;
use budd_core::transcript::{TranscriptFollower, TranscriptSummary};
use notify::{RecursiveMode, Watcher};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Notify, mpsc};

/// How often the sessions are listed when nothing is pressing.
pub const CALM_INTERVAL: Duration = Duration::from_secs(8);
/// How often while a sent message awaits its answer.
pub const URGENT_INTERVAL: Duration = Duration::from_secs(4);
/// How often while the assistant is unbound or its process is gone, as after a keep-alive refresh.
pub const LOST_INTERVAL: Duration = Duration::from_secs(3);
/// How often the state files are looked at when no file-system event arrives.
pub const STATE_POLL: Duration = Duration::from_secs(2);

/// What quickens the listing: an awaited answer, or a lost assistant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cadence {
    /// A sent message awaits its answer.
    pub urgent: bool,
    /// The assistant is unbound or its process is dead.
    pub lost: bool,
}

impl Cadence {
    /// How long to wait before the next listing.
    pub fn interval(self) -> Duration {
        if self.lost {
            LOST_INTERVAL
        } else if self.urgent {
            URGENT_INTERVAL
        } else {
            CALM_INTERVAL
        }
    }
}

/// Lists the sessions on its own cadence; the engine asks for the next listing and can hurry it.
///
/// The listing itself runs on a blocking thread, as `claude agents` is a process, and whatever lists
/// is given as a function so tests can list from memory.
pub struct SessionPoller {
    list: Arc<dyn Fn() -> Result<Vec<AgentRow>, String> + Send + Sync>,
    poke: Arc<Notify>,
    cadence: Cadence,
    first: bool,
    /// Divides every interval, so tests wait milliseconds rather than seconds.
    #[cfg(test)]
    scale: u32,
}

impl SessionPoller {
    /// A poller that lists with `list`.
    pub fn new(list: impl Fn() -> Result<Vec<AgentRow>, String> + Send + Sync + 'static) -> SessionPoller {
        SessionPoller {
            list: Arc::new(list),
            poke: Arc::new(Notify::new()),
            cadence: Cadence::default(),
            first: true,
            #[cfg(test)]
            scale: 1,
        }
    }

    /// How long the next wait is.
    fn wait(&self) -> Duration {
        let interval = self.cadence.interval();
        #[cfg(test)]
        {
            interval / self.scale
        }
        #[cfg(not(test))]
        {
            interval
        }
    }

    /// A poller over the real `claude agents --json`.
    pub fn for_claude(claude: crate::claude_cli::Claude) -> SessionPoller {
        SessionPoller::new(move || {
            claude
                .agents(false)
                .map(agents_json::from_values)
                .map_err(|error| error.to_string())
        })
    }

    /// A handle that makes the poller list now rather than at the end of its interval.
    pub fn poker(&self) -> Poker {
        Poker(Arc::clone(&self.poke))
    }

    /// Lists now rather than at the end of the interval.
    pub fn poke(&self) {
        self.poke.notify_one();
    }

    /// Sets what quickens the next intervals.
    pub fn set_cadence(&mut self, cadence: Cadence) {
        self.cadence = cadence;
    }

    /// The cadence in force.
    pub fn cadence(&self) -> Cadence {
        self.cadence
    }

    /// Waits out the interval (or a poke), then lists. The first call lists at once.
    pub async fn next(&mut self) -> Result<Vec<AgentRow>, String> {
        if self.first {
            self.first = false;
        } else {
            let sleep = tokio::time::sleep(self.wait());
            tokio::select! {
                () = sleep => {}
                () = self.poke.notified() => {}
            }
        }
        let list = Arc::clone(&self.list);
        match tokio::task::spawn_blocking(move || list()).await {
            Ok(result) => result,
            Err(error) => Err(format!("the listing panicked: {error}")),
        }
    }
}

/// A handle that hurries a [`SessionPoller`] from elsewhere.
#[derive(Debug, Clone)]
pub struct Poker(Arc<Notify>);

impl Poker {
    /// Lists now.
    pub fn poke(&self) {
        self.0.notify_one();
    }
}

/// Seconds since the Unix epoch of a file time.
fn seconds(time: SystemTime) -> f64 {
    time.duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// The modification time of `path` in seconds, or `None` when the file is not there.
pub fn modified(path: &Path) -> Option<f64> {
    fs::metadata(path).ok()?.modified().ok().map(seconds)
}

/// What one of a session's files holds now: its text and when it was written, or nothing when it is gone.
#[derive(Debug, Clone, PartialEq)]
pub struct FileRead {
    /// The file's text.
    pub text: String,
    /// When it was written, seconds since the Unix epoch.
    pub modified: f64,
}

/// Follows the folder of state files: `<session id>.json` and `<session id>.reply.md`, as the hook
/// writes them. File-system events wake it, and it looks anyway every [`STATE_POLL`], since an atomic
/// rename can slip past a watcher; either way it reports only files whose modification time moved.
pub struct StateFileWatcher {
    dir: Dir,
    seen: HashMap<PathBuf, Option<f64>>,
    events: mpsc::UnboundedReceiver<()>,
    poke: Arc<Notify>,
    /// Kept alive for the folder to stay watched.
    _watcher: Option<Box<dyn Watcher + Send>>,
    /// Whether file-system events are coming, for the log.
    pub native: bool,
}

impl StateFileWatcher {
    /// Watches `dir`, creating the folder so there is something to watch. Falls back to polling alone
    /// when the platform's watcher cannot be set up.
    pub fn new(dir: Dir) -> StateFileWatcher {
        let _ = fs::create_dir_all(dir.path());
        let (sender, events) = mpsc::unbounded_channel();
        let handler = move |_event: Result<notify::Event, notify::Error>| {
            let _ = sender.send(());
        };
        let mut native = false;
        let watcher: Option<Box<dyn Watcher + Send>> = match notify::recommended_watcher(handler) {
            Ok(mut watcher) => {
                if watcher.watch(dir.path(), RecursiveMode::NonRecursive).is_ok() {
                    native = true;
                    Some(Box::new(watcher))
                } else {
                    None
                }
            }
            Err(error) => {
                tracing::warn!(%error, "no file-system watcher; polling the state files instead");
                None
            }
        };
        StateFileWatcher {
            dir,
            seen: HashMap::new(),
            events,
            poke: Arc::new(Notify::new()),
            _watcher: watcher,
            native,
        }
    }

    /// The folder watched.
    pub fn dir(&self) -> &Dir {
        &self.dir
    }

    /// A handle that makes the watcher look now, as after a `hook.poke`.
    pub fn poker(&self) -> Poker {
        Poker(Arc::clone(&self.poke))
    }

    /// Waits for an event, a poke or the poll interval, then says which sessions' files moved since
    /// the last look: each session id once, in no particular order. The first call reports every file.
    pub async fn changed(&mut self) -> Vec<String> {
        loop {
            let sleep = tokio::time::sleep(STATE_POLL);
            tokio::select! {
                () = sleep => {}
                () = self.poke.notified() => {}
                _ = self.events.recv() => {
                    // Events come in bursts; take the rest of the burst before looking.
                    while self.events.try_recv().is_ok() {}
                }
            }
            let changed = self.scan();
            if !changed.is_empty() {
                return changed;
            }
        }
    }

    /// Looks now and says which sessions' files moved since the last look.
    pub fn scan(&mut self) -> Vec<String> {
        let mut now: HashMap<PathBuf, Option<f64>> = HashMap::new();
        if let Ok(entries) = fs::read_dir(self.dir.path()) {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if name.starts_with('.') || session_of(name).is_none() {
                    continue;
                }
                let at = entry.metadata().ok().and_then(|m| m.modified().ok()).map(seconds);
                now.insert(path, at);
            }
        }
        let mut changed: Vec<String> = Vec::new();
        let mut note = |path: &Path| {
            if let Some(session) = path.file_name().and_then(|n| n.to_str()).and_then(session_of)
                && !changed.iter().any(|s| s == session)
            {
                changed.push(session.to_owned());
            }
        };
        for (path, at) in &now {
            if self.seen.get(path) != Some(at) {
                note(path);
            }
        }
        for path in self.seen.keys() {
            if !now.contains_key(path) {
                note(path);
            }
        }
        self.seen = now;
        changed
    }

    /// The session's state file as it is now, or `None` when it is gone or not a report.
    pub fn report(&self, session_id: &str) -> Option<(SessionReport, f64)> {
        let path = self.dir.report_path(session_id);
        let modified = modified(&path)?;
        let report = self.dir.read_report(session_id)?;
        Some((report, modified))
    }

    /// The session's reply file as it is now, or `None` when it is gone or empty.
    pub fn reply(&self, session_id: &str) -> Option<FileRead> {
        let path = self.dir.reply_path(session_id);
        let modified = modified(&path)?;
        let text = fs::read_to_string(&path).ok()?;
        if text.is_empty() {
            return None;
        }
        Some(FileRead { text, modified })
    }
}

/// The session a state or reply file belongs to, from its name.
fn session_of(name: &str) -> Option<&str> {
    name.strip_suffix(".reply.md")
        .or_else(|| name.strip_suffix(".json"))
        .filter(|id| !id.is_empty())
}

/// One transcript on disk, summed up as it grows. It is re-read only when the file's size or
/// modification time moved; a file replaced at the same path, or cut back, is read afresh, which the
/// follower underneath notices by its identity and size.
#[derive(Debug, Clone)]
pub struct TranscriptFile {
    path: PathBuf,
    follower: TranscriptFollower,
    stamp: Option<(f64, u64)>,
}

impl TranscriptFile {
    /// A follower of the transcript at `path`, which need not exist yet.
    pub fn new(path: impl Into<PathBuf>) -> TranscriptFile {
        TranscriptFile {
            path: path.into(),
            follower: TranscriptFollower::new(),
            stamp: None,
        }
    }

    /// The transcript's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the transcript says so far.
    pub fn summary(&self) -> &TranscriptSummary {
        self.follower.summary()
    }

    /// When the transcript was last written, as of the last update.
    pub fn modified(&self) -> Option<f64> {
        self.stamp.map(|(at, _)| at)
    }

    /// Reads what the transcript gained, when its size or time moved; true when it was read.
    pub fn update(&mut self) -> bool {
        let Ok(metadata) = fs::metadata(&self.path) else {
            return false;
        };
        let stamp = (metadata.modified().ok().map(seconds).unwrap_or(0.0), metadata.len());
        if self.stamp == Some(stamp) {
            return false;
        }
        self.stamp = Some(stamp);
        self.follower.update(&FsTranscriptSource::new(&self.path));
        true
    }
}

/// What Claude Code's job runner keeps about a session in `<config>/jobs/<short id>/state.json`.
///
/// This is an unstable surface: the file belongs to Claude Code, is not documented, and may change
/// or go. It is read with low trust, every field optional, and nothing Budd does depends on it alone:
/// it refines a desk's line and tells a blocked job from an idle one when `claude agents` does not.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JobInfo {
    /// The state, detail and intent `budd_core` knows.
    pub state: JobState,
    /// What the job needs from the human, when the runner says.
    pub needs: Option<String>,
}

/// The job file of one session, re-read when its modification time moves.
#[derive(Debug, Clone)]
pub struct JobFile {
    path: PathBuf,
    modified: Option<f64>,
    info: Option<JobInfo>,
}

impl JobFile {
    /// The job file of the session with `short_id` under `config_dir`.
    pub fn new(config_dir: &Path, short_id: &str) -> JobFile {
        JobFile {
            path: config_dir.join("jobs").join(short_id).join("state.json"),
            modified: None,
            info: None,
        }
    }

    /// The file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the file said when last read, or `None` without a file or when it is not JSON.
    pub fn info(&self) -> Option<&JobInfo> {
        self.info.as_ref()
    }

    /// Re-reads the file when its time moved; true when it was read.
    pub fn update(&mut self) -> bool {
        let modified = modified(&self.path);
        if modified == self.modified && (modified.is_some() || self.info.is_none()) {
            return false;
        }
        self.modified = modified;
        self.info = modified
            .and_then(|_| fs::read_to_string(&self.path).ok())
            .and_then(|text| {
                let state = JobState::decode(&text)?;
                let needs = serde_json::from_str::<serde_json::Value>(&text)
                    .ok()
                    .and_then(|value| value.get("needs").and_then(|n| n.as_str()).map(str::to_owned));
                Some(JobInfo { state, needs })
            });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn the_cadence_quickens_when_an_answer_is_awaited_or_the_assistant_is_gone() {
        assert_eq!(Cadence::default().interval(), CALM_INTERVAL);
        assert_eq!(
            Cadence {
                urgent: true,
                lost: false
            }
            .interval(),
            URGENT_INTERVAL
        );
        assert_eq!(
            Cadence {
                urgent: true,
                lost: true
            }
            .interval(),
            LOST_INTERVAL
        );
    }

    #[tokio::test]
    async fn the_poller_lists_at_once_then_on_its_interval_and_when_poked() {
        let count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        let mut poller = SessionPoller::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(agents_json::parse(include_str!("../fixtures/synthetic/agents.json")))
        });
        // Intervals of 8, 4 and 3 ms instead of seconds.
        poller.scale = 1000;
        let start = std::time::Instant::now();
        let rows = poller.next().await.unwrap();
        assert_eq!(rows.len(), 5);
        assert!(
            start.elapsed() < CALM_INTERVAL / 1000,
            "the first listing waits for nothing"
        );
        let before = std::time::Instant::now();
        poller.next().await.unwrap();
        assert!(before.elapsed() >= CALM_INTERVAL / 1000);
        poller.set_cadence(Cadence {
            urgent: true,
            lost: false,
        });
        assert_eq!(poller.wait(), URGENT_INTERVAL / 1000);
        poller.set_cadence(Cadence {
            urgent: false,
            lost: true,
        });
        assert_eq!(poller.wait(), LOST_INTERVAL / 1000);
        // A poke lists now, without waiting out the interval.
        poller.scale = 1;
        poller.set_cadence(Cadence::default());
        let poker = poller.poker();
        poker.poke();
        let before = std::time::Instant::now();
        poller.next().await.unwrap();
        assert!(before.elapsed() < Duration::from_secs(1));
        poller.poke();
        poller.next().await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 4);
        // A failed listing is an error, not a panic.
        let mut failing = SessionPoller::new(|| Err("not logged in".to_owned()));
        assert_eq!(failing.next().await, Err("not logged in".to_owned()));
    }

    fn report(session_id: &str, state: &str) -> SessionReport {
        let mut report = SessionReport::new(state, 1.0);
        report.session_id = Some(session_id.to_owned());
        report
    }

    #[tokio::test]
    async fn the_state_file_watcher_reports_each_file_that_moved() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Dir(temp.path().join("sessions"));
        let mut watcher = StateFileWatcher::new(dir.clone());
        assert!(watcher.scan().is_empty());
        dir.write_report(&report("s1", "working")).unwrap();
        dir.write_reply("s1", "hello").unwrap();
        dir.write_report(&report("s2", "idle")).unwrap();
        fs::write(dir.path().join(".tmp-x"), "x").unwrap();
        fs::write(dir.path().join("notes.txt"), "x").unwrap();
        let mut changed = watcher.changed().await;
        changed.sort();
        assert_eq!(changed, vec!["s1", "s2"]);
        assert!(watcher.scan().is_empty());
        let (read, at) = watcher.report("s1").unwrap();
        assert_eq!(read.state, "working");
        assert!(at > 0.0);
        assert_eq!(watcher.reply("s1").unwrap().text, "hello");
        assert_eq!(watcher.reply("s2"), None);
        assert_eq!(watcher.report("missing"), None);
        // A rewrite moves the time (give the clock a moment), and a removal counts too.
        tokio::time::sleep(Duration::from_millis(30)).await;
        dir.write_reply("s2", "later").unwrap();
        fs::remove_file(dir.report_path("s1")).unwrap();
        let mut changed = watcher.changed().await;
        changed.sort();
        assert_eq!(changed, vec!["s1", "s2"]);
        assert_eq!(watcher.report("s1"), None);
        assert_eq!(watcher.reply("s2").unwrap().text, "later");
        // A poke looks at once, finding nothing, and keeps waiting.
        let poker = watcher.poker();
        poker.poke();
        let waited = tokio::time::timeout(Duration::from_millis(300), watcher.changed()).await;
        assert!(waited.is_err(), "nothing changed, so nothing is reported");
        assert_eq!(session_of("abc.reply.md"), Some("abc"));
        assert_eq!(session_of("abc.json"), Some("abc"));
        assert_eq!(session_of(".json"), None);
        assert_eq!(session_of("notes.txt"), None);
    }

    fn user(text: &str) -> String {
        format!(r#"{{"type":"user","message":{{"content":"{text}"}}}}"#)
    }

    fn said(text: &str, id: &str) -> String {
        format!(r#"{{"type":"assistant","message":{{"id":"{id}","content":[{{"type":"text","text":"{text}"}}]}}}}"#)
    }

    #[test]
    fn the_transcript_file_reads_only_when_the_file_moved_and_restarts_when_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("t.jsonl");
        let mut transcript = TranscriptFile::new(&path);
        assert!(!transcript.update(), "no file yet");
        assert_eq!(transcript.modified(), None);
        fs::write(&path, format!("{}\n{}\n", user("first"), said("one", "m1"))).unwrap();
        assert!(transcript.update());
        assert!(!transcript.update(), "unchanged");
        assert_eq!(transcript.summary().prompt.as_deref(), Some("first"));
        assert_eq!(transcript.summary().reply.as_deref(), Some("one"));
        assert!(transcript.modified().is_some());
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{}", said("two", "m2")).unwrap();
        drop(file);
        assert!(transcript.update());
        assert_eq!(transcript.summary().reply.as_deref(), Some("two"));
        // Cut back: read afresh.
        fs::write(&path, format!("{}\n", user("fresh start"))).unwrap();
        assert!(transcript.update());
        assert_eq!(transcript.summary().prompt.as_deref(), Some("fresh start"));
        assert_eq!(transcript.summary().reply, None);
        // Replaced by another file of the same size and a new inode: still read afresh.
        let other = temp.path().join("other.jsonl");
        fs::write(&other, format!("{}\n", user("other start"))).unwrap();
        fs::rename(&other, &path).unwrap();
        let moved = fs::metadata(&path).unwrap().modified().unwrap() + Duration::from_secs(2);
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(moved)
            .unwrap();
        assert!(transcript.update());
        assert_eq!(transcript.summary().prompt.as_deref(), Some("other start"));
        assert_eq!(transcript.path(), path);
    }

    #[test]
    fn the_job_file_is_read_on_change_and_read_with_low_trust() {
        let temp = tempfile::tempdir().unwrap();
        let mut job = JobFile::new(temp.path(), "f754f44d");
        assert_eq!(job.path(), temp.path().join("jobs").join("f754f44d").join("state.json"));
        assert!(!job.update());
        assert_eq!(job.info(), None);
        fs::create_dir_all(job.path().parent().unwrap()).unwrap();
        fs::write(
            job.path(),
            r#"{"state":"blocked","detail":"needs login","intent":"/lets-do-it CASBO-1","needs":"login","extra":1}"#,
        )
        .unwrap();
        assert!(job.update());
        assert!(!job.update());
        let info = job.info().unwrap();
        assert_eq!(info.state.state.as_deref(), Some("blocked"));
        assert_eq!(info.state.intent.as_deref(), Some("/lets-do-it CASBO-1"));
        assert_eq!(info.needs.as_deref(), Some("login"));
        // Garbage reads as nothing, and the file going away is a change too.
        let later = fs::metadata(job.path()).unwrap().modified().unwrap() + Duration::from_secs(2);
        fs::write(job.path(), "not json").unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(job.path())
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert!(job.update());
        assert_eq!(job.info(), None);
        fs::write(job.path(), r#"{"state":"done"}"#).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(job.path())
            .unwrap()
            .set_modified(later + Duration::from_secs(2))
            .unwrap();
        assert!(job.update());
        assert_eq!(job.info().unwrap().state.state.as_deref(), Some("done"));
        fs::remove_file(job.path()).unwrap();
        assert!(job.update());
        assert_eq!(job.info(), None);
        assert!(!job.update());
    }

    #[test]
    fn modified_is_seconds_or_nothing() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(modified(&temp.path().join("none")), None);
        let path = temp.path().join("a");
        fs::write(&path, "x").unwrap();
        assert!(modified(&path).unwrap() > 1.0e9);
        // The listing function may hold state behind a mutex, as a fake does.
        let rows = Mutex::new(vec![AgentRow::default()]);
        let poller = SessionPoller::new(move || Ok(rows.lock().map(|r| r.clone()).unwrap_or_default()));
        assert_eq!(poller.cadence(), Cadence::default());
    }
}
