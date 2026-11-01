//! Finding and running the user's `claude` binary with a clean environment.
//!
//! Budd drives the Claude Code the human installed, unmodified, against Budd's own config folder. This
//! module finds that binary in the places the installers put it, builds the environment its children
//! get (the user's login never comes from a variable, so none that could outrank it is passed on), and
//! runs the subcommands Budd needs: version, agents, auth status, mcp, stop, and starting a background
//! session. The parsers are kept apart from the processes so they can be tested on recorded output.

use budd_core::account::{AccountStatus, McpServerStatus};
use semver::Version;
use serde_json::Value;
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

/// The binary's file name on this platform.
pub const BINARY: &str = if cfg!(windows) { "claude.exe" } else { "claude" };

/// The variables never passed to a child: each makes Claude Code use a key, a token, a profile or a
/// cloud provider instead of the subscription login in Budd's config folder.
pub const DROPPED_VARS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_PROFILE",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
];

/// How long `claude --bg` may take to print its session and return.
pub const START_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a login shell may take to say where `claude` is.
#[cfg(unix)]
const SHELL_TIMEOUT: Duration = Duration::from_secs(5);
/// How many characters of a failure's last line are kept.
const LAST_LINE: usize = 160;

/// What went wrong running `claude`.
#[derive(Debug, Error)]
pub enum Error {
    /// The binary could not be started at all: missing, not executable, or the system refused.
    #[error("claude could not be started: {0}")]
    Spawn(#[from] io::Error),
    /// The command ran and failed; `last_line` is the last thing it printed.
    #[error("claude {command} failed ({}): {last_line}", status_text(status))]
    Failed {
        /// The subcommand, such as "stop abc123".
        command: String,
        /// Its exit status, or `None` when a signal ended it.
        status: Option<i32>,
        /// The last non-empty line it printed, at most 160 characters.
        last_line: String,
    },
    /// The command printed something that is not JSON.
    #[error("claude {command} printed invalid JSON: {source}")]
    Json {
        /// The subcommand.
        command: String,
        /// What the parser said.
        #[source]
        source: serde_json::Error,
    },
    /// The command printed JSON of another shape than this module expects.
    #[error("claude {command} printed something other than {expected}")]
    Shape {
        /// The subcommand.
        command: String,
        /// What was expected, such as "a JSON array".
        expected: &'static str,
    },
    /// The command did not return in time.
    #[error("claude {command} did not return within {} s", timeout.as_secs())]
    Timeout {
        /// The subcommand.
        command: String,
        /// How long it was given.
        timeout: Duration,
    },
}

fn status_text(status: &Option<i32>) -> String {
    match status {
        Some(code) => format!("status {code}"),
        None => "ended by a signal".to_owned(),
    }
}

/// The last non-empty line of `output`, cut to 160 characters, or a stand-in when it printed nothing.
pub fn last_line(output: &str) -> String {
    output
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(|line| line.chars().take(LAST_LINE).collect())
        .unwrap_or_else(|| "claude printed nothing".to_owned())
}

/// Whether `path` is a file this process may run. On Windows every file counts; the shim rules decide
/// what to do with a `.cmd`.
pub fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// The `.exe` beside a `.cmd` or `.bat` shim, when there is one: a shim needs `cmd.exe` to run and
/// hides the real process, so the binary is used instead. Any other path is returned as it is.
pub fn without_shim(path: &Path) -> PathBuf {
    let is_shim = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"));
    if is_shim {
        let exe = path.with_extension("exe");
        if exe.is_file() {
            return exe;
        }
    }
    path.to_path_buf()
}

/// The places the installers put `claude`, in the order they are tried after the hint: the native
/// installer's `~/.local/bin`, Homebrew, Claude Code's own `~/.claude/local`, and WinGet's links.
fn install_places() -> Vec<PathBuf> {
    let mut places = Vec::new();
    if let Some(home) = std::env::home_dir() {
        places.push(home.join(".local").join("bin").join(BINARY));
        if cfg!(not(windows)) {
            places.push(PathBuf::from("/opt/homebrew/bin/claude"));
            places.push(PathBuf::from("/usr/local/bin/claude"));
            places.push(home.join(".claude").join("local").join("claude"));
        }
    }
    if cfg!(windows)
        && let Some(local) = std::env::var_os("LOCALAPPDATA")
    {
        places.push(
            PathBuf::from(local)
                .join("Microsoft")
                .join("WinGet")
                .join("Links")
                .join(BINARY),
        );
    }
    places
}

/// Where `claude` is: the hint (the `claudePath` setting) when it runs, else the installers' places,
/// else whatever `PATH` says, else, on Unix, what a login shell says, since a GUI app's own `PATH` is
/// shorter than the human's. A `.cmd` shim found on the way is traded for the `.exe` beside it.
pub fn locate(hint: Option<&Path>) -> Option<PathBuf> {
    let found = hint
        .map(Path::to_path_buf)
        .into_iter()
        .chain(install_places())
        .map(|path| without_shim(&path))
        .find(|path| is_executable(path));
    if found.is_some() {
        return found;
    }
    if let Ok(path) = which::which(BINARY).or_else(|_| which::which("claude")) {
        return Some(without_shim(&path));
    }
    shell_lookup()
}

/// What `$SHELL -lc 'command -v claude'` prints, when it prints a path that runs within a few seconds.
#[cfg(unix)]
fn shell_lookup() -> Option<PathBuf> {
    let shell = std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let child = Command::new(shell)
        .args(["-lc", "command -v claude"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let (status, text) = wait_with_timeout(child, SHELL_TIMEOUT).ok()??;
    if !status.success() {
        return None;
    }
    let path = PathBuf::from(text.trim());
    (path.is_absolute() && is_executable(&path)).then_some(path)
}

/// Windows has no login shell to ask; `PATH` is the last word.
#[cfg(not(unix))]
fn shell_lookup() -> Option<PathBuf> {
    None
}

/// Waits for `child` up to `timeout`, reading its stdout meanwhile, and gives its status and output; a
/// child that outstays the timeout is killed and `None` comes back.
#[cfg(unix)]
fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> io::Result<Option<(std::process::ExitStatus, String)>> {
    let reader = child.stdout.take().map(|stdout| {
        thread::spawn(move || {
            let mut text = String::new();
            let _ = BufReader::new(stdout).read_to_string(&mut text);
            text
        })
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(20));
    };
    let text = reader.and_then(|handle| handle.join().ok()).unwrap_or_default();
    Ok(Some((status, text)))
}

/// The environment a child gets, and the variables of this process that were dropped on purpose, so
/// doctor can show the human what Budd's sessions do not see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildEnv {
    /// Every variable the child gets.
    pub vars: Vec<(OsString, OsString)>,
    /// The names in [`DROPPED_VARS`] this process has set, which the child does not get.
    pub dropped: Vec<String>,
}

impl ChildEnv {
    /// The value of `name` in the child's environment.
    pub fn get(&self, name: &str) -> Option<&OsString> {
        self.vars.iter().find(|(key, _)| key == name).map(|(_, value)| value)
    }
}

/// A fresh environment for a child `claude`, built from this process's: the home, a `PATH` that starts
/// with the binary's own folder, the locale and terminal, the temp folder and shell on Unix, the system
/// folders Windows programs need, and `CLAUDE_CONFIG_DIR` set to `config_dir`. Nothing else of this
/// process's environment comes along, least of all the variables in [`DROPPED_VARS`].
pub fn child_env(exe_dir: &Path, config_dir: &Path) -> ChildEnv {
    child_env_from(exe_dir, config_dir, |name| std::env::var_os(name))
}

/// [`child_env`] over `current` instead of this process's environment, for tests.
pub fn child_env_from(exe_dir: &Path, config_dir: &Path, current: impl Fn(&str) -> Option<OsString>) -> ChildEnv {
    let present = |name: &str| current(name).filter(|value| !value.is_empty());
    let mut vars: Vec<(OsString, OsString)> = Vec::new();
    let mut set = |name: &str, value: OsString| vars.push((name.into(), value));
    let home = present(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .or_else(|| std::env::home_dir().map(PathBuf::into_os_string));
    if let Some(home) = &home {
        set("HOME", home.clone());
        if cfg!(windows) {
            set("USERPROFILE", home.clone());
        }
    }
    let system_root = present("SYSTEMROOT").unwrap_or_else(|| r"C:\Windows".into());
    set(
        "PATH",
        search_path(exe_dir, home.as_deref(), &system_root, present("PATH")),
    );
    set("LANG", present("LANG").unwrap_or_else(|| "en_US.UTF-8".into()));
    set("TERM", present("TERM").unwrap_or_else(|| "xterm-256color".into()));
    if cfg!(windows) {
        set("SYSTEMROOT", system_root.clone());
        set(
            "COMSPEC",
            present("COMSPEC").unwrap_or_else(|| PathBuf::from(&system_root).join("System32").join("cmd.exe").into()),
        );
        set(
            "PATHEXT",
            present("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into()),
        );
        for name in ["APPDATA", "LOCALAPPDATA", "TEMP", "TMP", "USERNAME"] {
            if let Some(value) = present(name) {
                set(name, value);
            }
        }
    } else {
        if let Some(tmpdir) = present("TMPDIR") {
            set("TMPDIR", tmpdir);
        }
        set("SHELL", present("SHELL").unwrap_or_else(default_shell));
        // Claude Code finds its Keychain login by the user's name: without USER it reports the account
        // logged out (seen on 2.1.316).
        for name in ["USER", "LOGNAME"] {
            if let Some(value) = present(name) {
                set(name, value);
            }
        }
    }
    set("CLAUDE_CONFIG_DIR", config_dir.as_os_str().to_owned());
    let dropped = DROPPED_VARS
        .iter()
        .filter(|name| current(name).is_some())
        .map(|name| (*name).to_owned())
        .collect();
    ChildEnv { vars, dropped }
}

/// The shell a child gets when this process has none: bash where it is, else sh.
fn default_shell() -> OsString {
    if Path::new("/bin/bash").is_file() {
        "/bin/bash".into()
    } else {
        "/bin/sh".into()
    }
}

/// `exe_dir` first, then the folders every program expects, then whatever else this process's own
/// `PATH` had, once each. The tail keeps the human's tools (git, gh, node) reachable from a session
/// when Budd was started from a terminal; a GUI launch has little there to add.
fn search_path(
    exe_dir: &Path,
    home: Option<&std::ffi::OsStr>,
    system_root: &std::ffi::OsStr,
    current: Option<OsString>,
) -> OsString {
    let mut entries: Vec<PathBuf> = vec![exe_dir.to_path_buf()];
    if cfg!(windows) {
        let root = PathBuf::from(system_root);
        entries.push(root.join("System32"));
        entries.push(root.clone());
        entries.push(root.join("System32").join("Wbem"));
        entries.push(root.join("System32").join("WindowsPowerShell").join("v1.0"));
        if let Some(home) = home {
            entries.push(PathBuf::from(home).join(".local").join("bin"));
        }
    } else {
        if let Some(home) = home {
            entries.push(PathBuf::from(home).join(".local").join("bin"));
        }
        if cfg!(target_os = "macos") {
            entries.push(PathBuf::from("/opt/homebrew/bin"));
        }
        for dir in ["/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
            entries.push(PathBuf::from(dir));
        }
    }
    if let Some(current) = current {
        entries.extend(std::env::split_paths(&current));
    }
    let mut seen: Vec<PathBuf> = Vec::new();
    for entry in entries {
        if !entry.as_os_str().is_empty() && !seen.contains(&entry) {
            seen.push(entry);
        }
    }
    std::env::join_paths(&seen).unwrap_or_else(|_| exe_dir.as_os_str().to_owned())
}

/// The version in `claude --version`'s output, which reads "2.1.316 (Claude Code)".
pub fn parse_version(text: &str) -> Option<Version> {
    let first = text.split_whitespace().next()?;
    Version::parse(first.trim_start_matches('v')).ok()
}

/// The rows in `claude agents --json`'s output, which is a JSON array of objects.
pub fn parse_agents(text: &str) -> Result<Vec<Value>, Error> {
    let command = "agents --json".to_owned();
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(rows)) => Ok(rows),
        Ok(_) => Err(Error::Shape {
            command,
            expected: "a JSON array",
        }),
        Err(source) => Err(Error::Json { command, source }),
    }
}

/// The status in `claude auth status --json`'s output.
pub fn parse_auth_status(text: &str) -> Result<AccountStatus, Error> {
    let command = "auth status --json".to_owned();
    if let Err(source) = serde_json::from_str::<Value>(text) {
        return Err(Error::Json { command, source });
    }
    AccountStatus::decode(text).ok_or(Error::Shape {
        command,
        expected: "an object with a loggedIn flag",
    })
}

/// The `claude` binary and the config folder its runs are about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claude {
    /// The binary, as [`locate`] found it.
    pub exe: PathBuf,
    /// The `CLAUDE_CONFIG_DIR` every run gets: Budd's own folder.
    pub config_dir: PathBuf,
}

impl Claude {
    /// `exe` run against `config_dir`.
    pub fn new(exe: impl Into<PathBuf>, config_dir: impl Into<PathBuf>) -> Claude {
        Claude {
            exe: exe.into(),
            config_dir: config_dir.into(),
        }
    }

