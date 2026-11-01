//! `budd setup`, `budd connect` and `budd reset`: readying Budd's folder, logging it in, and starting over.
//!
//! Setup makes the folder without touching the login: the folders, `settings.json` wired to this
//! binary, the MCP server, the assistant's starter `CLAUDE.md`, the defaults in `config.json`. It adds
//! only what is missing and can run any number of times. Connect opens the platform's terminal on
//! `claude auth login` for the folder (logging in is a browser flow the human does themselves) and
//! waits until `claude auth status` says logged in. Reset forgets what Budd learned while running and
//! keeps what the human set. Nothing here reads a credential, and the human's own `~/.claude` is only
//! read when they pass it as a source.

use crate::claude_cli::{self, Claude};
use crate::config::Config;
use crate::crew::{CLAUDE_JSON, SETTINGS_FILE};
use crate::paths::{BuddHome, ensure_private_dir, write_atomically};
use budd_core::account::{AccountStatus, FolderTrust, SERVER_NAME, settings};
use budd_core::location::project_slug;
use serde_json::Value;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

/// How often the login is checked while connect waits.
pub const LOGIN_POLL: Duration = Duration::from_secs(2);
/// How long connect waits for the login unless told otherwise.
pub const LOGIN_WAIT: Duration = Duration::from_secs(600);
/// The login method Budd's folder should end up with.
pub const CLAUDE_AI: &str = "claude.ai";
/// The file the daemon keeps its state in.
pub const STATE_FILE: &str = "budd-state.json";

