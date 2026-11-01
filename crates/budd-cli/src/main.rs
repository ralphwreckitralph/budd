//! The `budd` binary.
//!
//! Subcommands land here as the adapters behind them land in `budd_claude`.

use clap::{Parser, Subcommand};

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
        home: Option<std::path::PathBuf>,
        /// Also write every state and reply file into this folder (for the existing macOS app).
        #[arg(long, value_name = "DIR")]
        legacy_state_dir: Option<std::path::PathBuf>,
    },
    /// Budd's MCP server on stdio, started by Claude Code for each session.
    Mcp,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Hook { home, legacy_state_dir } => {
            budd_claude::hookrun::main(budd_claude::hookrun::Options { home, legacy_state_dir })
        }
        Command::Mcp => budd_claude::mcp_stdio::main(),
    }
}
