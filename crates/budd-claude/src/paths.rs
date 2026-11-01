//! Where Budd keeps its things on each platform: the Budd folder, its Claude config dir, state, drops, run and logs.
//!
//! Everything of Budd's lives under one folder, `~/.budd` (`%USERPROFILE%\.budd` on Windows), which the
//! `BUDD_HOME` variable overrides. Inside it, `claude/` is the `CLAUDE_CONFIG_DIR` of Budd's own account,
//! apart from the human's `~/.claude`.

use std::ffi::OsString;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// The variable that moves Budd's folder somewhere else, for tests and for a second Budd.
pub const HOME_VAR: &str = "BUDD_HOME";
/// The folder's name under the home directory.
pub const FOLDER_NAME: &str = ".budd";

/// Why Budd's folder could not be placed: neither `BUDD_HOME` nor the home directory is set.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("neither BUDD_HOME nor the home directory ({home_var}) is set")]
pub struct NoHome {
    /// The variable the platform keeps the home directory in: `HOME` or `USERPROFILE`.
    pub home_var: &'static str,
}

pub use crate::ipc::Endpoint;

/// Budd's folder and every path under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuddHome {
    root: PathBuf,
}

/// The variable the platform keeps the home directory in.
#[cfg(windows)]
const PLATFORM_HOME_VAR: &str = "USERPROFILE";
/// The variable the platform keeps the home directory in.
#[cfg(not(windows))]
const PLATFORM_HOME_VAR: &str = "HOME";

impl BuddHome {
    /// Budd's folder for this process: `BUDD_HOME` when set and not empty, else `.budd` under the home
    /// directory (`HOME` on Unix, `USERPROFILE` on Windows).
    pub fn resolve() -> Result<BuddHome, NoHome> {
        Self::from_vars(std::env::var_os(HOME_VAR), std::env::var_os(PLATFORM_HOME_VAR))
    }

    /// The folder the two variables name: `budd_home` as it is when set and not empty, else `.budd`
    /// under `home`. This is what [`BuddHome::resolve`] reads from the environment, kept apart so it
    /// can be tested without touching the process environment.
    pub fn from_vars(budd_home: Option<OsString>, home: Option<OsString>) -> Result<BuddHome, NoHome> {
        if let Some(root) = budd_home.filter(|value| !value.is_empty()) {
            return Ok(BuddHome::at(root));
        }
        match home.filter(|value| !value.is_empty()) {
            Some(home) => Ok(BuddHome::at(PathBuf::from(home).join(FOLDER_NAME))),
            None => Err(NoHome {
                home_var: PLATFORM_HOME_VAR,
            }),
        }
    }

    /// Budd's folder at `root`, wherever that is.
    pub fn at(root: impl Into<PathBuf>) -> BuddHome {
        BuddHome { root: root.into() }
    }

    /// The folder itself.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `claude/`: the `CLAUDE_CONFIG_DIR` of Budd's own account.
    pub fn claude_dir(&self) -> PathBuf {
        self.root.join("claude")
    }

    /// `assistant/`: the folder the assistant's own session runs in.
    pub fn assistant_dir(&self) -> PathBuf {
        self.root.join("assistant")
    }

    /// `pipeline/`: the local plugin marketplace, with the kit's git repository inside.
    pub fn pipeline_dir(&self) -> PathBuf {
        self.root.join("pipeline")
    }

    /// `profiles/`: one project profile per folder Budd works in.
    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join("profiles")
    }

    /// `modules/`: optional installs such as the evidence shots' browser.
    pub fn modules_dir(&self) -> PathBuf {
        self.root.join("modules")
    }

    /// `config.json`: the app's settings, outside the Claude config dir.
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.json")
    }

    /// `state/`: what the daemon keeps between runs.
    pub fn state_dir(&self) -> PathBuf {
        self.root.join("state")
    }

    /// `state/sessions/`: one state file and reply per session, written by the hook.
    pub fn sessions_dir(&self) -> PathBuf {
        self.state_dir().join("sessions")
    }

    /// `drops/`: copies of the files and pictures dropped on a desk.
    pub fn drops_dir(&self) -> PathBuf {
        self.root.join("drops")
    }

    /// `run/`: the socket and the pid files; private to the user.
    pub fn run_dir(&self) -> PathBuf {
        self.root.join("run")
    }

    /// `logs/`: the daemon's logs.
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Where the daemon listens on this platform.
    pub fn endpoint(&self) -> Endpoint {
        #[cfg(windows)]
        {
            let user = std::env::var("USERNAME").unwrap_or_default();
            Endpoint::NamedPipe(pipe_name(&user))
        }
        #[cfg(not(windows))]
        {
            Endpoint::UnixSocket(self.run_dir().join("budd.sock"))
        }
    }
}

