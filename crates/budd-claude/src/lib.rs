//! Adapters around the user's Claude Code CLI: processes, pty, state files, compat parsers.
//!
//! Everything that touches the file system, processes or sockets lives here, on top of the pure `budd_core`.

pub mod claude_cli;
pub mod compat;
pub mod hookrun;
pub mod ipc;
pub mod mcp_stdio;
pub mod messenger;
pub mod paths;
pub mod procs;
pub mod pty;
pub mod state_files;
pub mod watch;