/// What went wrong.
#[derive(Debug, Error)]
pub enum Error {
    /// A file or folder could not be made or read.
    #[error("{what} {path}: {source}")]
    Io {
        /// What was being done.
        what: &'static str,
        /// Where.
        path: PathBuf,
        /// What went wrong.
        #[source]
        source: io::Error,
    },
    /// `claude` is needed for this and was not found.
    #[error("claude was not found; install Claude Code or pass --claude <exe>")]
    NoClaude,
    /// `claude` failed.
    #[error("{0}")]
    Claude(#[from] claude_cli::Error),
    /// The config file is there but broken.
    #[error("{0}")]
    Config(#[from] crate::config::Error),
    /// The terminal could not be opened.
    #[error("opening the terminal: {0}")]
    Terminal(String),
    /// The login did not come within the wait.
    #[error("no claude.ai login after {} s; run `budd connect` again when you have logged in", .0.as_secs())]
    LoginTimeout(Duration),
    /// A settings file is not JSON.
    #[error("{path} is not JSON: {source}")]
    NotJson {
        /// The file.
        path: PathBuf,
        /// What the parser said.
        #[source]
        source: serde_json::Error,
    },
}

fn io_error<'a>(what: &'static str, path: &'a Path) -> impl FnOnce(io::Error) -> Error + 'a {
    move |source| Error::Io {
        what,
        path: path.to_path_buf(),
        source,
    }
}

/// The starter persona of the assistant's folder, written once and then the human's to edit.
pub const ASSISTANT_CLAUDE_MD: &str = "\
# Budd's assistant

You are the assistant session Budd shows at the first desk of his office.
Budd is the app on the human's screen; you are the one Claude Code session he keeps running for them, in this folder, on his own account.

## How to answer

- Answer the human's messages directly and briefly. They read your replies on Budd's card, not in a terminal.
- When you need something from them, end your reply with one `Reply:` line offering the choices, as the kit's reply contract says, for example: `Reply: ` + the answers in backticks, `yes` or `no`.
- When the choice is yours to recommend and Budd may take it for you, use a `Decide:` line instead, naming the answer you recommend.
- Ask through Budd's `ask` tool when you want the question pinned on his card with buttons; it returns at once and the answer arrives as your next message.

## What you never do

- Never start crew sessions yourself (`claude --bg`, scripts, anything). Budd starts them, from Start Tickets or a chat, in the right project and permission mode.
- Never touch the human's own `~/.claude`: your account and folder are Budd's.
- Never run anything destructive without a `Reply:` line asking first.
";

/// Readies `home` for Budd without logging in. `claude` is needed to register the MCP server; without
/// it that step is reported and skipped. `exe` is this binary, which the hooks and the server run.
/// With `dry_run`, nothing is written and every line says what would be done. The lines come back for
/// the terminal.
pub fn setup(home: &BuddHome, claude: Option<&Claude>, exe: &Path, dry_run: bool) -> Result<Vec<String>, Error> {
    let mut lines = Vec::new();
    let mut skipped = Vec::new();
    let mut note = |done: bool, text: String| {
        let prefix = if dry_run {
            "would "
        } else if done {
            ""
        } else {
            "kept "
        };
        lines.push(format!("{prefix}{text}"));
    };

    // The folders.
    for (dir, private) in [
        (home.root().to_path_buf(), false),
        (home.claude_dir(), false),
        (home.assistant_dir(), false),
        (home.state_dir(), false),
        (home.run_dir(), true),
        (home.logs_dir(), false),
        (home.drops_dir(), false),
    ] {
        let existed = dir.is_dir();
        if !dry_run {
            if private {
                ensure_private_dir(&dir).map_err(io_error("creating", &dir))?;
            } else {
                std::fs::create_dir_all(&dir).map_err(io_error("creating", &dir))?;
            }
        }
        note(
            !existed,
            format!("create {}{}", dir.display(), if private { " (private)" } else { "" }),
        );
    }

    // settings.json: this binary on every event, auto mode.
    let settings_path = home.claude_dir().join(SETTINGS_FILE);
    let existing = read_json(&settings_path)?;
    let (settings, changed) = settings::reconcile(existing, &exe.to_string_lossy());
    if changed && !dry_run {
        let text = format!("{}\n", serde_json::to_string_pretty(&settings).unwrap_or_default());
        write_atomically(&settings_path, text.as_bytes()).map_err(io_error("writing", &settings_path))?;
    }
    note(
        changed,
        format!(
            "write the hooks and auto mode into {} for {}",
            settings_path.display(),
            exe.display()
        ),
    );

    // The assistant's folder is trusted, so the assistant and chats start without a question.
    let assistant_dir = home.assistant_dir();
    let trusted = if dry_run {
        !crate::crew::ensure_trusted_check(&home.claude_dir(), &assistant_dir)
    } else {
        crate::crew::ensure_trusted(&home.claude_dir(), &assistant_dir).map_err(io_error(
            "trusting the assistant folder in",
            &home.claude_dir().join(CLAUDE_JSON),
        ))?
    };
    note(trusted, format!("trust {} in {}", assistant_dir.display(), CLAUDE_JSON));

    // The MCP server.
    match claude {
        Some(claude) => {
            let registered = if dry_run {
                !server_registered(claude, exe)
            } else {
                register_server(claude, exe)?
            };
            note(
                registered,
                format!("register the MCP server {SERVER_NAME} as `{} mcp` at user scope", exe.display()),
            );
        }
        None => skipped.push(format!(
            "skipped registering the MCP server {SERVER_NAME}: claude was not found; run `budd setup` again once it is installed"
        )),
    }

    // The starter persona and the config, only when missing.
    let claude_md = assistant_dir.join("CLAUDE.md");
    let wrote_md = !claude_md.exists();
    if wrote_md && !dry_run {
        write_atomically(&claude_md, ASSISTANT_CLAUDE_MD.as_bytes()).map_err(io_error("writing", &claude_md))?;
    }
    note(wrote_md, format!("write the starter {}", claude_md.display()));
    let config_path = home.config_path();
    let wrote_config = !config_path.exists();
    if wrote_config && !dry_run {
        Config::default().save(&config_path)?;
    }
    note(
        wrote_config,
        format!("write the defaults into {}", config_path.display()),
    );
    lines.append(&mut skipped);
    Ok(lines)
}

/// The JSON object in `path`, `None` when the file is missing, an error when it is not JSON.
fn read_json(path: &Path) -> Result<Option<Value>, Error> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|source| Error::NotJson {
            path: path.to_path_buf(),
            source,
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::Io {
            what: "reading",
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Whether `claude mcp get budd` says the server runs `exe mcp` already.
pub fn server_registered(claude: &Claude, exe: &Path) -> bool {
    claude
        .mcp_get(SERVER_NAME)
        .is_ok_and(|output| crate::doctor::registered_to(&output, exe))
}

/// Registers the MCP server `budd` as `exe mcp` at user scope in the folder `claude` runs against,
/// replacing one that points elsewhere (a Budd that moved), and says whether anything changed.
pub fn register_server(claude: &Claude, exe: &Path) -> Result<bool, Error> {
    match claude.mcp_get(SERVER_NAME) {
        Ok(output) if crate::doctor::registered_to(&output, exe) => return Ok(false),
        Ok(_) => {
            claude.mcp_remove(SERVER_NAME)?;
        }
        Err(_) => {}
    }
    claude.mcp_add(SERVER_NAME, exe, &["mcp"])?;
    Ok(true)
}

/// Which terminal connect opens the login in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Terminal {
    /// The platform's: Terminal.app, Windows Terminal or PowerShell, x-terminal-emulator.
    #[default]
    Default,
    /// iTerm2, on macOS.
    Iterm,
}

impl Terminal {
    /// The terminal `--terminal` names.
    pub fn parse(name: &str) -> Option<Terminal> {
        match name.to_lowercase().as_str() {
            "default" | "terminal" => Some(Terminal::Default),
            "iterm" | "iterm2" => Some(Terminal::Iterm),
            _ => None,
        }
    }
}

/// What `budd connect` was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectOptions {
    /// Where the login opens.
    pub terminal: Terminal,
    /// How long to wait for the login.
    pub wait: Duration,
    /// A config folder whose trusted projects Budd's folder takes on.
    pub carry_trust_from: Option<PathBuf>,
    /// A config folder whose memory for the assistant folder is copied once.
    pub import_memory_from: Option<PathBuf>,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        ConnectOptions {
            terminal: Terminal::Default,
            wait: LOGIN_WAIT,
            carry_trust_from: None,
            import_memory_from: None,
        }
    }
}

/// Opens the login, waits for it, then carries trust and memory over as asked. Progress goes to `out`.
pub fn connect(
    home: &BuddHome,
    claude: &Claude,
    options: &ConnectOptions,
    out: &mut dyn Write,
) -> Result<AccountStatus, Error> {
    std::fs::create_dir_all(home.claude_dir()).map_err(io_error("creating", &home.claude_dir()))?;
    let already = claude.auth_status().ok().filter(|status| logged_in_right(status));
    let status = match already {
        Some(status) => {
            let _ = writeln!(out, "Budd's folder is already logged in as {}", status.summary());
            status
        }
        None => {
            open_login(home, claude, options.terminal)?;
            let _ = writeln!(
                out,
                "Log in with the account Budd should use, in the window that opened. Waiting up to {} s.",
                options.wait.as_secs()
            );
            let status = wait_for_login(claude, options.wait)?;
            let _ = writeln!(out, "Logged in as {}", status.summary());
            status
        }
    };
    if let Some(from) = &options.carry_trust_from {
        let carried = carry_trust(from, &home.claude_dir())?;
        let _ = writeln!(out, "Trusted {carried} folders from {}", from.display());
    }
    if let Some(from) = &options.import_memory_from {
        let copied = import_memory(from, &home.claude_dir(), &home.assistant_dir())?;
        let _ = writeln!(
            out,
            "{}",
            if copied {
                format!("Copied the assistant's memory from {}", from.display())
            } else {
                "Memory already there or none to copy".to_owned()
            }
        );
    }
    Ok(status)
}

/// Whether the folder is logged in the way Budd wants: with claude.ai, or with a Claude Code that
/// does not say.
pub fn logged_in_right(status: &AccountStatus) -> bool {
    status.logged_in && status.auth_method.as_deref().is_none_or(|method| method == CLAUDE_AI)
}

/// Opens the platform's terminal on `claude auth login --claudeai` with `CLAUDE_CONFIG_DIR` set to
/// Budd's folder.
pub fn open_login(home: &BuddHome, claude: &Claude, terminal: Terminal) -> Result<(), Error> {
    let config_dir = home.claude_dir();
    if cfg!(target_os = "macos") {
        let script = login_script(&config_dir, &claude.exe);
        let file = home.run_dir().join("login.command");
        ensure_private_dir(&home.run_dir()).map_err(io_error("creating", &home.run_dir()))?;
        write_atomically(&file, script.as_bytes()).map_err(io_error("writing", &file))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))
                .map_err(io_error("chmod", &file))?;
        }
        let app = match terminal {
            Terminal::Default => "Terminal",
            Terminal::Iterm => "iTerm",
        };
        return spawn_detached(Command::new("open").args(["-a", app]).arg(&file));
    }
    if cfg!(windows) {
        let command = format!(
            "$env:CLAUDE_CONFIG_DIR='{}'; & '{}' auth login --claudeai",
            config_dir.to_string_lossy().replace('\'', "''"),
            claude.exe.to_string_lossy().replace('\'', "''")
        );
        if let Ok(wt) = which::which("wt.exe") {
            return spawn_detached(Command::new(wt).arg("--").args([
                "powershell.exe",
                "-NoExit",
                "-Command",
                &command,
            ]));
        }
        let mut powershell = Command::new("powershell.exe");
        powershell.args(["-NoExit", "-Command", &command]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            powershell.creation_flags(windows_sys::Win32::System::Threading::CREATE_NEW_CONSOLE);
        }
        return spawn_detached(&mut powershell);
    }
    let script = format!(
        "export CLAUDE_CONFIG_DIR={}; exec {} auth login --claudeai",
        shell_quote(&config_dir.to_string_lossy()),
        shell_quote(&claude.exe.to_string_lossy())
    );
    spawn_detached(Command::new("x-terminal-emulator").args(["-e", "sh", "-c", &script]))
}

