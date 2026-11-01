//! `<home>/config.json`: what the human set by hand, read by the daemon, the CLI and the app alike.
//!
//! The file is the one place Budd's settings live, outside the Claude config folder so a `claude`
//! update never touches it. A missing file means the defaults; a key this build does not know is kept
//! as it was and written back, so an older and a newer Budd can share the file. Writes go through a
//! temporary file beside it, so a reader never sees half a file.

use budd_core::crew::CrewBoard;
use budd_core::hook::ROTATE_AT;
use budd_core::keepalive::Input as Keepalive;
use budd_core::reply::PilotMode;
use budd_core::reply::{TicketProject, TicketTable};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// The file's format version this build writes.
pub const VERSION: u64 = 1;
/// The placeholder in a project's command that the ticket key replaces.
pub const KEY_PLACEHOLDER: &str = "{key}";

/// Why the file could not be read or written.
#[derive(Debug, Error)]
pub enum Error {
    /// The file is there but is not JSON, or not the shape this module knows.
    #[error("{path} is not a Budd config file: {source}")]
    Malformed {
        /// The file.
        path: PathBuf,
        /// What the parser said.
        #[source]
        source: serde_json::Error,
    },
    /// Reading or writing failed.
    #[error("{what} {path}: {source}")]
    Io {
        /// "reading" or "writing".
        what: &'static str,
        /// The file.
        path: PathBuf,
        /// What went wrong.
        #[source]
        source: io::Error,
    },
}

/// A project Budd starts tickets in: the Jira prefixes it claims, where it is, and the command a ticket
/// starts with there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    /// The Jira prefixes whose tickets start here, such as `["CASBO"]`; matched without regard to case.
    pub prefixes: Vec<String>,
    /// How the human calls the project ("gamecenter"); the folder's last name when empty.
    pub name: String,
    /// The project folder, a git repository; "~" at the front means the home directory.
    pub folder: PathBuf,
    /// The slash command a ticket starts with, `{key}` standing for the ticket key.
    pub command: String,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            prefixes: Vec::new(),
            name: String::new(),
            folder: PathBuf::new(),
            command: "/budd:do-it {key}".to_owned(),
        }
    }
}

impl Project {
    /// The project's name: as set, else the folder's last name.
    pub fn display_name(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        self.folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.folder.to_string_lossy().into_owned())
    }

    /// The command that starts `key` here: the template with `{key}` replaced, or the key appended
    /// when the template has no placeholder ("/lets-do-it" starts "/lets-do-it CASBO-1").
    pub fn command_for(&self, key: &str) -> String {
        ticket_command(&self.command, key)
    }
}

/// `template` with `{key}` replaced by `key`, or `key` appended after a space when there is no
/// placeholder.
pub fn ticket_command(template: &str, key: &str) -> String {
    if template.contains(KEY_PLACEHOLDER) {
        template.replace(KEY_PLACEHOLDER, key)
    } else {
        format!("{} {key}", template.trim_end())
    }
}

/// How crew sessions start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Crew {
    /// The context size at which a crew session counts as full.
    pub context_limit: u64,
    /// The model crew start on, when not the folder's default.
    pub model: Option<String>,
    /// A folder every crew session gets added to its reach.
    pub add_dir: Option<PathBuf>,
    /// Whether crew start with Claude in Chrome.
    pub chrome: bool,
}

impl Default for Crew {
    fn default() -> Self {
        Crew {
            context_limit: CrewBoard::DEFAULT_CONTEXT_LIMIT,
            model: None,
            add_dir: None,
            chrome: false,
        }
    }
}

/// How the assistant's own session runs and when the keep-alive replaces it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Assistant {
    /// The model a fresh assistant starts on.
    pub model: String,
    /// The context size at which a quiet assistant is replaced.
    pub rotate_at: u64,
    /// How many idle minutes get an idle assistant replaced.
    pub idle_minutes: u64,
    /// How many quiet minutes a full assistant needs before it is replaced.
    pub quiet_minutes: u64,
}

impl Default for Assistant {
    fn default() -> Self {
        Assistant {
            model: "opus".to_owned(),
            rotate_at: ROTATE_AT,
            idle_minutes: Keepalive::IDLE_MIN as u64,
            quiet_minutes: Keepalive::QUIET_MIN as u64,
        }
    }
}