    /// The binary's folder, which heads every child's `PATH`.
    fn exe_dir(&self) -> PathBuf {
        self.exe
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// A folder with no project in it, for subcommands that are about the account rather than a folder:
    /// run from a repository, `claude mcp list` would list the repository's servers too.
    fn neutral_cwd() -> PathBuf {
        std::env::temp_dir()
    }

    /// `claude args` in `cwd` with the clean environment, stdio not yet decided.
    pub fn command(&self, args: &[&str], cwd: &Path) -> Command {
        let mut command = Command::new(&self.exe);
        command.args(args).current_dir(cwd).env_clear();
        for (name, value) in child_env(&self.exe_dir(), &self.config_dir).vars {
            command.env(name, value);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        }
        command
    }

    /// Runs `claude args` in `cwd` to the end, stdin closed, stdout and stderr captured.
    pub fn run(&self, args: &[&str], cwd: &Path) -> io::Result<Output> {
        self.command(args, cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
    }

    /// [`Claude::run`], giving stdout when it succeeded and [`Error::Failed`] with the last line it
    /// printed when it did not.
    fn run_checked(&self, args: &[&str], cwd: &Path) -> Result<String, Error> {
        let output = self.run(args, cwd)?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if output.status.success() {
            return Ok(stdout);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let said = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr.into_owned()
        };
        Err(Error::Failed {
            command: args.join(" "),
            status: output.status.code(),
            last_line: last_line(&said),
        })
    }

    /// What `claude --version` says, or `None` when it did not run or said something else.
    pub fn version(&self) -> Option<Version> {
        let output = self.run(&["--version"], &Self::neutral_cwd()).ok()?;
        output
            .status
            .success()
            .then(|| parse_version(&String::from_utf8_lossy(&output.stdout)))
            .flatten()
    }

    /// The sessions `claude agents --json` lists, as raw rows; with `all`, the completed ones too.
    pub fn agents(&self, all: bool) -> Result<Vec<Value>, Error> {
        let mut args = vec!["agents", "--json"];
        if all {
            args.push("--all");
        }
        parse_agents(&self.run_checked(&args, &Self::neutral_cwd())?)
    }

    /// Who the config folder is logged in as, from `claude auth status --json`.
    pub fn auth_status(&self) -> Result<AccountStatus, Error> {
        // `claude auth status` exits 1 when it judges the folder logged out, and still prints the JSON
        // that says so; that answer is wanted, not an error.
        let output = self.run(&["auth", "status", "--json"], &Self::neutral_cwd())?;
        match parse_auth_status(&String::from_utf8_lossy(&output.stdout)) {
            Ok(status) => Ok(status),
            Err(_) if !output.status.success() => Err(Error::Failed {
                command: "claude auth status --json".to_owned(),
                status: output.status.code(),
                last_line: last_line(&String::from_utf8_lossy(&output.stderr)),
            }),
            Err(error) => Err(error),
        }
    }

    /// The MCP servers the folder's sessions load, health-checked by `claude mcp list`, which takes a
    /// few seconds.
    pub fn mcp_list(&self) -> Result<Vec<McpServerStatus>, Error> {
        Ok(McpServerStatus::parse(
            &self.run_checked(&["mcp", "list"], &Self::neutral_cwd())?,
        ))
    }

    /// Stops the background session with `short_id`, as `claude stop` does: its process ends and it
    /// leaves the live sessions, but its conversation and job file stay, so `claude attach` brings it back.
    pub fn stop(&self, short_id: &str) -> Result<(), Error> {
        self.run_checked(&["stop", short_id], &Self::neutral_cwd()).map(|_| ())
    }

    /// What `claude mcp get name` printed about the server, or [`Error::Failed`] when there is none.
    pub fn mcp_get(&self, name: &str) -> Result<String, Error> {
        self.run_checked(&["mcp", "get", name], &Self::neutral_cwd())
    }

    /// Registers the stdio server `name` at user scope, running `exe args`, so every session of the
    /// account can call it.
    pub fn mcp_add(&self, name: &str, exe: &Path, args: &[&str]) -> Result<(), Error> {
        let exe = exe.to_string_lossy();
        let mut argv = vec!["mcp", "add", "-s", "user", name, "--", exe.as_ref()];
        argv.extend_from_slice(args);
        self.run_checked(&argv, &Self::neutral_cwd()).map(|_| ())
    }

    /// Removes the user-scope server `name`.
    pub fn mcp_remove(&self, name: &str) -> Result<(), Error> {
        self.run_checked(&["mcp", "remove", "-s", "user", name], &Self::neutral_cwd())
            .map(|_| ())
    }

    /// Starts `claude argv` in `cwd` as a background session, in its own session and process group so
    /// nothing that happens to Budd reaches it, and gives the lines it printed before it returned
    /// (`--bg` prints the session's id and returns at once). A start that fails is [`Error::Failed`]
    /// with its last line; one that does not return within [`START_TIMEOUT`] is killed.
    pub fn start_bg(&self, cwd: &Path, argv: &[String]) -> Result<Vec<String>, Error> {
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        let command_text = args.join(" ");
        let mut command = self.command(&args, cwd);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: setsid only changes the child's session and process group; it allocates nothing
            // and takes no locks, so it is safe between fork and exec.
            unsafe {
                command.pre_exec(|| {
                    libc::setsid();
                    Ok(())
                });
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};
            command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
        }
        let mut child = command.spawn()?;
        let (sender, lines) = mpsc::channel::<String>();
        for pipe in [
            child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
            child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let sender = sender.clone();
            // The session's own process may hold the pipe open after `claude --bg` returned; the reader
            // then stays until it ends, which is why the lines go through a channel and not a join.
            thread::spawn(move || {
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    if sender.send(line).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if started.elapsed() >= START_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Timeout {
                    command: command_text,
                    timeout: START_TIMEOUT,
                });
            }
            thread::sleep(Duration::from_millis(20));
        };
        let mut printed: Vec<String> = Vec::new();
        while let Ok(line) = lines.recv_timeout(Duration::from_millis(200)) {
            printed.push(line);
        }
        if status.success() {
            Ok(printed)
        } else {
            Err(Error::Failed {
                command: command_text,
                status: status.code(),
                last_line: last_line(&printed.join("\n")),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn fake_env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    #[test]
    fn the_child_env_drops_the_login_overrides_and_keeps_the_rest() {
        let current = fake_env(&[
            ("HOME", "/Users/ralph"),
            ("USERPROFILE", r"C:\Users\ralph"),
            ("PATH", "/Users/ralph/bin:/usr/bin"),
            ("LANG", "de_DE.UTF-8"),
            ("TERM", "screen"),
            ("TMPDIR", "/var/tmp/ralph"),
            ("SHELL", "/bin/zsh"),
            ("SYSTEMROOT", r"C:\Windows"),
            ("APPDATA", r"C:\Users\ralph\AppData\Roaming"),
            ("LOCALAPPDATA", r"C:\Users\ralph\AppData\Local"),
            ("COMSPEC", r"C:\Windows\System32\cmd.exe"),
            ("ANTHROPIC_API_KEY", "sk-ant-secret"),
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("ANTHROPIC_BASE_URL", ""),
            ("CLAUDE_CONFIG_DIR", "/Users/ralph/.claude"),
            ("AWS_PROFILE", "work"),
            ("GITHUB_TOKEN", "ghp_x"),
        ]);
        let env = child_env_from(
            Path::new("/opt/claude/bin"),
            Path::new("/Users/ralph/.budd/claude"),
            current,
        );
        assert_eq!(
            env.dropped,
            ["ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL", "CLAUDE_CODE_USE_BEDROCK"]
        );
        for name in DROPPED_VARS {
            assert_eq!(env.get(name), None, "{name}");
        }
        assert_eq!(env.get("AWS_PROFILE"), None);
        assert_eq!(env.get("GITHUB_TOKEN"), None);
        assert_eq!(env.get("CLAUDE_CONFIG_DIR").unwrap(), "/Users/ralph/.budd/claude");
        assert_eq!(env.get("LANG").unwrap(), "de_DE.UTF-8");
        assert_eq!(env.get("TERM").unwrap(), "screen");
        let path: Vec<PathBuf> = std::env::split_paths(env.get("PATH").unwrap()).collect();
        assert_eq!(path[0], Path::new("/opt/claude/bin"));
        assert!(path.contains(&PathBuf::from("/usr/bin")));
        assert!(path.contains(&PathBuf::from("/Users/ralph/bin")));
        assert_eq!(path.iter().filter(|p| **p == Path::new("/usr/bin")).count(), 1);
        if cfg!(windows) {
            assert_eq!(env.get("HOME").unwrap(), r"C:\Users\ralph");
            assert_eq!(env.get("USERPROFILE").unwrap(), r"C:\Users\ralph");
            assert_eq!(env.get("SYSTEMROOT").unwrap(), r"C:\Windows");
            assert_eq!(env.get("APPDATA").unwrap(), r"C:\Users\ralph\AppData\Roaming");
            assert_eq!(env.get("LOCALAPPDATA").unwrap(), r"C:\Users\ralph\AppData\Local");
            assert_eq!(env.get("COMSPEC").unwrap(), r"C:\Windows\System32\cmd.exe");
            assert_eq!(env.get("SHELL"), None);
        } else {
            assert_eq!(env.get("HOME").unwrap(), "/Users/ralph");
            assert_eq!(env.get("USERPROFILE"), None);
            assert_eq!(env.get("TMPDIR").unwrap(), "/var/tmp/ralph");
            assert_eq!(env.get("SHELL").unwrap(), "/bin/zsh");
            assert_eq!(env.get("SYSTEMROOT"), None);
            assert_eq!(env.get("APPDATA"), None);
        }
        // Nothing is listed twice.
        let mut names: Vec<&OsString> = env.vars.iter().map(|(k, _)| k).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), env.vars.len());
    }

    #[cfg(unix)]
    #[test]
    fn the_child_env_keeps_the_user_name_for_the_keychain() {
        let env = child_env_from(
            Path::new("/opt/claude/bin"),
            Path::new("/Users/ralph/.budd/claude"),
            |name| match name {
                "HOME" => Some("/Users/ralph".into()),
                "USER" => Some("ralph".into()),
                "LOGNAME" => Some("ralph".into()),
                _ => None,
            },
        );
        let get = |key: &str| env.vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone());
        assert_eq!(get("USER").as_deref(), Some(std::ffi::OsStr::new("ralph")));
        assert_eq!(get("LOGNAME").as_deref(), Some(std::ffi::OsStr::new("ralph")));
    }

    #[test]
    fn a_bare_process_still_gets_a_usable_env() {
        let env = child_env_from(Path::new("/x"), Path::new("/c"), |_| None);
        assert!(env.dropped.is_empty());
        assert_eq!(env.get("LANG").unwrap(), "en_US.UTF-8");
        assert_eq!(env.get("TERM").unwrap(), "xterm-256color");
        assert_eq!(env.get("CLAUDE_CONFIG_DIR").unwrap(), "/c");
        assert!(env.get("PATH").is_some());
        if cfg!(windows) {
            assert_eq!(env.get("SYSTEMROOT").unwrap(), r"C:\Windows");
            assert!(env.get("COMSPEC").is_some());
        } else {
            assert!(env.get("SHELL").is_some());
        }
    }

    #[test]
    fn the_version_is_the_first_word() {
        assert_eq!(parse_version("2.1.316 (Claude Code)\n"), Some(Version::new(2, 1, 316)));
        assert_eq!(parse_version("v2.1.0"), Some(Version::new(2, 1, 0)));
        assert_eq!(
            parse_version("2.2.0-beta.1 (Claude Code)").map(|v| v.to_string()),
            Some("2.2.0-beta.1".to_owned())
        );
        assert_eq!(parse_version("Claude Code 2.1.316"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn agents_and_auth_status_parse_from_fixtures() {
        let rows = parse_agents(include_str!("../fixtures/synthetic/agents.json")).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0]["name"], "assistant");
        assert!(matches!(parse_agents("{}"), Err(Error::Shape { .. })));
        assert!(matches!(parse_agents("not json"), Err(Error::Json { .. })));
        assert!(parse_agents("[]").unwrap().is_empty());

        let status = parse_auth_status(include_str!("../fixtures/synthetic/auth-status.json")).unwrap();
        assert!(status.logged_in);
        assert_eq!(status.email.as_deref(), Some("budd@example.com"));
        assert_eq!(status.auth_method.as_deref(), Some("claude.ai"));
        assert_eq!(status.config_directory.as_deref(), Some("/Users/example/.budd/claude"));
        assert!(matches!(
            parse_auth_status(r#"{"email": "x"}"#),
            Err(Error::Shape { .. })
        ));
        assert!(matches!(parse_auth_status("nope"), Err(Error::Json { .. })));
    }

    #[test]
    fn errors_say_what_claude_said() {
        let failed = Error::Failed {
            command: "stop abc".to_owned(),
            status: Some(1),
            last_line: "No such session".to_owned(),
        };
        assert_eq!(failed.to_string(), "claude stop abc failed (status 1): No such session");
        assert_eq!(last_line("one\n\ntwo  \n\n"), "two");
        assert_eq!(last_line("   \n"), "claude printed nothing");
        assert_eq!(last_line(&"x".repeat(500)).chars().count(), LAST_LINE);
    }

    fn executable(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\necho 2.1.316 (Claude Code)\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn a_hint_that_runs_wins_and_a_shim_yields_to_its_exe() {
        let temp = tempfile::tempdir().unwrap();
        let hinted = executable(temp.path(), BINARY);
        assert_eq!(locate(Some(&hinted)), Some(hinted.clone()));
        // A hint that is a folder, missing, or not executable is passed over.
        assert_ne!(locate(Some(temp.path())), Some(temp.path().to_path_buf()));
        let missing = temp.path().join("nowhere").join(BINARY);
        assert_ne!(locate(Some(&missing)), Some(missing));
        #[cfg(unix)]
        {
            let plain = temp.path().join("plain");
            std::fs::write(&plain, "").unwrap();
            assert!(!is_executable(&plain));
            assert_ne!(locate(Some(&plain)), Some(plain));
        }
        // A .cmd shim with an .exe beside it is the .exe; one on its own stays.
        let shim = executable(temp.path(), "claude.cmd");
        assert_eq!(without_shim(&shim), shim);
        let exe = executable(temp.path(), "claude.exe");
        assert_eq!(without_shim(&shim), exe);
        assert_eq!(locate(Some(&shim)), Some(exe));
        assert_eq!(
            without_shim(Path::new("/usr/local/bin/claude")),
            Path::new("/usr/local/bin/claude")
        );
    }

    #[test]
    fn the_exe_dir_heads_the_path_of_a_command() {
        let claude = Claude::new("/opt/claude/bin/claude", "/Users/ralph/.budd/claude");
        assert_eq!(claude.exe_dir(), Path::new("/opt/claude/bin"));
        assert_eq!(Claude::new("claude", "/c").exe_dir(), Path::new("."));
        let command = claude.command(&["--version"], Path::new("/"));
        let envs: HashMap<String, Option<String>> = command
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(envs["CLAUDE_CONFIG_DIR"].as_deref(), Some("/Users/ralph/.budd/claude"));
        assert!(envs["PATH"].as_deref().unwrap().starts_with("/opt/claude/bin"));
        assert!(!envs.contains_key("ANTHROPIC_API_KEY"));
    }
}