/// The `login.command` Terminal.app opens: the folder exported, then `claude auth login`.
pub fn login_script(config_dir: &Path, claude_exe: &Path) -> String {
    format!(
        "#!/bin/sh\nexport CLAUDE_CONFIG_DIR={}\necho \"Logging Budd in. Pick the account Budd should use; this window can close when it says so.\"\nexec {} auth login --claudeai\n",
        shell_quote(&config_dir.to_string_lossy()),
        shell_quote(&claude_exe.to_string_lossy())
    )
}

/// `text` in single quotes for a POSIX shell.
pub fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// Starts `command` and lets it go, stdio closed.
fn spawn_detached(command: &mut Command) -> Result<(), Error> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| Error::Terminal(format!("{:?}: {error}", command.get_program())))
}

/// Asks `claude auth status --json` every two seconds until the folder is logged in with claude.ai,
/// for up to `wait`.
pub fn wait_for_login(claude: &Claude, wait: Duration) -> Result<AccountStatus, Error> {
    let started = Instant::now();
    loop {
        if let Ok(status) = claude.auth_status()
            && logged_in_right(&status)
        {
            return Ok(status);
        }
        if started.elapsed() >= wait {
            return Err(Error::LoginTimeout(wait));
        }
        thread::sleep(LOGIN_POLL.min(wait.saturating_sub(started.elapsed()).max(Duration::from_millis(50))));
    }
}

