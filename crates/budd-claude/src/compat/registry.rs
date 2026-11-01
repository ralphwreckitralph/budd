//! Claude Code's own record of its live processes: `<config>/sessions/<pid>.json` (verified against 2.1.316).
//!
//! Each running `claude` writes one file named after its pid, with its session id, folder, name, job id,
//! status, messaging socket and version. A file can outlive its process, so a reader pairs it with a
//! liveness check; and every field is optional, as the file belongs to Claude Code.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// The folder under a config dir that holds the records.
pub const FOLDER: &str = "sessions";

/// One `sessions/<pid>.json`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionRecord {
    /// The process, from the file, else from the file's name.
    pub pid: Option<u32>,
    /// The full session id.
    pub session_id: Option<String>,
    /// The folder the session runs in.
    pub cwd: Option<String>,
    /// The name given with `--name`.
    pub name: Option<String>,
    /// The background job's id, for a `--bg` session.
    pub job_id: Option<String>,
    /// What the session is doing, as Claude Code words it.
    pub status: Option<String>,
    /// The Unix socket or named pipe the session takes messages on.
    pub messaging_socket_path: Option<String>,
    /// The Claude Code version that wrote the file.
    pub version: Option<String>,
}

impl SessionRecord {
    /// The record in `text`, or `None` when it is not a JSON object.
    pub fn decode(text: &str) -> Option<SessionRecord> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        value.is_object().then(|| serde_json::from_value(value).ok()).flatten()
    }
}

/// The folder the records are in.
pub fn folder(config_dir: &Path) -> PathBuf {
    config_dir.join(FOLDER)
}

/// Every record under `config_dir`, in no particular order; a config dir without the folder has none.
/// A file that is not a readable JSON object is skipped, and a record that does not name its pid takes
/// it from the file's name.
pub fn read_all(config_dir: &Path) -> Vec<SessionRecord> {
    let Ok(entries) = std::fs::read_dir(folder(config_dir)) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            tracing::debug!(path = %path.display(), "session record could not be read");
            continue;
        };
        let Some(mut record) = SessionRecord::decode(&text) else {
            tracing::debug!(path = %path.display(), "session record is not JSON");
            continue;
        };
        if record.pid.is_none() {
            record.pid = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(|stem| stem.parse().ok());
        }
        records.push(record);
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_records_and_skips_what_is_not_one() {
        let temp = tempfile::tempdir().unwrap();
        let dir = folder(temp.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("40001.json"),
            r#"{"pid": 40001, "sessionId": "11111111-1111-4111-8111-111111111111", "cwd": "/w/a", "name": "assistant",
                "jobId": "a1b2c3d4", "status": "idle", "messagingSocketPath": "/tmp/claude-40001.sock", "version": "2.1.316"}"#,
        )
        .unwrap();
        std::fs::write(dir.join("40002.json"), r#"{"sessionId": "2222", "cwd": "/w/b"}"#).unwrap();
        std::fs::write(dir.join("40003.json"), "half a {").unwrap();
        std::fs::write(dir.join("notes.txt"), "{}").unwrap();
        let mut records = read_all(temp.path());
        records.sort_by_key(|record| record.pid);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].pid, Some(40001));
        assert_eq!(records[0].job_id.as_deref(), Some("a1b2c3d4"));
        assert_eq!(
            records[0].messaging_socket_path.as_deref(),
            Some("/tmp/claude-40001.sock")
        );
        assert_eq!(records[0].version.as_deref(), Some("2.1.316"));
        // The pid comes from the file name when the record lacks it.
        assert_eq!(records[1].pid, Some(40002));
        assert_eq!(records[1].name, None);
    }

    #[test]
    fn a_config_dir_without_the_folder_has_no_records() {
        let temp = tempfile::tempdir().unwrap();
        assert!(read_all(temp.path()).is_empty());
        assert_eq!(SessionRecord::decode("[]"), None);
        assert_eq!(SessionRecord::decode("{}"), Some(SessionRecord::default()));
    }
}
