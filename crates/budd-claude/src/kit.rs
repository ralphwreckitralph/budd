//! The kit's small tools, each a subcommand of `budd`: the evidence set per ticket, the per-worktree
//! handover log, the expectation document check, and the working-tree fingerprint.
//!
//! They are ports of the pipeline scripts the private lane ran (`bin/evidence`, `handover.sh`,
//! `expectation-shape.mjs`, `lib/fingerprint.sh`), kept to the same arguments and the same printed
//! lines so the skills that call them need no change. What they share lives here: where the Claude
//! config folder is, the clock, and how `git` is run (through `std::process`, never a shell, so the
//! same code runs on Windows).

pub mod evidence;
pub mod expect;
pub mod fingerprint;
pub mod handover;

use crate::paths::BuddHome;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// The variable Claude Code keeps its config folder in.
pub const CONFIG_DIR_VAR: &str = "CLAUDE_CONFIG_DIR";

/// The Claude config folder the kit's state lives under: `CLAUDE_CONFIG_DIR` when set and not empty,
/// else `<budd home>/claude`, Budd's own account. The human's `~/.claude` is never assumed.
pub fn config_dir(home: &BuddHome) -> PathBuf {
    config_dir_from(std::env::var_os(CONFIG_DIR_VAR), home)
}

/// [`config_dir`] with the variable's value passed in, for the tests.
pub fn config_dir_from(var: Option<std::ffi::OsString>, home: &BuddHome) -> PathBuf {
    match var.filter(|value| !value.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => home.claude_dir(),
    }
}

/// A moment in UTC, broken into its calendar fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utc {
    /// The year.
    pub year: i64,
    /// The month, 1 to 12.
    pub month: u32,
    /// The day of the month, 1 to 31.
    pub day: u32,
    /// The hour, 0 to 23.
    pub hour: u32,
    /// The minute.
    pub minute: u32,
    /// The second.
    pub second: u32,
}

impl Utc {
    /// Now.
    pub fn now() -> Utc {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Utc::from_unix(seconds)
    }

    /// The moment `seconds` after the Unix epoch, by the civil-from-days arithmetic of Howard Hinnant.
    pub fn from_unix(seconds: u64) -> Utc {
        let days = (seconds / 86_400) as i64;
        let rest = (seconds % 86_400) as u32;
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = yoe + era * 400 + i64::from(month <= 2);
        Utc {
            year,
            month,
            day,
            hour: rest / 3600,
            minute: rest % 3600 / 60,
            second: rest % 60,
        }
    }

    /// ISO 8601 to the second, as Python's `isoformat(timespec="seconds")` writes a UTC moment:
    /// `2026-11-01T09:30:00+00:00`.
    pub fn iso(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}+00:00",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// As `date '+%Y-%m-%d %H:%M %z'` prints it, in UTC: `2026-11-01 09:30 +0000`.
    pub fn stamp(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02} +0000",
            self.year, self.month, self.day, self.hour, self.minute
        )
    }
}

/// What one `git` run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitOutput {
    /// Whether git exited 0.
    pub ok: bool,
    /// Its stdout, as text.
    pub stdout: String,
    /// Its stderr, as text.
    pub stderr: String,
}

impl GitOutput {
    /// Stdout without the trailing newline, or `None` when git failed.
    pub fn line(&self) -> Option<String> {
        self.ok.then(|| self.stdout.trim_end_matches(['\r', '\n']).to_owned())
    }
}

/// Runs `git -C <repo> <args>` with `env` set for that run alone, and reads what it printed. A `git`
/// that cannot be started at all (not installed) counts as a failed run whose stderr says so.
pub fn git<I, S>(repo: &Path, args: I, env: &[(&str, &OsStr)]) -> GitOutput
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    command.arg("-C").arg(repo).args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    match command.output() {
        Ok(output) => GitOutput {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        },
        Err(error) => GitOutput {
            ok: false,
            stdout: String::new(),
            stderr: format!("git could not be run: {error}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_dir_is_the_variable_else_budds_claude_folder() {
        let home = BuddHome::at("/b");
        assert_eq!(config_dir_from(None, &home), PathBuf::from("/b/claude"));
        assert_eq!(config_dir_from(Some("".into()), &home), PathBuf::from("/b/claude"));
        assert_eq!(config_dir_from(Some("/c".into()), &home), PathBuf::from("/c"));
    }

    #[test]
    fn utc_breaks_the_epoch_seconds_into_calendar_fields() {
        assert_eq!(Utc::from_unix(0).iso(), "1970-01-01T00:00:00+00:00");
        // 2026-11-01 09:30:05 UTC.
        let at = Utc::from_unix(1_793_525_405);
        assert_eq!(at.iso(), "2026-11-01T09:30:05+00:00");
        assert_eq!(at.stamp(), "2026-11-01 09:30 +0000");
        // A leap day and a year end.
        assert_eq!(Utc::from_unix(1_709_164_800).iso(), "2024-02-29T00:00:00+00:00");
        assert_eq!(Utc::from_unix(1_735_689_599).iso(), "2024-12-31T23:59:59+00:00");
    }
}