/// Who answers the crew's `Decide:` lines, and how long they stand first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pilot {
    /// The mode Budd starts in when he has no state file saying otherwise.
    pub mode: PilotMode,
    /// How many seconds a `Decide:` line stands before autopilot answers it.
    pub decide_delay_seconds: u64,
}

impl Default for Pilot {
    fn default() -> Self {
        Pilot {
            mode: PilotMode::Human,
            decide_delay_seconds: 8,
        }
    }
}

/// Whether Budd speaks, and in which voice.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Voice {
    /// Whether he speaks at all.
    pub enabled: bool,
    /// The platform's voice identifier, or the default voice.
    pub identifier: Option<String>,
}

/// Which of the kit's gates run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gates {
    /// The destructive-action gate.
    pub destructive: bool,
    /// The scope guard.
    pub guard: bool,
    /// The context budget gate.
    pub context_budget: bool,
    /// The rethinker.
    pub rethinker: bool,
    /// Lint on every edit.
    pub lint_edit: bool,
}

impl Default for Gates {
    fn default() -> Self {
        Gates {
            destructive: true,
            guard: true,
            context_budget: true,
            rethinker: true,
            lint_edit: true,
        }
    }
}

/// The context budget gate's thresholds, in tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextBudget {
    /// From here the gate warns.
    pub warn: u64,
    /// From here the gate stops the session.
    pub stop: u64,
}

impl Default for ContextBudget {
    fn default() -> Self {
        ContextBudget {
            warn: 120_000,
            stop: CrewBoard::DEFAULT_CONTEXT_LIMIT,
        }
    }
}

/// What the kit keeps as evidence of a ticket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EvidenceConfig {
    /// Keep a manifest per ticket.
    pub manifest: bool,
    /// Shoot before and after frames.
    pub shots: bool,
}

impl Default for EvidenceConfig {
    fn default() -> Self {
        EvidenceConfig {
            manifest: true,
            shots: true,
        }
    }
}

/// The learning loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Learning {
    /// Whether a finished crew session gets a lesson.
    pub enabled: bool,
    /// How many quiet minutes a finished session waits before its lesson starts.
    pub quiet_minutes: u64,
}

impl Default for Learning {
    fn default() -> Self {
        Learning {
            enabled: true,
            quiet_minutes: 10,
        }
    }
}

/// The bundled pipeline's settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Kit {
    /// Which gates run.
    pub gates: Gates,
    /// The context budget gate's thresholds.
    pub context_budget: ContextBudget,
    /// What is kept as evidence.
    pub evidence: EvidenceConfig,
    /// The learning loop.
    pub learning: Learning,
}

/// The whole file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The file's format version.
    pub version: u64,
    /// The `claude` binary, when it is somewhere Budd would not look.
    pub claude_path: Option<PathBuf>,
    /// The name of the assistant's session.
    pub assistant_name: String,
    /// The projects Start Tickets can start a key in.
    pub projects: Vec<Project>,
    /// How crew start.
    pub crew: Crew,
    /// The assistant's session and its keep-alive.
    pub assistant: Assistant,
    /// Autopilot.
    pub pilot: Pilot,
    /// Speech.
    pub voice: Voice,
    /// Whether Budd makes sounds.
    pub sounds: bool,
    /// Whether the app opens at login.
    pub open_at_login: bool,
    /// The kit.
    pub kit: Kit,
    /// Keys this build does not know, kept so a newer Budd's settings survive an older one's write.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: VERSION,
            claude_path: None,
            assistant_name: "assistant".to_owned(),
            projects: Vec::new(),
            crew: Crew::default(),
            assistant: Assistant::default(),
            pilot: Pilot::default(),
            voice: Voice::default(),
            sounds: true,
            open_at_login: false,
            kit: Kit::default(),
            extra: Map::new(),
        }
    }
}