/// Trusts in `to` every folder `from`'s `.claude.json` trusts, and says how many. Nothing else of
/// `from` is read, and nothing else of `to` changes.
pub fn carry_trust(from: &Path, to: &Path) -> Result<usize, Error> {
    let source = read_json(&from.join(CLAUDE_JSON))?.unwrap_or(Value::Null);
    let trusted = FolderTrust::trusted(&source);
    if trusted.is_empty() {
        return Ok(0);
    }
    let target_path = to.join(CLAUDE_JSON);
    let target = read_json(&target_path)?.unwrap_or_else(|| Value::Object(Default::default()));
    let already = FolderTrust::trusted(&target);
    let new: Vec<&String> = trusted.iter().filter(|folder| !already.contains(folder)).collect();
    if new.is_empty() {
        return Ok(0);
    }
    let merged = FolderTrust::trusting(target, &trusted);
    let text = format!("{}\n", serde_json::to_string_pretty(&merged).unwrap_or_default());
    write_atomically(&target_path, text.as_bytes()).map_err(io_error("writing", &target_path))?;
    Ok(new.len())
}

/// Copies `from`'s memory for `assistant_dir` (`projects/<slug>/memory/`) into `to`, once: nothing is
/// copied when `to` has the folder already or `from` has none. Says whether it copied.
pub fn import_memory(from: &Path, to: &Path, assistant_dir: &Path) -> Result<bool, Error> {
    let relative = Path::new("projects")
        .join(project_slug(&assistant_dir.to_string_lossy()))
        .join("memory");
    let source = from.join(&relative);
    let target = to.join(&relative);
    if from == to || !source.is_dir() || target.exists() {
        return Ok(false);
    }
    copy_dir(&source, &target).map_err(io_error("copying memory to", &target))?;
    Ok(true)
}

/// Copies the files and folders under `from` into `to`, which is created.
fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// What `budd reset` did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResetReport {
    /// The daemon forgot its state and stopped the assistant.
    pub via_daemon: bool,
    /// The state file was deleted.
    pub state_deleted: bool,
    /// How many dropped files went.
    pub drops_removed: usize,
    /// The folder was logged out.
    pub signed_out: bool,
}

