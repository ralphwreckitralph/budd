//! `budd daemon`: one instance per home, serving the endpoint, polling, ticking and keeping the
//! assistant alive, with the engine in the middle.
//!
//! The daemon owns `<home>/run/daemon.lock` (its pid; stale when that process is gone) and writes
//! `<home>/run/daemon.json` with its pid, version and endpoint for clients to find it. Everything it
//! does runs on tokio: the connection tasks, the sessions poller, the state file watcher, a one second
//! tick, the keep-alive every five minutes, and a blocking task for each effect the engine asks for.

use crate::ClientId;
use crate::config::Config;
use crate::engine::{Completion, Effect, Engine};
use crate::host::{ClaudeHost, RealHost};
use crate::server::{Clients, Inbound, Server, serve_connection};
use budd_claude::claude_cli::{Claude, locate};
use budd_claude::doctor;
use budd_claude::ipc::Endpoint;
use budd_claude::paths::{BuddHome, ensure_private_dir};
use budd_claude::procs;
use budd_claude::state_files::Dir;
use budd_claude::watch::{SessionPoller, StateFileWatcher};
use budd_core::crew::AgentRow;
use budd_core::presence::SessionReport;
use serde_json::json;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, watch};

/// How often the engine ticks.
pub const TICK: Duration = Duration::from_secs(1);
/// How often the keep-alive runs.
pub const KEEPALIVE: Duration = Duration::from_secs(5 * 60);
/// How long after start the keep-alive first runs, so the first listing is in.
pub const KEEPALIVE_FIRST: Duration = Duration::from_secs(20);
/// How often the account is looked up again.
pub const ACCOUNT: Duration = Duration::from_secs(10 * 60);
/// How often `config.json` is looked at for a change.
pub const CONFIG_POLL: Duration = Duration::from_secs(5);

/// What `budd daemon` was started with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Options {
    /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
    pub home: Option<PathBuf>,
    /// The `CLAUDE_CONFIG_DIR` of Budd's account, instead of `<home>/claude`.
    pub config_dir: Option<PathBuf>,
    /// The assistant session's name, instead of the config file's or "assistant".
    pub assistant_name: Option<String>,
    /// The `claude` binary, instead of the one found.
    pub claude: Option<PathBuf>,
}

/// Why the daemon could not run.
#[derive(Debug, Error)]
pub enum DaemonError {
    /// No home folder could be placed.
    #[error("{0}")]
    NoHome(#[from] budd_claude::paths::NoHome),
    /// Another daemon holds the lock.
    #[error("another budd daemon is running (pid {0})")]
    AlreadyRunning(u32),
    /// Something on the file system or the endpoint failed.
    #[error("{what}: {source}")]
    Io {
        /// What was being done.
        what: String,
        /// What went wrong.
        #[source]
        source: io::Error,
    },
}

fn io_error(what: &str) -> impl FnOnce(io::Error) -> DaemonError + '_ {
    move |source| DaemonError::Io {
        what: what.to_owned(),
        source,
    }
}

/// The single-instance lock: `<run>/daemon.lock` holding the owner's pid, removed when dropped.
#[derive(Debug)]
pub struct Lock {
    path: PathBuf,
}

impl Lock {
    /// Takes the lock in `run_dir`, replacing one whose process is gone; refuses one whose process lives.
    pub fn acquire(run_dir: &Path) -> Result<Lock, DaemonError> {
        Self::acquire_with(run_dir, procs::is_alive)
    }

