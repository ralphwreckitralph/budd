//! The `budd` binary.
//!
//! Subcommands land here as the adapters behind them land in `budd_claude` and `budd_engine`.

use budd_claude::claude_cli::{self, Claude};
use budd_claude::doctor;
use budd_claude::messenger::{self, Message, Outcome, Target, Via};
use budd_claude::paths::BuddHome;
use budd_claude::setup::{self, ConnectOptions, Terminal};
use budd_core::delivery::PathStyle;
use budd_proto::{Command as DaemonCommand, Request};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

/// How long `budd reset` gives the daemon to forget and stop the assistant.
const RESET_WAIT: Duration = Duration::from_secs(30);

/// Budd: an office for your Claude Code background sessions.
#[derive(Parser)]
#[command(name = "budd", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// The lifecycle hook Claude Code runs on every session event (reads the event on stdin).
    Hook {
        /// The event's name, as the settings pass it; the event on stdin is the one that counts.
        #[arg(value_name = "EVENT")]
        event: Option<String>,
        /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
        #[arg(long, value_name = "DIR")]
        home: Option<PathBuf>,
        /// Also write every state and reply file into this folder (for the existing macOS app).
        #[arg(long, value_name = "DIR")]
        legacy_state_dir: Option<PathBuf>,
    },
    /// Budd's MCP server on stdio, started by Claude Code for each session.
    Mcp,
    /// The daemon: follows the sessions, serves the app and the tools, keeps the assistant alive.
    Daemon {
        /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
        #[arg(long, value_name = "DIR")]
        home: Option<PathBuf>,
        /// The Claude config folder of Budd's account, instead of `<home>/claude`.
        #[arg(long, value_name = "DIR")]
        config_dir: Option<PathBuf>,
        /// The assistant session's name, instead of "assistant".
        #[arg(long, value_name = "NAME")]
        assistant_name: Option<String>,
        /// The `claude` binary, instead of the one found.
        #[arg(long, value_name = "PATH")]
        claude: Option<PathBuf>,
    },
    /// Checks the install: claude, Budd's account, the hooks, the MCP server, the endpoint, the daemon.
    ///
    /// Exits 0 when nothing failed, 1 when something must be fixed before Budd works.
    Doctor {
        /// Print the checks as JSON, one object per check.
        #[arg(long)]
        json: bool,
        /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
        #[arg(long, value_name = "DIR")]
        home: Option<PathBuf>,
        /// The `claude` binary, instead of the one found.
        #[arg(long, value_name = "PATH")]
        claude: Option<PathBuf>,
    },
    /// Prepares Budd's home: the folders, the hooks, the MCP server, the assistant's persona, config.json.
    ///
    /// Safe to run again: what is already right is kept, and the human's own edits stay.
    Setup {
        /// Say what would be done without doing it.
        #[arg(long)]
        dry_run: bool,
        /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
        #[arg(long, value_name = "DIR")]
        home: Option<PathBuf>,
        /// The `claude` binary, instead of the one found.
        #[arg(long, value_name = "PATH")]
        claude: Option<PathBuf>,
    },
    /// Logs Budd's own Claude folder in to claude.ai, in a terminal window, and waits for it.
    Connect {
        /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
        #[arg(long, value_name = "DIR")]
        home: Option<PathBuf>,
        /// The `claude` binary, instead of the one found.
        #[arg(long, value_name = "PATH")]
        claude: Option<PathBuf>,
        /// Where the login opens: "terminal" (the platform's) or "iterm".
        #[arg(long, value_name = "NAME", default_value = "terminal")]
        terminal: String,
        /// How many seconds to wait for the login.
        #[arg(long, value_name = "SECONDS", default_value_t = setup::LOGIN_WAIT.as_secs())]
        wait: u64,
        /// A Claude config folder (usually ~/.claude) whose trusted folders Budd's folder takes on.
        #[arg(long, value_name = "DIR")]
        carry_trust_from: Option<PathBuf>,
        /// A Claude config folder whose memory for the assistant's folder is copied over, once.
        #[arg(long, value_name = "DIR")]
        import_memory_from: Option<PathBuf>,
    },
    /// Forgets what Budd learned while running and stops the assistant, so he starts over.
    ///
    /// The config, Budd's Claude folder and its login stay, unless `--sign-out` is given.
    Reset {
        /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
        #[arg(long, value_name = "DIR")]
        home: Option<PathBuf>,
        /// The `claude` binary, instead of the one found.
        #[arg(long, value_name = "PATH")]
        claude: Option<PathBuf>,
        /// Also log Budd's Claude folder out.
        #[arg(long)]
        sign_out: bool,
    },
    /// Connects to the daemon and prints every line it sends, as JSON.
    Watch {
        /// Print the raw JSON lines (the only form for now).
        #[arg(long)]
        json: bool,
        /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
        #[arg(long, value_name = "DIR")]
        home: Option<PathBuf>,
    },
    /// Type a message into a background session through `claude attach` and say whether it arrived.
    ///
    /// Prints one JSON line: {"outcome":"delivered","via":"pty"} or {"outcome":"unconfirmed","reason":"..."}.
    /// Exits 0 when delivered and 1 when not.
    Send {
        /// The Claude config folder the session runs under (`CLAUDE_CONFIG_DIR`).
        #[arg(long, value_name = "DIR")]
        config_dir: PathBuf,
        /// The session's short id, as `claude agents` lists it.
        #[arg(long, value_name = "SHORT ID")]
        session: String,
        /// The session's full session id (not needed to send).
        #[arg(long, value_name = "ID")]
        session_id: Option<String>,
        /// The session's transcript, where the message is confirmed; without it a send is never confirmed.
        #[arg(long, value_name = "PATH")]
        transcript: Option<PathBuf>,
        /// The session is asleep (retired while it waited on you), so `claude attach` wakes it first.
        #[arg(long)]
        asleep: bool,
        /// A picture to attach, as a path without spaces; repeatable.
        #[arg(long, value_name = "PATH")]
        picture: Vec<PathBuf>,
        /// A file to drop on the session, pasted as its escaped path after the text; repeatable.
        #[arg(long, value_name = "PATH")]
        file: Vec<PathBuf>,
        /// The `claude` binary, instead of the one Budd finds.
        #[arg(long, value_name = "EXE")]
        claude: Option<PathBuf>,
        /// The message.
        text: String,
    },
    /// Clear a background session's conversation, as `/clear` typed in its terminal would.
    ///
    /// Prints one JSON line as `send` does; the clear counts once `claude agents` lists the session
    /// with a new session id. Exits 0 when it did and 1 when not.
    Clear {
        /// The Claude config folder the session runs under (`CLAUDE_CONFIG_DIR`).
        #[arg(long, value_name = "DIR")]
        config_dir: PathBuf,
        /// The session's short id, as `claude agents` lists it.
        #[arg(long, value_name = "SHORT ID")]
        session: String,
        /// The session id it has now, which the clear replaces.
        #[arg(long, value_name = "ID")]
        session_id: String,
        /// The `claude` binary, instead of the one Budd finds.
        #[arg(long, value_name = "EXE")]
        claude: Option<PathBuf>,
    },
}