/// The named pipe for `user`: `\\.\pipe\budd-<hex of a hash of the name>`. The same binary on both
/// ends computes it, so the hash only has to be stable within one build.
pub fn pipe_name(user: &str) -> String {
    let mut hasher = DefaultHasher::new();
    user.hash(&mut hasher);
    format!(r"\\.\pipe\budd-{:016x}", hasher.finish())
}

/// Creates `path` and its parents when missing, and on Unix makes it the user's alone (mode 0700),
/// also when it already existed with looser permissions. Windows folders under the user's profile are
/// private by inheritance; the pipe's own security descriptor is the IPC module's concern.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)?.permissions();
        if permissions.mode() & 0o777 != 0o700 {
            permissions.set_mode(0o700);
            std::fs::set_permissions(path, permissions)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budd_home_overrides_the_home_directory() {
        let temp = tempfile::tempdir().unwrap();
        let home = BuddHome::from_vars(Some(temp.path().into()), Some("/Users/nobody".into())).unwrap();
        assert_eq!(home.root(), temp.path());
        assert_eq!(home.claude_dir(), temp.path().join("claude"));
        assert_eq!(home.assistant_dir(), temp.path().join("assistant"));
        assert_eq!(home.pipeline_dir(), temp.path().join("pipeline"));
        assert_eq!(home.profiles_dir(), temp.path().join("profiles"));
        assert_eq!(home.modules_dir(), temp.path().join("modules"));
        assert_eq!(home.config_path(), temp.path().join("config.json"));
        assert_eq!(home.state_dir(), temp.path().join("state"));
        assert_eq!(home.sessions_dir(), temp.path().join("state").join("sessions"));
        assert_eq!(home.drops_dir(), temp.path().join("drops"));
        assert_eq!(home.run_dir(), temp.path().join("run"));
        assert_eq!(home.logs_dir(), temp.path().join("logs"));
    }

    #[test]
    fn without_budd_home_it_is_dot_budd_under_the_home_directory() {
        let home = BuddHome::from_vars(None, Some("/Users/nobody".into())).unwrap();
        assert_eq!(home.root(), Path::new("/Users/nobody").join(".budd"));
        // An empty override is no override.
        let home = BuddHome::from_vars(Some("".into()), Some("/Users/nobody".into())).unwrap();
        assert_eq!(home.root(), Path::new("/Users/nobody").join(".budd"));
        assert!(BuddHome::from_vars(None, None).is_err());
        assert!(BuddHome::from_vars(Some("".into()), Some("".into())).is_err());
    }

    #[test]
    fn the_endpoint_is_in_the_run_folder_or_a_pipe_named_for_the_user() {
        let temp = tempfile::tempdir().unwrap();
        let home = BuddHome::at(temp.path());
        match home.endpoint() {
            Endpoint::UnixSocket(path) => assert_eq!(path, temp.path().join("run").join("budd.sock")),
            Endpoint::NamedPipe(name) => assert!(name.starts_with(r"\\.\pipe\budd-")),
        }
        let name = pipe_name("ralph");
        assert!(name.starts_with(r"\\.\pipe\budd-"));
        assert_eq!(name.len(), r"\\.\pipe\budd-".len() + 16);
        assert_eq!(name, pipe_name("ralph"));
        assert_ne!(name, pipe_name("other"));
    }

    #[test]
    fn a_private_dir_is_created_for_the_user_alone() {
        let temp = tempfile::tempdir().unwrap();
        let run = temp.path().join("deep").join("run");
        ensure_private_dir(&run).unwrap();
        assert!(run.is_dir());
        // Doing it again is fine.
        ensure_private_dir(&run).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&run).unwrap().permissions().mode() & 0o777, 0o700);
            // A folder that was left open is closed.
            std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755)).unwrap();
            ensure_private_dir(&run).unwrap();
            assert_eq!(std::fs::metadata(&run).unwrap().permissions().mode() & 0o777, 0o700);
        }
    }
}