impl Config {
    /// The file at `path`, or the defaults when there is none. A file that is there but unreadable, or
    /// not a config file, is an error: the defaults must never silently replace a human's settings.
    pub fn load(path: &Path) -> Result<Config, Error> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(source) => {
                return Err(Error::Io {
                    what: "reading",
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        Config::parse(&text).map_err(|source| Error::Malformed {
            path: path.to_path_buf(),
            source,
        })
    }

    /// The config in `text`.
    pub fn parse(text: &str) -> Result<Config, serde_json::Error> {
        let mut config: Config = serde_json::from_str(text)?;
        config.version = VERSION;
        Ok(config)
    }

    /// Writes the file at `path`, pretty printed, creating its folder, through a temporary file beside
    /// it renamed into place.
    pub fn save(&self, path: &Path) -> Result<(), Error> {
        let io_error = |source: io::Error| Error::Io {
            what: "writing",
            path: path.to_path_buf(),
            source,
        };
        let mut text = serde_json::to_string_pretty(self).map_err(|source| Error::Malformed {
            path: path.to_path_buf(),
            source,
        })?;
        text.push('\n');
        crate::paths::write_atomically(path, text.as_bytes()).map_err(io_error)?;
        Ok(())
    }

    /// The config as JSON, as `settings.get` reports it.
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /// This config with `patch` laid over it: objects merge key by key, anything else replaces, and a
    /// `null` removes a key, so a client can change one setting without sending the rest. The result
    /// must still read as a config, or the patch is refused and nothing changes.
    pub fn patched(&self, patch: &Value) -> Result<Config, serde_json::Error> {
        let mut value = self.to_value();
        merge(&mut value, patch);
        let mut config: Config = serde_json::from_value(value)?;
        config.version = VERSION;
        Ok(config)
    }

    /// The ticket table the projects make: each prefix upper-cased, pointing at its project's folder
    /// (as written, "~" and all) and command template. A later project claiming a prefix again wins,
    /// and a project without a folder claims nothing.
    pub fn ticket_table(&self) -> TicketTable {
        let mut table = TicketTable::new();
        for project in &self.projects {
            if project.folder.as_os_str().is_empty() {
                continue;
            }
            for prefix in &project.prefixes {
                let prefix = prefix.trim().to_uppercase();
                if prefix.is_empty() {
                    continue;
                }
                table.insert(
                    prefix,
                    TicketProject::new(project.folder.to_string_lossy().into_owned(), project.command.clone()),
                );
            }
        }
        table
    }

    /// The project that claims `prefix`, if any; the last one in the list when several do.
    pub fn project_for(&self, prefix: &str) -> Option<&Project> {
        self.projects
            .iter()
            .rev()
            .find(|project| project.prefixes.iter().any(|p| p.trim().eq_ignore_ascii_case(prefix)))
    }
}

/// Lays `patch` over `target`: objects merge key by key, `null` removes, anything else replaces.
fn merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                if value.is_null() {
                    target.remove(key);
                } else {
                    match target.get_mut(key) {
                        Some(existing) => merge(existing, value),
                        None => {
                            target.insert(key.clone(), value.clone());
                        }
                    }
                }
            }
        }
        (target, patch) => *target = patch.clone(),
    }
}

