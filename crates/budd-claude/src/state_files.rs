//! The per-session state files `budd hook` writes and the daemon reads, atomically.
//!
//! One folder holds `<session id>.json`, the [`SessionReport`] the hook last wrote for the session, and
//! `<session id>.reply.md`, the full text of its last reply. Every write goes to a temporary file in the
//! same folder and is renamed into place, so a reader never sees half a file. The layout is the one the
//! Python hook wrote for the macOS app, which is why the app can read a folder Budd writes.

use budd_core::presence::SessionReport;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The folder of state files for a set of sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dir(pub PathBuf);

impl Dir {
    /// The folder's path.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// The session's state file: `<dir>/<session id>.json`.
    pub fn report_path(&self, session_id: &str) -> PathBuf {
        self.0.join(format!("{session_id}.json"))
    }

    /// The session's reply file: `<dir>/<session id>.reply.md`.
    pub fn reply_path(&self, session_id: &str) -> PathBuf {
        self.0.join(format!("{session_id}.reply.md"))
    }

    /// The session's last report, or `None` when there is no file or it is not one.
    pub fn read_report(&self, session_id: &str) -> Option<SessionReport> {
        checked(session_id).ok()?;
        let text = fs::read_to_string(self.report_path(session_id)).ok()?;
        SessionReport::decode(&text)
    }

    /// Writes `report` as the state file of the session it names, atomically. Fails when the report has
    /// no session id, since the id is the file's name.
    pub fn write_report(&self, report: &SessionReport) -> io::Result<()> {
        let session_id = report
            .session_id
            .as_deref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the report names no session"))?;
        checked(session_id)?;
        let text = serde_json::to_string(report).map_err(io::Error::other)?;
        self.write_atomically(&self.report_path(session_id), text.as_bytes())
    }

    /// Writes `text` as the session's reply file, atomically.
    pub fn write_reply(&self, session_id: &str, text: &str) -> io::Result<()> {
        checked(session_id)?;
        self.write_atomically(&self.reply_path(session_id), text.as_bytes())
    }

    /// Removes every file in the folder not modified for `older_than`, and says how many went. A folder
    /// that does not exist has nothing to prune.
    pub fn prune(&self, older_than: Duration) -> io::Result<usize> {
        let cutoff = SystemTime::now()
            .checked_sub(older_than)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let entries = match fs::read_dir(&self.0) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error),
        };
        let mut removed = 0;
        for entry in entries {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if !metadata.is_file() {
                continue;
            }
            if metadata.modified()? < cutoff && fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Every session with a state file in the folder, as (session id, report), in no particular order.
    /// Files that are not reports are left out; a missing folder lists nothing.
    pub fn list(&self) -> io::Result<Vec<(String, SessionReport)>> {
        let entries = match fs::read_dir(&self.0) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut found = Vec::new();
        for entry in entries {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(session_id) = name.strip_suffix(".json") else {
                continue;
            };
            if session_id.is_empty() || session_id.starts_with('.') {
                continue;
            }
            if let Some(report) = fs::read_to_string(&path)
                .ok()
                .and_then(|text| SessionReport::decode(&text))
            {
                found.push((session_id.to_owned(), report));
            }
        }
        Ok(found)
    }

    /// Writes `bytes` to a temporary file beside `path` and renames it into place, creating the folder first.
    fn write_atomically(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        fs::create_dir_all(&self.0)?;
        let mut file = tempfile::Builder::new().prefix(".tmp-").tempfile_in(&self.0)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    }
}

/// Refuses a session id that would name a file outside the folder or a hidden one.
fn checked(session_id: &str) -> io::Result<()> {
    let plain = !session_id.is_empty()
        && !session_id.starts_with('.')
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
    if plain {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a session id: {session_id:?}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn report(session_id: &str, state: &str) -> SessionReport {
        let mut report = SessionReport::new(state, 1.0);
        report.session_id = Some(session_id.to_owned());
        report
    }

    #[test]
    fn reports_and_replies_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Dir(temp.path().join("sessions"));
        assert_eq!(dir.read_report("s1"), None);
        assert!(dir.list().unwrap().is_empty());
        dir.write_report(&report("s1", "working")).unwrap();
        assert_eq!(dir.read_report("s1").unwrap().state, "working");
        assert_eq!(dir.report_path("s1"), temp.path().join("sessions").join("s1.json"));
        dir.write_reply("s1", "pong").unwrap();
        assert_eq!(fs::read_to_string(dir.reply_path("s1")).unwrap(), "pong");
        // Overwriting replaces the whole file, and leaves no temporary file behind.
        dir.write_report(&report("s1", "done")).unwrap();
        assert_eq!(dir.read_report("s1").unwrap().state, "done");
        let names: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.iter().all(|name| !name.starts_with(".tmp-")));
        dir.write_report(&report("s2", "idle")).unwrap();
        let mut listed: Vec<String> = dir.list().unwrap().into_iter().map(|(id, _)| id).collect();
        listed.sort();
        assert_eq!(listed, vec!["s1", "s2"]);
    }

    #[test]
    fn a_report_without_a_session_or_with_a_path_for_one_is_refused() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Dir(temp.path().to_path_buf());
        let nameless = SessionReport::new("idle", 1.0);
        assert_eq!(
            dir.write_report(&nameless).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            dir.write_reply("../escape", "x").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            dir.write_reply("", "x").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(dir.read_report("../etc/passwd"), None);
        assert!(!temp.path().parent().unwrap().join("escape.reply.md").exists());
    }

    #[test]
    fn a_broken_file_reads_as_nothing_and_is_not_listed() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Dir(temp.path().to_path_buf());
        fs::write(dir.report_path("bad"), "not json").unwrap();
        assert_eq!(dir.read_report("bad"), None);
        assert!(dir.list().unwrap().is_empty());
    }

    #[test]
    fn pruning_removes_only_old_files() {
        let temp = tempfile::tempdir().unwrap();
        let dir = Dir(temp.path().to_path_buf());
        dir.write_report(&report("fresh", "idle")).unwrap();
        let old = dir.report_path("old");
        File::create(&old).unwrap();
        let long_ago = SystemTime::now() - Duration::from_secs(10 * 24 * 3600);
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        assert_eq!(dir.prune(Duration::from_secs(7 * 24 * 3600)).unwrap(), 1);
        assert!(!old.exists());
        assert!(dir.report_path("fresh").exists());
        assert_eq!(Dir(temp.path().join("missing")).prune(Duration::ZERO).unwrap(), 0);
    }
}
