//! What the daemon remembers between runs: `<home>/state/budd-state.json`.
//!
//! The same facts the macOS app kept in its defaults under the state keys, so a restart never calls
//! the human over for an ask they saw, never answers an ask autopilot answered, and keeps the unread
//! count. Written whole, to a temporary file renamed into place.

use budd_core::reply::PilotMode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io;
use std::path::Path;

/// The file's contents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Persisted {
    /// What autopilot answered, by session: `{session: {reply, answer, at, failed}}`.
    pub autopilot_answers: Value,
    /// The reply each blocked crew session was on when the human saw it, by session.
    pub crew_attention: HashMap<String, String>,
    /// How many of the assistant's replies are unread.
    pub unread_count: usize,
    /// When the newest reply seen was written.
    pub unread_newest: Option<f64>,
    /// Who answers the crew's asks.
    pub pilot_mode: PilotMode,
    /// Whether the office is open.
    pub office_open: bool,
}

impl Default for Persisted {
    fn default() -> Self {
        Persisted {
            autopilot_answers: Value::Object(Default::default()),
            crew_attention: HashMap::new(),
            unread_count: 0,
            unread_newest: None,
            pilot_mode: PilotMode::Human,
            office_open: true,
        }
    }
}

impl Persisted {
    /// The file at `path`, or the defaults when it is missing or not readable as one.
    pub fn load(path: &Path) -> Persisted {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes the file at `path`, creating its folder, through a temporary file beside it.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_state_round_trips_and_a_missing_file_is_the_defaults() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("state").join("budd-state.json");
        assert_eq!(Persisted::load(&path), Persisted::default());
        let state = Persisted {
            autopilot_answers: json!({"s1": {"reply": "abc", "answer": "go", "at": 1.0, "failed": false}}),
            crew_attention: HashMap::from([("s1".to_owned(), "abc".to_owned())]),
            unread_count: 2,
            unread_newest: Some(5.0),
            pilot_mode: PilotMode::Autopilot,
            office_open: false,
        };
        state.save(&path).expect("save");
        assert_eq!(Persisted::load(&path), state);
        assert!(!path.with_extension(format!("tmp-{}", std::process::id())).exists());
        // Keys a later build adds are ignored; keys it lacks take their defaults.
        std::fs::write(&path, r#"{"unread_count": 3, "later": true}"#).expect("write");
        let loaded = Persisted::load(&path);
        assert_eq!(loaded.unread_count, 3);
        assert!(loaded.office_open);
        std::fs::write(&path, "garbage").expect("write");
        assert_eq!(Persisted::load(&path), Persisted::default());
    }
}