    /// [`Lock::acquire`] with `alive` deciding whether a pid still runs.
    pub fn acquire_with(run_dir: &Path, alive: impl Fn(u32) -> bool) -> Result<Lock, DaemonError> {
        ensure_private_dir(run_dir).map_err(io_error("creating the run folder"))?;
        let path = run_dir.join("daemon.lock");
        for _ in 0..2 {
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}", std::process::id());
                    return Ok(Lock { path });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let holder: Option<u32> = fs::read_to_string(&path).ok().and_then(|text| text.trim().parse().ok());
                    match holder {
                        Some(pid) if pid != std::process::id() && alive(pid) => {
                            return Err(DaemonError::AlreadyRunning(pid));
                        }
                        _ => {
                            // Stale: its process is gone, or the file is not a pid.
                            fs::remove_file(&path).map_err(io_error("removing a stale lock"))?;
                        }
                    }
                }
                Err(error) => return Err(io_error("taking the lock")(error)),
            }
        }
        Err(DaemonError::Io {
            what: "taking the lock".to_owned(),
            source: io::Error::new(io::ErrorKind::AlreadyExists, "the lock keeps reappearing"),
        })
    }

    /// The lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// `<run>/daemon.json`: where the daemon is, for clients. Removed when dropped.
#[derive(Debug)]
pub struct Advert {
    path: PathBuf,
}

impl Advert {
    /// Writes the file.
    pub fn write(run_dir: &Path, version: &str, endpoint: &Endpoint) -> io::Result<Advert> {
        let path = run_dir.join("daemon.json");
        let text = json!({
            "pid": std::process::id(),
            "version": version,
            "endpoint": endpoint.to_string(),
        })
        .to_string();
        fs::write(&path, text)?;
        Ok(Advert { path })
    }