/// `path` with a leading "~" or "~/" replaced by `home`; any other path as it is.
pub fn expand_tilde(path: &Path, home: Option<&Path>) -> PathBuf {
    let Some(home) = home else {
        return path.to_path_buf();
    };
    let text = path.to_string_lossy();
    if text == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\")) {
        return home.join(rest);
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_missing_file_is_the_defaults_and_they_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("deep").join("config.json");
        let config = Config::load(&path).unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.version, 1);
        assert_eq!(config.assistant_name, "assistant");
        assert_eq!(config.assistant.model, "opus");
        assert_eq!(config.assistant.rotate_at, 400_000);
        assert_eq!(config.assistant.idle_minutes, 30);
        assert_eq!(config.assistant.quiet_minutes, 2);
        assert_eq!(config.crew.context_limit, 150_000);
        assert_eq!(config.pilot.decide_delay_seconds, 8);
        assert_eq!(config.pilot.mode, PilotMode::Human);
        assert!(config.kit.gates.destructive && config.kit.learning.enabled);
        config.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.ends_with('\n'));
        assert_eq!(Config::load(&path).unwrap(), config);
        let names: Vec<String> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["config.json"], "no temporary file is left behind");
    }

    #[test]
    fn unknown_keys_survive_a_write_and_partial_files_fill_in() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"assistant_name": "budd", "crew": {"chrome": true}, "future": {"thing": 1}, "version": 7}"#,
        )
        .unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.assistant_name, "budd");
        assert!(config.crew.chrome);
        assert_eq!(config.crew.context_limit, 150_000, "the rest of crew is the default");
        assert_eq!(config.version, VERSION, "the version is this build's");
        assert_eq!(config.extra["future"], json!({"thing": 1}));
        config.save(&path).unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["future"]["thing"], 1);
        assert_eq!(written["version"], 1);
        assert_eq!(written["kit"]["gates"]["lint_edit"], true);
    }

    #[test]
    fn a_broken_file_is_an_error_not_the_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.json");
        std::fs::write(&path, "{not json").unwrap();
        assert!(matches!(Config::load(&path), Err(Error::Malformed { .. })));
        std::fs::write(&path, r#"{"crew": {"context_limit": "lots"}}"#).unwrap();
        assert!(matches!(Config::load(&path), Err(Error::Malformed { .. })));
    }

    #[test]
    fn a_patch_merges_objects_replaces_the_rest_and_is_checked() {
        let config = Config::default();
        let patched = config
            .patched(&json!({
                "assistant": {"model": "sonnet"},
                "crew": {"model": "opus", "chrome": true},
                "sounds": false,
                "projects": [{"prefixes": ["casbo"], "folder": "~/webserver/gamecenter"}],
                "new_thing": "kept"
            }))
            .unwrap();
        assert_eq!(patched.assistant.model, "sonnet");
        assert_eq!(patched.assistant.rotate_at, 400_000, "the rest of assistant stays");
        assert_eq!(patched.crew.model.as_deref(), Some("opus"));
        assert!(patched.crew.chrome);
        assert!(!patched.sounds);
        assert_eq!(patched.projects.len(), 1);
        assert_eq!(patched.projects[0].command, "/budd:do-it {key}");
        assert_eq!(patched.extra["new_thing"], "kept");
        // A null removes: the model goes back to none, an unknown key goes away.
        let cleared = patched
            .patched(&json!({"crew": {"model": null}, "new_thing": null}))
            .unwrap();
        assert_eq!(cleared.crew.model, None);
        assert!(!cleared.extra.contains_key("new_thing"));
        // A patch that breaks the shape is refused.
        assert!(config.patched(&json!({"crew": "none"})).is_err());
        assert!(config.patched(&json!({"pilot": {"mode": "robot"}})).is_err());
    }

    #[test]
    fn projects_make_the_ticket_table_and_their_commands() {
        let config = Config {
            projects: vec![
                Project {
                    prefixes: vec!["casbo".to_owned(), " GC ".to_owned()],
                    name: String::new(),
                    folder: PathBuf::from("~/webserver/gamecenter"),
                    command: "/lets-do-it-v2".to_owned(),
                },
                Project {
                    prefixes: vec!["GNX".to_owned()],
                    name: "portal".to_owned(),
                    folder: PathBuf::from("/w/globalnet-portal"),
                    command: "/budd:do-it {key}".to_owned(),
                },
                Project {
                    prefixes: vec!["NOPE".to_owned()],
                    name: String::new(),
                    folder: PathBuf::new(),
                    command: "/x".to_owned(),
                },
            ],
            ..Config::default()
        };
        let table = config.ticket_table();
        assert_eq!(
            table.keys().cloned().collect::<Vec<_>>(),
            ["CASBO", "GC", "GNX"],
            "upper-cased and trimmed; a project without a folder claims nothing"
        );
        assert_eq!(table["CASBO"].folder, "~/webserver/gamecenter");
        assert_eq!(table["CASBO"].command, "/lets-do-it-v2");
        assert_eq!(
            config.project_for("casbo").map(|p| p.display_name()).as_deref(),
            Some("gamecenter")
        );
        assert_eq!(
            config.project_for("GNX").map(|p| p.display_name()).as_deref(),
            Some("portal")
        );
        assert_eq!(config.project_for("OPS"), None);
        assert_eq!(config.projects[0].command_for("CASBO-1"), "/lets-do-it-v2 CASBO-1");
        assert_eq!(config.projects[1].command_for("GNX-2"), "/budd:do-it GNX-2");
        assert_eq!(ticket_command("/go {key} now {key}", "A-1"), "/go A-1 now A-1");
    }

    #[test]
    fn a_tilde_expands_to_the_home_given() {
        let home = Path::new("/Users/ralph");
        assert_eq!(
            expand_tilde(Path::new("~/webserver/x"), Some(home)),
            PathBuf::from("/Users/ralph/webserver/x")
        );
        assert_eq!(expand_tilde(Path::new("~"), Some(home)), PathBuf::from("/Users/ralph"));
        assert_eq!(expand_tilde(Path::new("/abs/x"), Some(home)), PathBuf::from("/abs/x"));
        assert_eq!(
            expand_tilde(Path::new("~other/x"), Some(home)),
            PathBuf::from("~other/x")
        );
        assert_eq!(expand_tilde(Path::new("~/x"), None), PathBuf::from("~/x"));
    }
}