/// The Budd home the command line or the environment names.
fn budd_home(home: Option<PathBuf>) -> Result<BuddHome, String> {
    match home {
        Some(home) => Ok(BuddHome::at(home)),
        None => BuddHome::resolve().map_err(|error| error.to_string()),
    }
}

/// Where to look for `claude` first: the command line's path, else the config file's `claude_path`.
fn claude_hint(flag: Option<PathBuf>, home: &BuddHome) -> Option<PathBuf> {
    flag.or_else(|| {
        budd_claude::config::Config::load(&home.config_path())
            .ok()
            .and_then(|config| config.claude_path)
    })
}

/// The `claude` Budd's own folder runs, when one is found: the hint, the usual places, then `PATH`.
fn budds_claude(flag: Option<PathBuf>, home: &BuddHome) -> Option<Claude> {
    let hint = claude_hint(flag, home);
    claude_cli::locate(hint.as_deref()).map(|exe| {
        let exe = std::path::absolute(&exe).unwrap_or(exe);
        Claude::new(exe, home.claude_dir())
    })
}

/// This binary, as the hooks and the MCP server must name it.
fn this_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|error| format!("finding this binary: {error}"))?;
    Ok(std::path::absolute(&exe).unwrap_or(exe))
}

fn run_doctor(json: bool, home: Option<PathBuf>, claude: Option<PathBuf>) -> Result<bool, String> {
    let home = budd_home(home)?;
    let hint = claude_hint(claude, &home);
    let context = doctor::Context::resolve(home, hint.as_deref());
    let checks = doctor::run(&context);
    if json {
        let text = serde_json::to_string_pretty(&checks).map_err(|error| error.to_string())?;
        println!("{text}");
    } else {
        for check in &checks {
            println!("{}", check.line());
        }
    }
    Ok(doctor::worst(&checks) != doctor::State::Fail)
}

