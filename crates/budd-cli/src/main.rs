//! The `budd` binary.
//!
//! Subcommands land here as the adapters behind them land in `budd_claude` and `budd_engine`.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

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

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Hook { home, legacy_state_dir } => {
            budd_claude::hookrun::main(budd_claude::hookrun::Options { home, legacy_state_dir });
            Ok(())
        }
        Command::Mcp => {
            budd_claude::mcp_stdio::main();
            Ok(())
        }
        Command::Daemon {
            home,
            config_dir,
            assistant_name,
            claude,
        } => run_daemon(budd_engine::daemon::Options {
            home,
            config_dir,
            assistant_name,
            claude,
        }),
        Command::Watch { json: _, home } => run_watch(home),
    };
    if let Err(error) = result {
        eprintln!("budd: {error}");
        std::process::exit(1);
    }
}