/// Forgets what Budd learned while running: asks the daemon through `via_daemon` (which answers
/// `Some` when a daemon is there, telling it to forget and stop the assistant), deletes the state file
/// and the drops, and with `sign_out` logs the folder out. The config, the Claude folder and the kit
/// stay.
pub fn reset(
    home: &BuddHome,
    claude: Option<&Claude>,
    sign_out: bool,
    via_daemon: &mut dyn FnMut() -> Option<Result<(), String>>,
) -> Result<ResetReport, Error> {
    let mut report = ResetReport::default();
    match via_daemon() {
        Some(Ok(())) => report.via_daemon = true,
        Some(Err(error)) => return Err(Error::Terminal(format!("the daemon did not reset: {error}"))),
        None => {}
    }
    let state = home.state_dir().join(STATE_FILE);
    match std::fs::remove_file(&state) {
        Ok(()) => report.state_deleted = true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(io_error("deleting", &state)(source)),
    }
    report.drops_removed = clear_dir(&home.drops_dir()).map_err(io_error("clearing", &home.drops_dir()))?;
    if sign_out {
        let claude = claude.ok_or(Error::NoClaude)?;
        claude.auth_logout()?;
        report.signed_out = true;
    }
    Ok(report)
}

/// Removes everything inside `dir`, keeping the folder, and says how many entries went; a missing
/// folder has nothing in it.
fn clear_dir(dir: &Path) -> io::Result<usize> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
        removed += 1;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fake_home(temp: &Path) -> (BuddHome, Option<Claude>) {
        let home = BuddHome::at(temp.join("budd"));
        let claude = crate::test_support::fake_claude().map(|exe| Claude::new(exe, home.claude_dir()));
        (home, claude)
    }

    #[test]
    fn setup_makes_the_folder_once_and_a_dry_run_touches_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let (home, claude) = fake_home(temp.path());
        let exe = Path::new("/opt/budd");
        let dry = setup(&home, claude.as_ref(), exe, true).unwrap();
        assert!(dry.iter().all(|line| line.starts_with("would ")), "{dry:?}");
        assert!(!home.root().exists(), "a dry run makes nothing");
        let first = setup(&home, claude.as_ref(), exe, false).unwrap();
        assert!(first.iter().any(|line| line.starts_with("create ")), "{first:?}");
        for dir in [
            home.claude_dir(),
            home.assistant_dir(),
            home.state_dir(),
            home.run_dir(),
            home.logs_dir(),
            home.drops_dir(),
        ] {
            assert!(dir.is_dir(), "{}", dir.display());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(home.run_dir()).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(home.claude_dir().join(SETTINGS_FILE)).unwrap()).unwrap();
        assert_eq!(settings["permissions"]["defaultMode"], "auto");
        for event in settings::EVENTS {
            assert_eq!(
                settings["hooks"][*event][0]["hooks"][0]["command"], "/opt/budd",
                "{event}"
            );
            assert_eq!(settings["hooks"][*event][0]["hooks"][0]["args"], json!(["hook", event]));
        }
        let trust: Value =
            serde_json::from_str(&std::fs::read_to_string(home.claude_dir().join(CLAUDE_JSON)).unwrap()).unwrap();
        assert_eq!(
            trust["projects"][home.assistant_dir().to_string_lossy().as_ref()]["hasTrustDialogAccepted"],
            true
        );
        let persona = std::fs::read_to_string(home.assistant_dir().join("CLAUDE.md")).unwrap();
        assert!(persona.contains("Reply:") && persona.contains("Never start crew"));
        assert_eq!(Config::load(&home.config_path()).unwrap(), Config::default());
        if let Some(claude) = &claude {
            assert!(server_registered(claude, exe));
        }
        // The human's edits stay: the persona, the config, and their own hooks.
        std::fs::write(home.assistant_dir().join("CLAUDE.md"), "mine").unwrap();
        let mut config = Config::default();
        config.sounds = false;
        config.save(&home.config_path()).unwrap();
        let mut settings = settings;
        settings["hooks"]["PreCompact"] = json!([{"hooks": [{"type": "command", "command": "echo"}]}]);
        std::fs::write(
            home.claude_dir().join(SETTINGS_FILE),
            serde_json::to_string(&settings).unwrap(),
        )
        .unwrap();
        let again = setup(&home, claude.as_ref(), exe, false).unwrap();
        assert!(
            again
                .iter()
                .all(|line| line.starts_with("kept ") || line.starts_with("skipped ")),
            "{again:?}"
        );
        assert_eq!(
            std::fs::read_to_string(home.assistant_dir().join("CLAUDE.md")).unwrap(),
            "mine"
        );
        assert!(!Config::load(&home.config_path()).unwrap().sounds);
        let settings: Value =
            serde_json::from_str(&std::fs::read_to_string(home.claude_dir().join(SETTINGS_FILE)).unwrap()).unwrap();
        assert_eq!(settings["hooks"]["PreCompact"][0]["hooks"][0]["command"], "echo");
        // A moved binary is re-registered everywhere.
        let moved = setup(&home, claude.as_ref(), Path::new("/new/budd"), false).unwrap();
        assert!(
            moved.iter().any(|line| line.starts_with("write the hooks")),
            "{moved:?}"
        );
        if let Some(claude) = &claude {
            assert!(
                moved.iter().any(|line| line.starts_with("register the MCP")),
                "{moved:?}"
            );
            assert!(server_registered(claude, Path::new("/new/budd")));
            assert!(!server_registered(claude, exe));
        }
    }

    #[test]
    fn the_login_script_and_quoting() {
        let script = login_script(Path::new("/Users/r/.budd/claude"), Path::new("/opt/home brew/claude"));
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("export CLAUDE_CONFIG_DIR='/Users/r/.budd/claude'\n"));
        assert!(script.ends_with("exec '/opt/home brew/claude' auth login --claudeai\n"));
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(Terminal::parse("iTerm"), Some(Terminal::Iterm));
        assert_eq!(Terminal::parse("default"), Some(Terminal::Default));
        assert_eq!(Terminal::parse("kitty"), None);
        let status = |logged_in: bool, method: Option<&str>| AccountStatus {
            logged_in,
            email: None,
            organization: None,
            plan: None,
            auth_method: method.map(str::to_owned),
            config_directory: None,
        };
        assert!(logged_in_right(&status(true, Some("claude.ai"))));
        assert!(logged_in_right(&status(true, None)));
        assert!(!logged_in_right(&status(true, Some("console"))));
        assert!(!logged_in_right(&status(false, Some("claude.ai"))));
    }

    #[test]
    fn waiting_for_the_login_reads_the_fake_until_it_says_so() {
        let temp = tempfile::tempdir().unwrap();
        let (home, Some(claude)) = fake_home(temp.path()) else {
            return;
        };
        std::fs::create_dir_all(home.claude_dir()).unwrap();
        let auth = temp.path().join("auth.json");
        std::fs::write(
            home.claude_dir().join("fake-claude.env"),
            format!("FAKE_CLAUDE_AUTH={}\n", auth.display()),
        )
        .unwrap();
        assert!(matches!(
            wait_for_login(&claude, Duration::from_millis(300)),
            Err(Error::LoginTimeout(_))
        ));
        std::fs::write(&auth, r#"{"loggedIn": true, "authMethod": "console"}"#).unwrap();
        assert!(matches!(
            wait_for_login(&claude, Duration::from_millis(300)),
            Err(Error::LoginTimeout(_))
        ));
        std::fs::write(
            &auth,
            r#"{"loggedIn": true, "authMethod": "claude.ai", "email": "budd@example.com"}"#,
        )
        .unwrap();
        let status = wait_for_login(&claude, Duration::from_secs(5)).unwrap();
        assert_eq!(status.email.as_deref(), Some("budd@example.com"));
        // Connect with an already logged-in folder opens nothing and carries what it is asked to.
        let other = temp.path().join("other-claude");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join(CLAUDE_JSON),
            json!({"projects": {"/w/a": {"hasTrustDialogAccepted": true}, "/w/b": {"hasTrustDialogAccepted": false}}})
                .to_string(),
        )
        .unwrap();
        let memory = other
            .join("projects")
            .join(project_slug(&home.assistant_dir().to_string_lossy()))
            .join("memory");
        std::fs::create_dir_all(memory.join("sub")).unwrap();
        std::fs::write(memory.join("MEMORY.md"), "remember").unwrap();
        std::fs::write(memory.join("sub").join("note.md"), "deep").unwrap();
        let options = ConnectOptions {
            carry_trust_from: Some(other.clone()),
            import_memory_from: Some(other.clone()),
            wait: Duration::from_secs(1),
            ..ConnectOptions::default()
        };
        let mut out: Vec<u8> = Vec::new();
        let status = connect(&home, &claude, &options, &mut out).unwrap();
        assert!(status.logged_in);
        let said = String::from_utf8(out).unwrap();
        assert!(said.contains("already logged in"), "{said}");
        assert!(said.contains("Trusted 1 folders"), "{said}");
        assert!(said.contains("Copied the assistant's memory"), "{said}");
        let trust: Value =
            serde_json::from_str(&std::fs::read_to_string(home.claude_dir().join(CLAUDE_JSON)).unwrap()).unwrap();
        assert_eq!(trust["projects"]["/w/a"]["hasTrustDialogAccepted"], true);
        assert!(trust["projects"].get("/w/b").is_none());
        let copied = home
            .claude_dir()
            .join("projects")
            .join(project_slug(&home.assistant_dir().to_string_lossy()))
            .join("memory");
        assert_eq!(std::fs::read_to_string(copied.join("MEMORY.md")).unwrap(), "remember");
        assert_eq!(
            std::fs::read_to_string(copied.join("sub").join("note.md")).unwrap(),
            "deep"
        );
        // Once there, memory is never copied again, and trust already carried counts zero.
        std::fs::write(memory.join("MEMORY.md"), "changed").unwrap();
        assert!(!import_memory(&other, &home.claude_dir(), &home.assistant_dir()).unwrap());
        assert_eq!(std::fs::read_to_string(copied.join("MEMORY.md")).unwrap(), "remember");
        assert_eq!(carry_trust(&other, &home.claude_dir()).unwrap(), 0);
        assert_eq!(
            carry_trust(&temp.path().join("nowhere"), &home.claude_dir()).unwrap(),
            0
        );
    }

    #[test]
    fn reset_forgets_state_and_drops_and_keeps_the_rest() {
        let temp = tempfile::tempdir().unwrap();
        let (home, claude) = fake_home(temp.path());
        std::fs::create_dir_all(home.state_dir()).unwrap();
        std::fs::create_dir_all(home.drops_dir().join("folder")).unwrap();
        std::fs::create_dir_all(home.claude_dir()).unwrap();
        std::fs::write(home.state_dir().join(STATE_FILE), "{}").unwrap();
        std::fs::write(home.state_dir().join("other.json"), "{}").unwrap();
        std::fs::write(home.drops_dir().join("a.png"), "x").unwrap();
        std::fs::write(home.drops_dir().join("folder").join("b.txt"), "x").unwrap();
        std::fs::write(home.config_path(), "{}").unwrap();
        let mut asked = 0;
        let report = reset(&home, None, false, &mut || {
            asked += 1;
            None
        })
        .unwrap();
        assert_eq!(asked, 1);
        assert_eq!(
            report,
            ResetReport {
                via_daemon: false,
                state_deleted: true,
                drops_removed: 2,
                signed_out: false,
            }
        );
        assert!(!home.state_dir().join(STATE_FILE).exists());
        assert!(home.state_dir().join("other.json").exists());
        assert!(home.drops_dir().is_dir());
        assert_eq!(std::fs::read_dir(home.drops_dir()).unwrap().count(), 0);
        assert!(home.config_path().exists() && home.claude_dir().exists());
        // With a daemon, it answers; a daemon that refuses stops the reset.
        let report = reset(&home, None, false, &mut || Some(Ok(()))).unwrap();
        assert!(report.via_daemon && !report.state_deleted && report.drops_removed == 0);
        assert!(reset(&home, None, false, &mut || Some(Err("busy".to_owned()))).is_err());
        // Signing out needs claude; with the fake it rewrites the auth file.
        assert!(matches!(reset(&home, None, true, &mut || None), Err(Error::NoClaude)));
        if let Some(claude) = &claude {
            let auth = temp.path().join("auth.json");
            std::fs::write(&auth, r#"{"loggedIn": true, "authMethod": "claude.ai"}"#).unwrap();
            std::fs::write(
                home.claude_dir().join("fake-claude.env"),
                format!("FAKE_CLAUDE_AUTH={}\n", auth.display()),
            )
            .unwrap();
            let report = reset(&home, Some(claude), true, &mut || None).unwrap();
            assert!(report.signed_out);
            assert!(!claude.auth_status().unwrap().logged_in);
        }
    }
}
