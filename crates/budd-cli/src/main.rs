//! The `budd` binary.
//!
//! Subcommands land here as the adapters behind them land in `budd_claude` and `budd_engine`.

use budd_claude::claude_cli::{self, Claude};
use budd_claude::messenger::{self, Message, Outcome, Target, Via};
use budd_core::delivery::PathStyle;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

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
fn budd_home(home: Option<PathBuf>) -> Result<budd_claude::paths::BuddHome, String> {
    match home {
        Some(home) => Ok(budd_claude::paths::BuddHome::at(home)),
        None => budd_claude::paths::BuddHome::resolve().map_err(|error| error.to_string()),
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
        Command::Hook { home, legacy_state_dir } => {
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
