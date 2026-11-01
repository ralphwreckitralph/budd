//! Adapters around the user's Claude Code CLI: processes, pty, state files, compat parsers.
//!
//! Everything that touches the file system, processes or sockets lives here, on top of the pure `budd_core`.

pub mod claude_cli;
pub mod compat;
pub mod config;
pub mod crew;
pub mod doctor;
pub mod gate;
pub mod hookrun;
pub mod ipc;
pub mod kit;
pub mod mcp_stdio;
pub mod messenger;
pub mod paths;
pub mod procs;
pub mod pty;
pub mod setup;
pub mod state_files;
pub mod watch;

/// Where the fake `claude` is for the tests in this crate, built if need be.
#[cfg(test)]
pub(crate) mod test_support {
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::OnceLock;

    const FAKE: &str = if cfg!(windows) {
        "fake-claude.exe"
    } else {
        "fake-claude"
    };

    /// `target/<profile>/fake-claude`, built with `cargo build -p fake-claude` when missing, or `None`
    /// with the reason printed, so a test can skip rather than fail where it cannot be had.
    pub fn fake_claude() -> Option<PathBuf> {
        static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
        FOUND
            .get_or_init(|| {
                if let Some(given) = std::env::var_os("FAKE_CLAUDE") {
                    return Some(PathBuf::from(given));
                }
                let exe = std::env::current_exe().ok()?;
                let deps = exe.parent()?;
                let profile = if deps.file_name().is_some_and(|name| name == "deps") {
                    deps.parent()?
                } else {
                    deps
                };
                let candidate = profile.join(FAKE);
                if !candidate.is_file() {
                    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
                    let built = Command::new(cargo)
                        .args(["build", "-p", "fake-claude", "--quiet"])
                        .current_dir(env!("CARGO_MANIFEST_DIR"))
                        .status()
                        .is_ok_and(|status| status.success());
                    if !built || !candidate.is_file() {
                        eprintln!(
                            "skipping: {} is missing; run `cargo build -p fake-claude` first",
                            candidate.display()
                        );
                        return None;
                    }
                }
                Some(candidate)
            })
            .clone()
    }
}
