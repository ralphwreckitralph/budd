//! What the daemon runs with: the folders, the binary, and the settings of `<home>/config.json`.
//!
//! The folders and the version are fixed for the daemon's life; the settings are the file's, laid over
//! whatever the command line said, and are applied again whenever the file changes or `settings.set`
//! writes it.

use budd_claude::config::Config as FileConfig;
use std::path::{Path, PathBuf};

/// The daemon's configuration.
#[derive(Debug, Clone, PartialEq)]
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
    /// The settings file as last read, defaults filled in.
    pub file: FileConfig,
    /// The assistant's name the command line fixed, which the file never overrides.
    pub fixed_assistant_name: Option<String>,
}

impl Config {
    /// The name the assistant's session has unless told otherwise.
    pub const DEFAULT_ASSISTANT: &'static str = "assistant";
    /// The model a fresh assistant starts on unless told otherwise.
    pub const DEFAULT_MODEL: &'static str = "opus";

    /// The defaults for `home`, with `config_dir` as the account's folder.
    pub fn new(home: impl Into<PathBuf>, config_dir: impl Into<PathBuf>) -> Config {
        let mut config = Config {
            home: home.into(),
            config_dir: config_dir.into(),
            assistant_name: Self::DEFAULT_ASSISTANT.to_owned(),
            assistant_model: Self::DEFAULT_MODEL.to_owned(),
            crew_context_limit: 0,
            rotate_at: 0,
            claude: None,
            version: env!("CARGO_PKG_VERSION").to_owned(),
            file: FileConfig::default(),
            fixed_assistant_name: None,
        };
        config.apply(FileConfig::default());
        config
    }

    /// The configuration with `<home>/config.json` laid over it. A missing file changes nothing; a
    /// broken one is logged and changes nothing either, so the daemon still comes up.
    pub fn with_file(mut self, path: &Path) -> Config {
        match FileConfig::load(path) {
            Ok(file) => self.apply(file),
            Err(error) => tracing::warn!(%error, "config.json was not read; running on the defaults"),
        }
        self
    }

    /// Takes `file`'s settings: the assistant's name (unless the command line fixed it), its model,
    /// the crew's context limit and the rotation point.
    pub fn apply(&mut self, file: FileConfig) {
        self.assistant_name = self
            .fixed_assistant_name
            .clone()
            .unwrap_or_else(|| file.assistant_name.clone());
        self.assistant_model = file.assistant.model.clone();
        self.crew_context_limit = file.crew.context_limit.max(1);
        self.rotate_at = file.assistant.rotate_at;
        self.file = file;
    }

    /// Fixes the assistant's name from the command line, over whatever the file says.
    pub fn fix_assistant_name(&mut self, name: String) {
        self.fixed_assistant_name = Some(name.clone());
        self.assistant_name = name;
    }

    /// Where the settings file is: `<home>/config.json`.
    pub fn file_path(&self) -> PathBuf {
        self.home.join("config.json")
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
    use budd_core::crew::CrewBoard;

    #[test]
    fn the_file_lays_over_the_defaults() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("config.json");
        let config = Config::new(temp.path(), temp.path().join("claude")).with_file(&path);
        assert_eq!(config.assistant_name, "assistant");
        assert_eq!(config.assistant_model, "opus");
        assert_eq!(config.crew_context_limit, CrewBoard::DEFAULT_CONTEXT_LIMIT);
        assert_eq!(config.rotate_at, budd_core::hook::ROTATE_AT);
        assert_eq!(config.assistant_dir(), temp.path().join("assistant"));
        assert_eq!(config.state_path(), temp.path().join("state").join("budd-state.json"));
        assert_eq!(config.file_path(), path);
        std::fs::write(
            &path,
            r#"{"assistant_name": "budd", "crew": {"context_limit": 1000}, "assistant": {"model": "sonnet", "rotate_at": 5}, "other": 1}"#,
        )
        .expect("write");
        let config = Config::new(temp.path(), temp.path().join("claude")).with_file(&path);
        assert_eq!(config.assistant_name, "budd");
        assert_eq!(config.assistant_model, "sonnet");
        assert_eq!(config.crew_context_limit, 1000);
        assert_eq!(config.rotate_at, 5);
        assert_eq!(config.file.extra["other"], 1);
        // A broken file leaves the defaults.
        std::fs::write(&path, "garbage").expect("write");
        assert_eq!(
            Config::new(temp.path(), temp.path().join("claude"))
                .with_file(&path)
                .assistant_name,
            "assistant"
        );
        // The command line's name outranks the file's, now and after a reload.
        std::fs::write(&path, r#"{"assistant_name": "budd"}"#).expect("write");
        let mut config = Config::new(temp.path(), temp.path().join("claude"));
        config.fix_assistant_name("fixed".to_owned());
        let mut config = config.with_file(&path);
        assert_eq!(config.assistant_name, "fixed");
        config.apply(FileConfig::default());
        assert_eq!(config.assistant_name, "fixed");
        // A zero context limit never divides by zero.
        let mut zero = FileConfig::default();
        zero.crew.context_limit = 0;
        config.apply(zero);
        assert_eq!(config.crew_context_limit, 1);
    }
}
