//! What the daemon was started with: the folders, the assistant's name and model, the budgets.

use budd_core::crew::CrewBoard;
use budd_core::hook::ROTATE_AT;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The daemon's configuration, fixed for its lifetime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Budd's home folder.
    pub home: PathBuf,
    /// The `CLAUDE_CONFIG_DIR` of Budd's own account.
    pub config_dir: PathBuf,
    /// The name of the assistant's session.
    pub assistant_name: String,
    /// The model a fresh assistant starts on.
    pub assistant_model: String,
    /// The context size at which a crew session counts as full.
    pub crew_context_limit: u64,
    /// The context size at which the keep-alive replaces the assistant.
    pub rotate_at: u64,
    /// The `claude` binary in use, when found.
    pub claude: Option<PathBuf>,
    /// The daemon's version.
    pub version: String,
}

impl Config {
    /// The name the assistant's session has unless told otherwise.
    pub const DEFAULT_ASSISTANT: &'static str = "assistant";
    /// The model a fresh assistant starts on unless told otherwise.
    pub const DEFAULT_MODEL: &'static str = "opus";

    /// The defaults for `home`, with `config_dir` as the account's folder.
    pub fn new(home: impl Into<PathBuf>, config_dir: impl Into<PathBuf>) -> Config {
        Config {
            home: home.into(),
            config_dir: config_dir.into(),
            assistant_name: Self::DEFAULT_ASSISTANT.to_owned(),
            assistant_model: Self::DEFAULT_MODEL.to_owned(),
            crew_context_limit: CrewBoard::DEFAULT_CONTEXT_LIMIT,
            rotate_at: ROTATE_AT,
            claude: None,
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    /// The configuration with what `<home>/config.json` says laid over it: `sessionName`,
    /// `assistantModel` and `crewContextLimit`, the keys the macOS app used. A missing or broken file
    /// changes nothing.
    pub fn with_file(mut self, path: &Path) -> Config {
        let Ok(text) = std::fs::read_to_string(path) else {
            return self;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return self;
        };
        if let Some(name) = value
            .get("sessionName")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            self.assistant_name = name.to_owned();
        }
        if let Some(model) = value
            .get("assistantModel")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            self.assistant_model = model.to_owned();
        }
        if let Some(limit) = value.get("crewContextLimit").and_then(Value::as_u64).filter(|n| *n > 0) {
            self.crew_context_limit = limit;
        }
        self
    }

    /// The folder the assistant's own session runs in: `<home>/assistant`.
    pub fn assistant_dir(&self) -> PathBuf {
        self.home.join("assistant")
    }

    /// Where the daemon keeps what it remembers: `<home>/state/budd-state.json`.
    pub fn state_path(&self) -> PathBuf {
        self.home.join("state").join("budd-state.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_lays_over_the_defaults() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("config.json");
        let config = Config::new(temp.path(), temp.path().join("claude")).with_file(&path);
        assert_eq!(config.assistant_name, "assistant");
        assert_eq!(config.crew_context_limit, CrewBoard::DEFAULT_CONTEXT_LIMIT);
        assert_eq!(config.assistant_dir(), temp.path().join("assistant"));
        assert_eq!(config.state_path(), temp.path().join("state").join("budd-state.json"));
        std::fs::write(
            &path,
            r#"{"sessionName": "budd", "crewContextLimit": 1000, "assistantModel": "sonnet", "other": 1}"#,
        )
        .expect("write");
        let config = Config::new(temp.path(), temp.path().join("claude")).with_file(&path);
        assert_eq!(config.assistant_name, "budd");
        assert_eq!(config.assistant_model, "sonnet");
        assert_eq!(config.crew_context_limit, 1000);
        std::fs::write(&path, "garbage").expect("write");
        assert_eq!(
            Config::new(temp.path(), temp.path().join("claude"))
                .with_file(&path)
                .assistant_name,
            "assistant"
        );
    }
}