    /// The file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Advert {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// What reaches the engine's task.
// A state file is large and a tick is nothing; the channel carries few of either, so a Box is not worth it.
#[allow(clippy::large_enum_variant)]
enum Msg {
    Inbound(Inbound),
    Sessions(Result<Vec<AgentRow>, String>),
    StateFile(String, Option<SessionReport>, Option<(String, f64)>),
    Tick,
    Keepalive,
    Account(Result<budd_core::account::AccountStatus, String>),
    Config(Result<budd_claude::config::Config, String>),
    Completion(Completion),
    /// The process was told to stop (SIGTERM, SIGINT or Ctrl-C), so the lock and the advert go cleanly.
    Signal(&'static str),
}

/// Runs the daemon until a client asks it to shut down. Blocks the calling task for its whole life.
pub async fn run(options: Options) -> Result<(), DaemonError> {
    let home = match &options.home {
        Some(home) => BuddHome::at(home),
        None => BuddHome::resolve()?,
    };
    let _lock = Lock::acquire(&home.run_dir())?;
    let config_dir = options.config_dir.clone().unwrap_or_else(|| home.claude_dir());
    let exe = locate(options.claude.as_deref());
    let claude_exe = exe.clone().unwrap_or_else(|| {
        tracing::warn!("the claude binary was not found; sessions cannot be listed until it is");
        PathBuf::from(budd_claude::claude_cli::BINARY)
    });
    let mut config = Config::new(home.root(), &config_dir).with_file(&home.config_path());
    config.claude = exe;
    if let Some(name) = &options.assistant_name {
        config.fix_assistant_name(name.clone());
    }
    let claude = Claude::new(claude_exe, config_dir.clone());
    let doctor = doctor::Context {
        home: BuddHome::at(home.root()),
        exe: std::env::current_exe()
            .ok()
            .and_then(|exe| std::path::absolute(&exe).ok())
            .unwrap_or_else(|| PathBuf::from("budd")),
        claude: config
            .claude
            .as_ref()
            .map(|exe| Claude::new(exe.clone(), config_dir.clone())),
        version: config.version.clone(),
    };
    let host: Arc<dyn ClaudeHost> = Arc::new(RealHost::new(claude.clone()).with_doctor(doctor));
    let endpoint = home.endpoint();
    let mut server = Server::bind(&endpoint).map_err(io_error("binding the endpoint"))?;
    let _advert =
        Advert::write(&home.run_dir(), &config.version, &endpoint).map_err(io_error("writing daemon.json"))?;
    tracing::info!(endpoint = %endpoint, home = %home.root().display(), "budd daemon up");

    let mut engine = Engine::new(Arc::clone(&host), config);
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();

    // Connections.
    let (to_engine, mut inbound) = mpsc::unbounded_channel::<Inbound>();
    tokio::spawn(async move {
        let mut next = 1u64;
        loop {
            match server.accept().await {
                Ok(stream) => {
                    let client = ClientId(next);
                    next += 1;
                    tokio::spawn(serve_connection(stream, client, to_engine.clone()));
                }
                Err(error) => {
                    tracing::warn!(%error, "accepting a connection failed");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    });
    let forward = tx.clone();
    tokio::spawn(async move {
        while let Some(message) = inbound.recv().await {
            if forward.send(Msg::Inbound(message)).is_err() {
                break;
            }
        }
    });

    // The sessions listing.
    let mut poller = SessionPoller::for_claude(claude);
    let list_poker = poller.poker();
    let (cadence_tx, mut cadence_rx) = watch::channel(engine.cadence());
    let sessions_tx = tx.clone();
    tokio::spawn(async move {
        loop {
            poller.set_cadence(*cadence_rx.borrow_and_update());
            let rows = poller.next().await;
            if sessions_tx.send(Msg::Sessions(rows)).is_err() {
                break;
            }
        }
    });

    // The state files.
    let mut watcher = StateFileWatcher::new(Dir(home.sessions_dir()));
    let state_poker = watcher.poker();
    tracing::info!(native = watcher.native, "watching the state files");
    let files_tx = tx.clone();
    tokio::spawn(async move {
        loop {
            for session in watcher.changed().await {
                let report = watcher.report(&session).map(|(report, _)| report);
                let reply = watcher.reply(&session).map(|read| (read.text, read.modified));
                if files_tx.send(Msg::StateFile(session, report, reply)).is_err() {
                    return;
                }
            }
        }
    });

    // The clocks.
    let tick_tx = tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK);
        loop {
            interval.tick().await;
            if tick_tx.send(Msg::Tick).is_err() {
                break;
            }
        }
    });
    let keepalive_tx = tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + KEEPALIVE_FIRST, KEEPALIVE);
        loop {
            interval.tick().await;
            if keepalive_tx.send(Msg::Keepalive).is_err() {
                break;
            }
        }
    });
    let account_tx = tx.clone();
    let account_host = Arc::clone(&host);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(ACCOUNT);
        loop {
            interval.tick().await;
            let host = Arc::clone(&account_host);
            let status = tokio::task::spawn_blocking(move || host.auth_status())
                .await
                .unwrap_or_else(|error| Err(format!("the lookup panicked: {error}")));
            if account_tx.send(Msg::Account(status)).is_err() {
                break;
            }
        }
    });

    // The settings file, read again whenever its time or size changes (an editor's save, or
    // `settings.set` from another daemon's client).
    let config_tx = tx.clone();
    let config_path = home.config_path();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(CONFIG_POLL);
        let mut last = file_stamp(&config_path);
        loop {
            interval.tick().await;
            let stamp = file_stamp(&config_path);
            if stamp == last {
                continue;
            }
            last = stamp;
            let read = budd_claude::config::Config::load(&config_path).map_err(|error| error.to_string());
            if config_tx.send(Msg::Config(read)).is_err() {
                break;
            }
        }
    });

    // The stop signals, so launchd, the app or a Ctrl-C end the daemon with its lock and advert removed.
    let signal_tx = tx.clone();
    tokio::spawn(async move {
        let _ = signal_tx.send(Msg::Signal(stop_signal().await));
    });

    // The engine's loop.
    let mut clients = Clients::new();
    while let Some(message) = rx.recv().await {
        match message {
            Msg::Inbound(inbound) => {
                if let Some((client, request)) = clients.inbound(inbound) {
                    engine.command(client, request);
                }
            }
            Msg::Sessions(rows) => engine.on_sessions(rows),
            Msg::StateFile(session, report, reply) => engine.on_state_file(&session, report, reply),
            Msg::Tick => engine.tick(),
            Msg::Keepalive => engine.keepalive_tick(),
            Msg::Account(status) => engine.on_account(status),
            Msg::Config(file) => engine.on_config(file),
            Msg::Completion(completion) => engine.complete(completion),
            Msg::Signal(name) => {
                tracing::info!("shutting down on {name}");
                break;
            }
        }
        let _ = cadence_tx.send_replace(engine.cadence());
        for outgoing in engine.take_outgoing() {
            clients.dispatch(outgoing);
        }
        let mut shutdown = false;
        for effect in engine.take_effects() {
            match effect {
                Effect::ListNow => list_poker.poke(),
                Effect::ReadStateNow => state_poker.poke(),
                Effect::Shutdown => shutdown = true,
                effect => spawn_effect(Arc::clone(&host), effect, tx.clone()),
            }
        }
        if shutdown {
            tracing::info!("shutting down as asked");
            break;
        }
    }
    Ok(())
}