fn run_setup(dry_run: bool, home: Option<PathBuf>, claude: Option<PathBuf>) -> Result<(), String> {
    let home = budd_home(home)?;
    let claude = budds_claude(claude, &home);
    if claude.is_none() {
        eprintln!(
            "budd: claude was not found, so the MCP server is not registered; run setup again once it is installed"
        );
    }
    let exe = this_exe()?;
    let lines = setup::setup(&home, claude.as_ref(), &exe, dry_run).map_err(|error| error.to_string())?;
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

fn run_connect(
    home: Option<PathBuf>,
    claude: Option<PathBuf>,
    terminal: &str,
    wait: u64,
    carry_trust_from: Option<PathBuf>,
    import_memory_from: Option<PathBuf>,
) -> Result<(), String> {
    let home = budd_home(home)?;
    let terminal =
        Terminal::parse(terminal).ok_or_else(|| format!("{terminal:?} is not a terminal; use terminal or iterm"))?;
    let claude = budds_claude(claude, &home).ok_or_else(|| setup::Error::NoClaude.to_string())?;
    let options = ConnectOptions {
        terminal,
        wait: Duration::from_secs(wait),
        carry_trust_from,
        import_memory_from,
    };
    let mut out = std::io::stdout();
    setup::connect(&home, &claude, &options, &mut out)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn run_reset(home: Option<PathBuf>, claude: Option<PathBuf>, sign_out: bool) -> Result<(), String> {
    let home = budd_home(home)?;
    let claude = budds_claude(claude, &home);
    let endpoint = home.endpoint();
    let mut via_daemon = || {
        let pid = doctor::running_daemon(&home)?;
        Some(ask_daemon(&endpoint, DaemonCommand::Reset, RESET_WAIT).map_err(|error| format!("pid {pid}: {error}")))
    };
    let report = setup::reset(&home, claude.as_ref(), sign_out, &mut via_daemon).map_err(|error| error.to_string())?;
    if report.via_daemon {
        println!("The daemon forgot what it learned and stopped the assistant; the keep-alive starts a fresh one.");
    } else {
        println!("No daemon running; the state was reset on disk.");
    }
    if report.state_deleted {
        println!("Deleted {}", home.state_dir().join(setup::STATE_FILE).display());
    }
    if report.drops_removed > 0 {
        println!(
            "Removed {} dropped files from {}",
            report.drops_removed,
            home.drops_dir().display()
        );
    }
    if report.signed_out {
        println!("Signed out of Budd's Claude folder; run `budd connect` to log in again.");
    }
    Ok(())
}

/// Sends `command` to the daemon at `endpoint` and waits for its reply, for up to `wait`.
fn ask_daemon(endpoint: &budd_claude::ipc::Endpoint, command: DaemonCommand, wait: Duration) -> Result<(), String> {
    let runtime = tokio::runtime::Runtime::new().map_err(|error| format!("starting the runtime: {error}"))?;
    let request = Request { id: Some(1), command };
    let reply = runtime
        .block_on(budd_engine::client::request(endpoint, &request, wait))
        .map_err(|error| {
            if budd_claude::ipc::is_not_running(&error) {
                format!("no budd daemon at {endpoint}")
            } else {
                error.to_string()
            }
        })?;
    if reply.ok {
        Ok(())
    } else {
        Err(reply.error.unwrap_or_else(|| "the daemon refused".to_owned()))
    }
}

/// Sends the daemon's log to `<home>/logs/daemon.log`, at the level `BUDD_LOG` names (info by default).
fn log_to(home: &budd_claude::paths::BuddHome) -> Result<(), String> {
    let logs = home.logs_dir();
    std::fs::create_dir_all(&logs).map_err(|error| format!("creating {}: {error}", logs.display()))?;
    let file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(logs.join("daemon.log"))
        .map_err(|error| format!("opening the log: {error}"))?;
    let filter = tracing_subscriber::EnvFilter::try_from_env("BUDD_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .try_init()
        .map_err(|error| format!("setting up the log: {error}"))
}

fn run_daemon(options: budd_engine::daemon::Options) -> Result<(), String> {
    let home = budd_home(options.home.clone())?;
    log_to(&home)?;
    let runtime = tokio::runtime::Runtime::new().map_err(|error| format!("starting the runtime: {error}"))?;
    let mut options = options;
    options.home = Some(home.root().to_path_buf());
    runtime
        .block_on(budd_engine::daemon::run(options))
        .map_err(|error| error.to_string())
}

fn run_watch(home: Option<PathBuf>) -> Result<(), String> {
    let home = budd_home(home)?;
    let endpoint = home.endpoint();
    let runtime = tokio::runtime::Runtime::new().map_err(|error| format!("starting the runtime: {error}"))?;
    let mut out = std::io::stdout();
    runtime
        .block_on(budd_engine::client::watch(&endpoint, "watch", &mut out))
        .map_err(|error| {
            if budd_claude::ipc::is_not_running(&error) {
                format!("no budd daemon at {endpoint}")
            } else {
                error.to_string()
            }
        })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Hook {
            event: _,
            home,
            legacy_state_dir,
        } => {
            budd_claude::hookrun::main(budd_claude::hookrun::Options { home, legacy_state_dir });
            ExitCode::SUCCESS
        }
        Command::Mcp => {
            budd_claude::mcp_stdio::main();
            ExitCode::SUCCESS
        }
        Command::Daemon {
            home,
            config_dir,
            assistant_name,
            claude,
        } => exit_code(run_daemon(budd_engine::daemon::Options {
            home,
            config_dir,
            assistant_name,
            claude,
        })),
        Command::Doctor { json, home, claude } => match run_doctor(json, home, claude) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(error) => exit_code(Err(error)),
        },
        Command::Setup { dry_run, home, claude } => exit_code(run_setup(dry_run, home, claude)),
        Command::Connect {
            home,
            claude,
            terminal,
            wait,
            carry_trust_from,
            import_memory_from,
        } => exit_code(run_connect(
            home,
            claude,
            &terminal,
            wait,
            carry_trust_from,
            import_memory_from,
        )),
        Command::Reset { home, claude, sign_out } => exit_code(run_reset(home, claude, sign_out)),
        Command::Watch { json: _, home } => exit_code(run_watch(home)),
        Command::Send {
            config_dir,
            session,
            session_id,
            transcript,
            asleep,
            picture,
            file,
            claude,
            text,
        } => {
            let target = Target {
                short_id: session,
                session_id: session_id.unwrap_or_default(),
                transcript,
                asleep,
            };
            let message = Message {
                text,
                pictures: picture,
                files: file,
            };
            report(
                claude_at(claude, config_dir).map(|claude| messenger::send(&claude, &target, &message, path_style())),
            )
        }
        Command::Clear {
            config_dir,
            session,
            session_id,
            claude,
        } => {
            let target = Target {
                short_id: session,
                session_id,
                transcript: None,
                asleep: false,
            };
            report(claude_at(claude, config_dir).map(|claude| messenger::clear(&claude, &target)))
        }
    }
}

/// The `claude` to drive: `exe` when given, else the one Budd finds; or why there is none. The path
/// is made absolute, since every run of it starts in another folder.
fn claude_at(exe: Option<PathBuf>, config_dir: PathBuf) -> Result<Claude, String> {
    let exe = exe
        .or_else(|| claude_cli::locate(None))
        .ok_or_else(|| "claude was not found; pass --claude <exe>".to_owned())?;
    let exe = std::path::absolute(&exe).unwrap_or(exe);
    Ok(Claude::new(exe, config_dir))
}

/// How a dropped file's path is written for the session's shell on this platform.
fn path_style() -> PathStyle {
    if cfg!(windows) {
        PathStyle::WindowsQuoted
    } else {
        PathStyle::Posix
    }
}

/// Success, or the error on stderr and a failing exit code.
fn exit_code(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("budd: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Prints the outcome as one JSON line and turns it into the exit code.
fn report(outcome: Result<Outcome, String>) -> ExitCode {
    let (line, delivered) = match outcome {
        Ok(Outcome::Delivered { via }) => {
            let via = match via {
                Via::Pty => "pty",
            };
            (serde_json::json!({"outcome": "delivered", "via": via}), true)
        }
        Ok(Outcome::Unconfirmed(reason)) | Err(reason) => {
            (serde_json::json!({"outcome": "unconfirmed", "reason": reason}), false)
        }
    };
    println!("{line}");
    if delivered {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
