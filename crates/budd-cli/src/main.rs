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
    Hook,
    /// Budd's MCP server on stdio, started by Claude Code for each session.
    Mcp,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Hook => budd_claude::hookrun::main(),
        Command::Mcp => budd_claude::mcp_stdio::main(),
    }
}
