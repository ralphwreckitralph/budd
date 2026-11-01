//! The `budd` binary.
//!
//! Subcommands land here as the adapters behind them land in `budd_claude`.

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