/// Waits for a signal that means stop, and says which: SIGTERM or SIGINT on Unix, Ctrl-C elsewhere.
async fn stop_signal() -> &'static str {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(term) => term,
            Err(error) => {
                tracing::warn!(%error, "SIGTERM cannot be watched; only Ctrl-C stops the daemon cleanly");
                let _ = tokio::signal::ctrl_c().await;
                return "Ctrl-C";
            }
        };
        tokio::select! {
            _ = term.recv() => "SIGTERM",
            _ = tokio::signal::ctrl_c() => "SIGINT",
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        "Ctrl-C"
    }
}

/// When the file at `path` was last written and how long it is, or `None` when it is not there: what
/// says it changed without reading it.
fn file_stamp(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Runs one slow effect on a blocking thread and sends its completion back.
fn spawn_effect(host: Arc<dyn ClaudeHost>, effect: Effect, tx: mpsc::UnboundedSender<Msg>) {
    tokio::task::spawn_blocking(move || {
        let completion = match effect {
            Effect::Send {
                id,
                target,
                message,
                pictures,
            } => Completion::Sent {
                id,
                outcome: host.send(&target, &message, &pictures),
            },
            Effect::Clear { id, target } => Completion::Cleared {
                id,
                outcome: host.clear(&target),
            },
            Effect::Stop { id, short_id } => Completion::Stopped {
                id,
                result: host.stop(&short_id),
            },
            Effect::StartBg { id, cwd, argv } => Completion::Started {
                id,
                result: host.start_bg(&cwd, &argv),
            },
            Effect::Doctor { id } => Completion::Doctor {
                id,
                checks: host.doctor(),
            },
            Effect::ListNow | Effect::ReadStateNow | Effect::Shutdown => return,
        };
        let _ = tx.send(Msg::Completion(completion));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_is_one_per_folder_and_a_stale_one_is_replaced() {
        let temp = tempfile::tempdir().expect("tempdir");
        let run = temp.path().join("run");
        let lock = Lock::acquire_with(&run, |_| true).expect("first");
        assert!(lock.path().exists());
        let text = fs::read_to_string(lock.path()).expect("reads");
        assert_eq!(text.trim(), std::process::id().to_string());
        // Our own pid in the file is our own stale lock, not another daemon.
        drop(lock);
        fs::write(run.join("daemon.lock"), "4242\n").expect("writes");
        match Lock::acquire_with(&run, |pid| pid == 4242) {
            Err(DaemonError::AlreadyRunning(4242)) => {}
            other => panic!("{other:?}"),
        }
        let lock = Lock::acquire_with(&run, |_| false).expect("stale replaced");
        assert_eq!(
            fs::read_to_string(lock.path()).expect("reads").trim(),
            std::process::id().to_string()
        );
        drop(lock);
        assert!(!run.join("daemon.lock").exists());
        fs::write(run.join("daemon.lock"), "not a pid").expect("writes");
        Lock::acquire_with(&run, |_| true).expect("garbage is stale");
        let advert = Advert::write(&run, "0.0.0", &Endpoint::UnixSocket(run.join("budd.sock"))).expect("writes");
        let text = fs::read_to_string(advert.path()).expect("reads");
        assert!(text.contains("\"pid\"") && text.contains("budd.sock"));
        let path = advert.path().to_path_buf();
        drop(advert);
        assert!(!path.exists());
    }
}
