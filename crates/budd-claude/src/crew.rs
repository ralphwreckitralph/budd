//! Starting, stopping and listing crew sessions and office chats (the rules of the assistant's `bin/crew`).
//!
//! Crew are the background sessions started for the human, one per ticket, in a project of theirs;
//! chats are sessions of the assistant's own folder opened from a free desk. The rules about them that
//! need no OS are [`budd_core::crew`]; this module is the adapter that reads the folders and the
//! settings, trusts the folder in Budd's `.claude.json`, and runs `claude --bg`. The checks are kept as
//! small functions so the engine, which never touches the OS itself, can run them through its host.

use crate::claude_cli::{self, Claude};
use crate::config::Config;
use crate::paths::{BuddHome, write_atomically};
use crate::procs;
use budd_core::account::FolderTrust;
use budd_core::crew::{self as rules, LiveSession, OfficeChat, Refusal, StartRequest};
use serde_json::Value;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

/// Claude Code's own settings file in a config folder.
pub const SETTINGS_FILE: &str = "settings.json";
/// Claude Code's per-account file, where folder trust is recorded.
pub const CLAUDE_JSON: &str = ".claude.json";
/// The permission mode a folder without a setting runs in.
pub const DEFAULT_MODE: &str = "default";

/// Why a crew session or chat could not be started, stopped or listed.
#[derive(Debug, Error)]
pub enum Error {
    /// The folder could not be resolved.
    #[error("{path} is not a folder Budd can start in: {source}")]
    NoSuchFolder {
        /// The folder as given.
        path: PathBuf,
        /// What the file system said.
        #[source]
        source: io::Error,
    },
    /// The folder has no `.git`.
    #[error("{0} is not a git repository")]
    NotARepository(PathBuf),
    /// The folder is the assistant's own: crew work in a project, never there.
    #[error("crew never run in the assistant's own folder ({0})")]
    AssistantFolder(PathBuf),
    /// One of the pure rules refused it.
    #[error("{0}")]
    Refused(#[from] Refusal),
    /// `claude` failed.
    #[error("{0}")]
    Claude(#[from] claude_cli::Error),
    /// Budd's `.claude.json` could not be read or written to trust the folder.
    #[error("trusting {folder} in {path}: {source}")]
    Trust {
        /// The folder to trust.
        folder: PathBuf,
        /// Budd's `.claude.json`.
        path: PathBuf,
        /// What went wrong.
        #[source]
        source: io::Error,
    },
    /// `stop` was asked for the assistant.
    #[error("stop is for crew; the assistant is not crew")]
    StopAssistant,
    /// `stop` was asked for a name no live session has.
    #[error("no live session is called \"{0}\"")]
    NotLive(String),
}

/// What a start did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    /// The session's name.
    pub name: String,
    /// The folder it started in.
    pub cwd: PathBuf,
    /// The `claude` arguments it started with.
    pub argv: Vec<String>,
    /// The permission mode it runs in.
    pub mode: String,
    /// What `claude --bg` printed before it returned.
    pub printed: Vec<String>,
}

/// `folder` as a crew folder: resolved to its real path, a git repository (a `.git` folder, or the
/// `.git` file of a worktree), and not `assistant_dir`, the assistant's own folder.
pub fn check_folder(folder: &Path, assistant_dir: &Path) -> Result<PathBuf, Error> {
    let real = plain(std::fs::canonicalize(folder).map_err(|source| Error::NoSuchFolder {
        path: folder.to_path_buf(),
        source,
    })?);
    if !real.is_dir() {
        return Err(Error::NoSuchFolder {
            path: folder.to_path_buf(),
            source: io::Error::new(io::ErrorKind::NotADirectory, "not a folder"),
        });
    }
    let assistant = std::fs::canonicalize(assistant_dir)
        .map(plain)
        .unwrap_or_else(|_| assistant_dir.to_path_buf());
    if real == assistant {
        return Err(Error::AssistantFolder(real));
    }
    if !real.join(".git").exists() {
        return Err(Error::NotARepository(real));
    }
    Ok(real)
}

/// `path` without the `\\?\` that Windows' canonicalize puts in front, so the trust key matches the
/// folder as Claude Code's own `process.cwd()` writes it; any other path as it is.
pub fn plain(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path
}

/// The permission mode the config folder's `settings.json` names under `permissions.defaultMode`,
/// or "default" when the file or the key is missing or unreadable. Whether that mode is allowed for
/// crew is [`budd_core::crew::check_start`]'s call.
pub fn permission_mode(config_dir: &Path) -> String {
    std::fs::read_to_string(config_dir.join(SETTINGS_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|settings| {
            settings
                .get("permissions")?
                .get("defaultMode")?
                .as_str()
                .filter(|mode| !mode.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| DEFAULT_MODE.to_owned())
}

/// Makes the config folder trust `folder`, so a background session started there never waits on the
/// trust question nobody can answer (`claude --bg` prints "Workspace not trusted" and exits). Merges
/// `projects.<folder>.hasTrustDialogAccepted = true` into `<config_dir>/.claude.json`, keeping every
/// other setting, and says whether it had to write. A `.claude.json` that is not JSON is left alone and
/// reported, as it is Claude Code's file.
pub fn ensure_trusted(config_dir: &Path, folder: &Path) -> io::Result<bool> {
    let path = config_dir.join(CLAUDE_JSON);
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("not JSON: {error}")))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Value::Object(Default::default()),
        Err(error) => return Err(error),
    };
    let key = folder.to_string_lossy().into_owned();
    if FolderTrust::trusted(&existing).iter().any(|trusted| *trusted == key) {
        return Ok(false);
    }
    let merged = FolderTrust::trusting(existing, &[key]);
    let text = serde_json::to_string_pretty(&merged).map_err(io::Error::other)?;
    write_atomically(&path, text.as_bytes())?;
    Ok(true)
}

/// Whether the config folder trusts `folder` already, without writing anything. A missing or
/// unreadable `.claude.json` trusts nothing.
pub fn ensure_trusted_check(config_dir: &Path, folder: &Path) -> bool {
    let key = folder.to_string_lossy();
    std::fs::read_to_string(config_dir.join(CLAUDE_JSON))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .is_some_and(|existing| FolderTrust::trusted(&existing).iter().any(|trusted| *trusted == key))
}

/// The background sessions on the bridge, as `budd_core::crew::live_sessions` picks them from
/// `claude agents --json`.
pub fn list(claude: &Claude, assistant: &str) -> Result<Vec<LiveSession>, Error> {
    let rows = crate::compat::agents_json::from_values(claude.agents(false)?);
    Ok(crate::compat::live_sessions(&rows, assistant, procs::is_alive))
}

/// The names of the sessions whose process is alive: the names a new session may not take.
pub fn live_names(sessions: &[LiveSession]) -> Vec<String> {
    sessions
        .iter()
        .filter(|session| !session.asleep())
        .map(|session| session.name.clone())
        .collect()
}

/// Starts the crew session `name` in `folder` with `brief` as its first message, as `crew start` did:
/// the folder is a git repository and not the assistant's, the name is free, the mode is the config
/// folder's own default (never bypass), the folder is trusted first, and the crew settings of
/// `config` (model, add dir, Chrome) apply.
pub fn start(
    claude: &Claude,
    config: &Config,
    home: &BuddHome,
    folder: &Path,
    name: &str,
    brief: &str,
) -> Result<Started, Error> {
    let cwd = check_folder(folder, &home.assistant_dir())?;
    let live = list(claude, &config.assistant_name)?;
    let mode = permission_mode(&claude.config_dir);
    let argv = start_argv(config, name, brief, &live_names(&live), &mode)?;
    trust(claude, &cwd)?;
    let printed = claude.start_bg(&cwd, &argv)?;
    Ok(Started {
        name: name.to_owned(),
        cwd,
        argv,
        mode,
        printed,
    })
}

/// The `claude` arguments a crew session starts with, or why it is refused: [`rules::check_start`]
/// over `config`'s crew settings.
pub fn start_argv<S: AsRef<str>>(
    config: &Config,
    name: &str,
    brief: &str,
    live_names: &[S],
    mode: &str,
) -> Result<Vec<String>, Refusal> {
    let request = StartRequest {
        name: name.to_owned(),
        brief: brief.to_owned(),
        model: config.crew.model.clone(),
        add_dir: config
            .crew
            .add_dir
            .as_ref()
            .map(|dir| dir.to_string_lossy().into_owned()),
        chrome: config.crew.chrome,
    };
    rules::check_start(&request, live_names, mode)
}

/// Starts a chat about `text`, as `crew chat` did: in the assistant's folder, named by the message's
/// first words ([`OfficeChat::name`]), with the assistant's own flags, and idle. The message itself is
/// not passed: the caller sends it once the session is listed, as any message to a crew session.
pub fn chat(claude: &Claude, config: &Config, home: &BuddHome, text: &str) -> Result<Started, Error> {
    let live = list(claude, &config.assistant_name)?;
    let taken = live_names(&live);
    let name = OfficeChat::name(text, &taken, &hhmm_now());
    let cwd = home.assistant_dir();
    let argv = chat_argv(config, home, &name, &taken)?;
    std::fs::create_dir_all(&cwd).map_err(|source| Error::NoSuchFolder {
        path: cwd.clone(),
        source,
    })?;
    trust(claude, &cwd)?;
    let printed = claude.start_bg(&cwd, &argv)?;
    Ok(Started {
        name,
        cwd,
        argv,
        mode: "auto".to_owned(),
        printed,
    })
}

/// The `claude` arguments the chat `name` starts with: [`rules::chat_flags`] over the assistant's
/// model, Budd's home as the added folder, and the crew Chrome setting.
pub fn chat_argv<S: AsRef<str>>(
    config: &Config,
    home: &BuddHome,
    name: &str,
    live_names: &[S],
) -> Result<Vec<String>, Refusal> {
    rules::chat_flags(
        name,
        &config.assistant.model,
        &home.root().to_string_lossy(),
        config.crew.chrome,
        live_names,
    )
}

/// Stops every live session called `name`, never the assistant, and gives the short ids stopped.
pub fn stop(claude: &Claude, assistant: &str, name: &str) -> Result<Vec<String>, Error> {
    if name == assistant {
        return Err(Error::StopAssistant);
    }
    let matches: Vec<String> = list(claude, assistant)?
        .into_iter()
        .filter(|session| session.name == name && !session.asleep())
        .map(|session| session.id)
        .collect();
    if matches.is_empty() {
        return Err(Error::NotLive(name.to_owned()));
    }
    for short_id in &matches {
        claude.stop(short_id)?;
    }
    Ok(matches)
}

/// Trusts `folder` in the config folder `claude` runs against.
fn trust(claude: &Claude, folder: &Path) -> Result<(), Error> {
    ensure_trusted(&claude.config_dir, folder)
        .map(|_| ())
        .map_err(|source| Error::Trust {
            folder: folder.to_path_buf(),
            path: claude.config_dir.join(CLAUDE_JSON),
            source,
        })
}

/// The time of day as "HHMM", in UTC, for naming a chat that has no words to take.
pub fn hhmm_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    hhmm(seconds)
}

/// "HHMM" of the time of day `seconds` since the epoch falls on, in UTC.
pub fn hhmm(seconds: u64) -> String {
    let of_day = seconds % 86_400;
    format!("{:02}{:02}", of_day / 3600, (of_day % 3600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn repo(root: &Path, name: &str) -> PathBuf {
        let folder = root.join(name);
        std::fs::create_dir_all(folder.join(".git")).unwrap();
        folder
    }

    #[test]
    fn a_crew_folder_is_a_repository_that_is_not_the_assistants() {
        let temp = tempfile::tempdir().unwrap();
        let assistant = temp.path().join("budd").join("assistant");
        std::fs::create_dir_all(assistant.join(".git")).unwrap();
        let project = repo(temp.path(), "project");
        let checked = check_folder(&project, &assistant).unwrap();
        assert_eq!(checked, std::fs::canonicalize(&project).unwrap());
        // A worktree's `.git` is a file, and still counts.
        let worktree = temp.path().join("worktree");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(".git"), "gitdir: ../project/.git/worktrees/x\n").unwrap();
        assert!(check_folder(&worktree, &assistant).is_ok());
        let plain = temp.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(matches!(
            check_folder(&plain, &assistant),
            Err(Error::NotARepository(_))
        ));
        assert!(matches!(
            check_folder(&assistant, &assistant),
            Err(Error::AssistantFolder(_))
        ));
        assert!(matches!(
            check_folder(&temp.path().join("missing"), &assistant),
            Err(Error::NoSuchFolder { .. })
        ));
        let file = temp.path().join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(matches!(
            check_folder(&file, &assistant),
            Err(Error::NoSuchFolder { .. })
        ));
    }

    #[test]
    fn the_permission_mode_is_the_settings_default_or_default() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(permission_mode(temp.path()), "default");
        std::fs::write(
            temp.path().join(SETTINGS_FILE),
            r#"{"permissions": {"defaultMode": "acceptEdits"}}"#,
        )
        .unwrap();
        assert_eq!(permission_mode(temp.path()), "acceptEdits");
        std::fs::write(temp.path().join(SETTINGS_FILE), r#"{"permissions": {}}"#).unwrap();
        assert_eq!(permission_mode(temp.path()), "default");
        std::fs::write(temp.path().join(SETTINGS_FILE), "garbage").unwrap();
        assert_eq!(permission_mode(temp.path()), "default");
        // Bypass is read as it is; the rule refuses it.
        std::fs::write(
            temp.path().join(SETTINGS_FILE),
            r#"{"permissions": {"defaultMode": "bypassPermissions"}}"#,
        )
        .unwrap();
        let config = Config::default();
        assert_eq!(
            start_argv(&config, "casbo-1", "do it", &["other"], &permission_mode(temp.path())),
            Err(Refusal::BypassPermissions)
        );
    }

    #[test]
    fn the_start_arguments_follow_the_config() {
        let mut config = Config::default();
        let argv = start_argv(&config, "casbo-1", " do it ", &Vec::<String>::new(), "default").unwrap();
        assert_eq!(
            argv,
            ["--bg", "--name", "casbo-1", "--permission-mode", "default", "do it"]
        );
        config.crew.model = Some("sonnet".to_owned());
        config.crew.add_dir = Some(PathBuf::from("/w"));
        config.crew.chrome = true;
        let argv = start_argv(&config, "casbo-1", "do it", &Vec::<String>::new(), "acceptEdits").unwrap();
        assert_eq!(
            argv,
            [
                "--bg",
                "--name",
                "casbo-1",
                "--permission-mode",
                "acceptEdits",
                "--model",
                "sonnet",
                "--add-dir",
                "/w",
                "--chrome",
                "do it"
            ]
        );
        assert_eq!(
            start_argv(&config, "casbo-1", "do it", &["casbo-1"], "default"),
            Err(Refusal::NameInUse("casbo-1".to_owned()))
        );
        assert!(matches!(
            start_argv(&config, "Assistant", "do it", &Vec::<String>::new(), "default"),
            Err(Refusal::NotACrewName(_))
        ));
        assert_eq!(
            start_argv(&config, "casbo-1", "  ", &Vec::<String>::new(), "default"),
            Err(Refusal::BlankBrief)
        );
        let home = BuddHome::at("/Users/ralph/.budd");
        let chat = chat_argv(&config, &home, "chat-hello", &Vec::<String>::new()).unwrap();
        assert_eq!(
            chat,
            [
                "--bg",
                "--name",
                "chat-hello",
                "--model",
                "opus",
                "--permission-mode",
                "auto",
                "--add-dir",
                "/Users/ralph/.budd",
                "--chrome"
            ]
        );
        assert!(matches!(
            chat_argv(&config, &home, "hello", &Vec::<String>::new()),
            Err(Refusal::NotAChatName(_))
        ));
    }

    #[test]
    fn trusting_a_folder_merges_into_claude_json_once() {
        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join("claude");
        let folder = Path::new("/w/project");
        assert!(!ensure_trusted_check(&config_dir, folder));
        assert!(ensure_trusted(&config_dir, folder).unwrap(), "written the first time");
        assert!(ensure_trusted_check(&config_dir, folder));
        let path = config_dir.join(CLAUDE_JSON);
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["projects"]["/w/project"]["hasTrustDialogAccepted"], true);
        assert!(!ensure_trusted(&config_dir, folder).unwrap(), "already trusted");
        // Other settings in the file stay.
        std::fs::write(
            &path,
            json!({"userID": "u", "projects": {"/w/project": {"hasTrustDialogAccepted": true, "allowedTools": ["Bash"]}}}).to_string(),
        )
        .unwrap();
        assert!(ensure_trusted(&config_dir, Path::new("/w/other")).unwrap());
        let written: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["userID"], "u");
        assert_eq!(written["projects"]["/w/project"]["allowedTools"], json!(["Bash"]));
        assert_eq!(written["projects"]["/w/other"]["hasTrustDialogAccepted"], true);
        // A file that is not JSON is Claude Code's problem, not overwritten.
        std::fs::write(&path, "{broken").unwrap();
        let error = ensure_trusted(&config_dir, Path::new("/w/x")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{broken");
    }

    #[test]
    fn live_names_are_the_awake_ones_and_the_clock_reads_hhmm() {
        let sessions = vec![
            LiveSession {
                id: "a".into(),
                session_id: "a".into(),
                name: "casbo-1".into(),
                pid: Some(1),
                cwd: "/w".into(),
                status: None,
                state: None,
                waiting_for: None,
                started_at: 0.0,
            },
            LiveSession {
                id: "b".into(),
                session_id: "b".into(),
                name: "casbo-2".into(),
                pid: None,
                cwd: "/w".into(),
                status: None,
                state: None,
                waiting_for: None,
                started_at: 0.0,
            },
        ];
        assert_eq!(live_names(&sessions), ["casbo-1"]);
        assert_eq!(hhmm(0), "0000");
        assert_eq!(hhmm(14 * 3600 + 32 * 60 + 59), "1432");
        assert_eq!(hhmm(86_400 + 60), "0001");
        assert_eq!(hhmm_now().len(), 4);
        assert_eq!(plain(PathBuf::from(r"\\?\C:\w\x")), PathBuf::from(r"C:\w\x"));
        assert_eq!(
            plain(PathBuf::from(r"\\?\UNC\srv\share")),
            PathBuf::from(r"\\srv\share")
        );
        assert_eq!(plain(PathBuf::from("/w/x")), PathBuf::from("/w/x"));
    }

    #[test]
    fn start_and_chat_run_the_fake_claude_and_trust_first() {
        let Some(exe) = crate::test_support::fake_claude() else {
            return;
        };
        let temp = tempfile::tempdir().unwrap();
        let home = BuddHome::at(temp.path().join("budd"));
        let claude = Claude::new(&exe, home.claude_dir());
        std::fs::create_dir_all(home.claude_dir()).unwrap();
        let started_log = temp.path().join("started.log");
        std::fs::write(
            home.claude_dir().join("fake-claude.env"),
            format!("FAKE_CLAUDE_STARTED={}\n", started_log.display()),
        )
        .unwrap();
        let project = repo(temp.path(), "project");
        let config = Config::default();
        let started = start(&claude, &config, &home, &project, "casbo-1", "do it").unwrap();
        assert_eq!(started.name, "casbo-1");
        assert_eq!(started.mode, "default");
        assert_eq!(started.cwd, std::fs::canonicalize(&project).unwrap());
        let log = std::fs::read_to_string(&started_log).unwrap();
        assert!(log.contains("--name casbo-1"), "{log}");
        let trusted: Value =
            serde_json::from_str(&std::fs::read_to_string(home.claude_dir().join(CLAUDE_JSON)).unwrap()).unwrap();
        assert_eq!(
            trusted["projects"][started.cwd.to_string_lossy().as_ref()]["hasTrustDialogAccepted"],
            true
        );
        let chat = chat(&claude, &config, &home, "Where is the dev server?").unwrap();
        assert_eq!(chat.name, "chat-where-is-the-dev");
        assert_eq!(chat.cwd, home.assistant_dir());
        assert!(home.assistant_dir().is_dir());
        // The assistant's folder is no crew folder, and a plain folder is none either.
        assert!(matches!(
            start(&claude, &config, &home, &home.assistant_dir(), "x-1", "go"),
            Err(Error::AssistantFolder(_))
        ));
        assert!(matches!(
            stop(&claude, "assistant", "assistant"),
            Err(Error::StopAssistant)
        ));
        assert!(matches!(stop(&claude, "assistant", "nobody"), Err(Error::NotLive(_))));
        assert!(list(&claude, "assistant").unwrap().is_empty());
    }
}
