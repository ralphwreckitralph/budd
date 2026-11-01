//! `budd doctor`: one line per thing Budd needs, each ok, a warning, or a failure.
//!
//! Every check is a small function giving a [`Check`], so the CLI prints them and the daemon answers
//! `doctor.run` with the same list. A check never panics and never changes anything: a missing piece
//! is a line that says what to run, usually `budd setup` or `budd connect`.

use crate::claude_cli::{self, Claude, DROPPED_VARS};
use crate::compat::version::{SUPPORTED, Standing, TESTED_THROUGH};
use crate::ipc::Endpoint;
use crate::paths::BuddHome;
use crate::procs;
use budd_core::account::{SERVER_NAME, settings};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The most bytes a Unix socket path may have, counting the final nul, on the platforms Budd runs on
/// (macOS allows 104; Linux 108).
pub const SOCKET_PATH_LIMIT: usize = 104;

/// How a check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// As it should be.
    Ok,
    /// Works, with something worth knowing.
    Warn,
    /// Budd cannot work until this is fixed.
    Fail,
}

impl State {
    /// The word as the line prints it, padded to the same width.
    pub fn label(self) -> &'static str {
        match self {
            State::Ok => "ok  ",
            State::Warn => "warn",
            State::Fail => "fail",
        }
    }
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    /// What was checked, one word or two: "claude", "account", "hooks".
    pub name: String,
    /// How it came out.
    pub state: State,
    /// What was found, and what to do when it is not ok.
    pub detail: String,
}

impl Check {
    /// A check that passed.
    pub fn ok(name: &str, detail: impl Into<String>) -> Check {
        Check::new(name, State::Ok, detail)
    }

    /// A check that passed with a note.
    pub fn warn(name: &str, detail: impl Into<String>) -> Check {
        Check::new(name, State::Warn, detail)
    }

    /// A check that failed.
    pub fn fail(name: &str, detail: impl Into<String>) -> Check {
        Check::new(name, State::Fail, detail)
    }

    fn new(name: &str, state: State, detail: impl Into<String>) -> Check {
        Check {
            name: name.to_owned(),
            state,
            detail: detail.into(),
        }
    }

    /// The line as the terminal shows it: "ok    claude   2.1.316 at /opt/homebrew/bin/claude".
    pub fn line(&self) -> String {
        format!("{}  {:<10} {}", self.state.label(), self.name, self.detail)
    }
}

/// The worst state in `checks`: what the exit code says.
pub fn worst(checks: &[Check]) -> State {
    checks.iter().map(|check| check.state).max().unwrap_or(State::Ok)
}

/// What the checks look at.
#[derive(Debug, Clone)]
pub struct Context {
    /// Budd's folder.
    pub home: BuddHome,
    /// This binary, which the hooks and the MCP server must point at.
    pub exe: PathBuf,
    /// The `claude` to check, when one was found.
    pub claude: Option<Claude>,
    /// Budd's version.
    pub version: String,
}

impl Context {
    /// The context for `home`: `claude` located from the hint (the config's `claude_path`), and this
    /// process's own binary.
    pub fn resolve(home: BuddHome, claude_hint: Option<&Path>) -> Context {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("budd"));
        let exe = std::path::absolute(&exe).unwrap_or(exe);
        let claude = claude_cli::locate(claude_hint).map(|path| Claude::new(path, home.claude_dir()));
        Context {
            home,
            exe,
            claude,
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

/// Every check, in the order the report prints them.
pub fn run(ctx: &Context) -> Vec<Check> {
    let mut checks = vec![check_budd(ctx), check_claude(ctx)];
    checks.push(check_config_dir(ctx));
    checks.push(check_account(ctx));
    checks.push(check_env());
    checks.push(check_hooks(ctx));
    checks.push(check_mcp(ctx));
    checks.push(check_endpoint(ctx));
    checks.push(check_daemon(ctx));
    if cfg!(windows) {
        checks.push(check_git_for_windows());
        checks.push(check_webview2());
    }
    checks.push(check_tool("node", "the kit's evidence shots and some skills need it"));
    checks.push(check_tool("gh", "the kit opens pull requests with it"));
    checks
}

/// Budd's own version and the platform.
pub fn check_budd(ctx: &Context) -> Check {
    Check::ok(
        "budd",
        format!(
            "{} on {} {} at {}",
            ctx.version,
            std::env::consts::OS,
            std::env::consts::ARCH,
            ctx.exe.display()
        ),
    )
}

/// `claude` found, and its version against the compat gate.
pub fn check_claude(ctx: &Context) -> Check {
    let Some(claude) = &ctx.claude else {
        return Check::fail(
            "claude",
            "not found; install Claude Code, or set claude_path in config.json",
        );
    };
    let Some(version) = claude.version() else {
        return Check::warn(
            "claude",
            format!("{} did not say its version; is it Claude Code?", claude.exe.display()),
        );
    };
    match Standing::of(&version) {
        Standing::Unsupported => Check::fail(
            "claude",
            format!(
                "{version} at {} is older than {}; run `claude update`",
                claude.exe.display(),
                *SUPPORTED
            ),
        ),
        Standing::Tested => Check::ok("claude", format!("{version} at {}", claude.exe.display())),
        Standing::Untested => Check::warn(
            "claude",
            format!(
                "{version} at {} is newer than {}, the last version tested; expected to work",
                claude.exe.display(),
                *TESTED_THROUGH
            ),
        ),
    }
}

/// Budd's Claude config folder exists.
pub fn check_config_dir(ctx: &Context) -> Check {
    let dir = ctx.home.claude_dir();
    if dir.is_dir() {
        Check::ok("folder", dir.display().to_string())
    } else {
        Check::fail("folder", format!("{} is missing; run `budd setup`", dir.display()))
    }
}

/// The folder is logged in to claude.ai.
pub fn check_account(ctx: &Context) -> Check {
    let Some(claude) = &ctx.claude else {
        return Check::fail("account", "cannot be checked without claude");
    };
    match claude.auth_status() {
        Ok(status) => account_check(&status.auth_method, status.logged_in, &status.summary()),
        Err(error) => Check::fail("account", format!("claude auth status failed: {error}")),
    }
}

/// The account line for what `claude auth status` said, apart from running it.
pub fn account_check(auth_method: &Option<String>, logged_in: bool, summary: &str) -> Check {
    if !logged_in {
        return Check::fail("account", "not logged in; run `budd connect`");
    }
    match auth_method.as_deref() {
        None | Some("claude.ai") => Check::ok("account", summary.to_owned()),
        Some(method) => Check::warn(
            "account",
            format!(
                "{summary}, logged in by {method} ({}); Budd's sessions should use the claude.ai login",
                auth_source(method)
            ),
        ),
    }
}

/// Where a login that is not claude.ai's comes from, as Claude Code names the methods.
pub fn auth_source(method: &str) -> &'static str {
    let lower = method.to_lowercase();
    if lower.contains("helper") {
        "the apiKeyHelper in settings.json"
    } else if lower.contains("oauth") || lower.contains("token") {
        "the CLAUDE_CODE_OAUTH_TOKEN variable"
    } else if lower.contains("bedrock") {
        "the CLAUDE_CODE_USE_BEDROCK variable"
    } else if lower.contains("vertex") {
        "the CLAUDE_CODE_USE_VERTEX variable"
    } else if lower.contains("foundry") {
        "the CLAUDE_CODE_USE_FOUNDRY variable"
    } else if lower.contains("console") || lower.contains("api") {
        "the ANTHROPIC_API_KEY variable or an API key set in claude"
    } else {
        "a method Budd does not know"
    }
}

/// The variables this process has set that Budd's sessions never see.
pub fn check_env() -> Check {
    let dropped: Vec<&str> = DROPPED_VARS
        .iter()
        .copied()
        .filter(|name| std::env::var_os(name).is_some())
        .collect();
    env_check(&dropped)
}

/// The env line for the dropped variables found.
pub fn env_check(dropped: &[&str]) -> Check {
    if dropped.is_empty() {
        Check::ok("env", "no login override variables set")
    } else {
        Check::warn(
            "env",
            format!(
                "{} set here; Budd's sessions do not get {}, and run on the folder's login",
                dropped.join(", "),
                if dropped.len() == 1 { "it" } else { "them" }
            ),
        )
    }
}

/// `settings.json` has this binary as the exec-form hook on every event and auto mode.
pub fn check_hooks(ctx: &Context) -> Check {
    let path = ctx.home.claude_dir().join(crate::crew::SETTINGS_FILE);
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(value) => Some(value),
            Err(error) => return Check::fail("hooks", format!("{} is not JSON: {error}", path.display())),
        },
        Err(_) => None,
    };
    let (_, changed) = settings::reconcile(existing, &ctx.exe.to_string_lossy());
    if changed {
        Check::warn(
            "hooks",
            format!(
                "{} does not run this binary on every event; run `budd setup`",
                path.display()
            ),
        )
    } else {
        Check::ok(
            "hooks",
            format!("{} events in {}", settings::EVENTS.len(), path.display()),
        )
    }
}

/// The MCP server `budd` is registered to this binary.
pub fn check_mcp(ctx: &Context) -> Check {
    let Some(claude) = &ctx.claude else {
        return Check::fail("mcp", "cannot be checked without claude");
    };
    match claude.mcp_get(SERVER_NAME) {
        Ok(output) if registered_to(&output, &ctx.exe) => {
            Check::ok("mcp", format!("{SERVER_NAME} runs {}", ctx.exe.display()))
        }
        Ok(_) => Check::warn(
            "mcp",
            format!("{SERVER_NAME} is registered to another binary; run `budd setup`"),
        ),
        Err(_) => Check::warn("mcp", format!("{SERVER_NAME} is not registered; run `budd setup`")),
    }
}

/// Whether `claude mcp get`'s output says the server runs `exe mcp`.
pub fn registered_to(output: &str, exe: &Path) -> bool {
    let exe = exe.to_string_lossy();
    output.contains(exe.as_ref())
        && output
            .lines()
            .any(|line| line.trim_start().starts_with("Args:") && line.split_whitespace().any(|word| word == "mcp"))
}

/// The endpoint fits and its folder is private.
pub fn check_endpoint(ctx: &Context) -> Check {
    match ctx.home.endpoint() {
        Endpoint::NamedPipe(name) => Check::ok("endpoint", name),
        Endpoint::UnixSocket(path) => {
            let length = path.as_os_str().len() + 1;
            if length > SOCKET_PATH_LIMIT {
                return Check::fail(
                    "endpoint",
                    format!(
                        "{} is {length} bytes, over the {SOCKET_PATH_LIMIT} a socket path may have; set BUDD_HOME to a shorter path",
                        path.display()
                    ),
                );
            }
            match run_dir_mode(&ctx.home.run_dir()) {
                None => Check::warn(
                    "endpoint",
                    format!("{} (run folder not created yet; run `budd setup`)", path.display()),
                ),
                Some(mode) if mode & 0o077 != 0 => Check::fail(
                    "endpoint",
                    format!(
                        "{} is mode {mode:o}, open to other users; run `budd setup`",
                        ctx.home.run_dir().display()
                    ),
                ),
                Some(_) => Check::ok("endpoint", path.display().to_string()),
            }
        }
    }
}

/// The permission bits of the run folder, or `None` when it is not there.
#[cfg(unix)]
fn run_dir_mode(dir: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(dir).ok().map(|m| m.permissions().mode() & 0o777)
}

/// Windows folders under the profile are private by inheritance; there is nothing to read.
#[cfg(not(unix))]
fn run_dir_mode(dir: &Path) -> Option<u32> {
    dir.is_dir().then_some(0o700)
}

/// The daemon is running: `run/daemon.json` names a live process.
pub fn check_daemon(ctx: &Context) -> Check {
    let path = ctx.home.run_dir().join("daemon.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Check::warn("daemon", "not running; start it with `budd daemon`");
    };
    let advert: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => {
            return Check::warn(
                "daemon",
                format!("{} is not JSON; start `budd daemon` again", path.display()),
            );
        }
    };
    let pid = advert.get("pid").and_then(Value::as_u64).map(|pid| pid as u32);
    match pid {
        Some(pid) if procs::is_alive(pid) => Check::ok(
            "daemon",
            format!(
                "pid {pid}, version {}",
                advert.get("version").and_then(Value::as_str).unwrap_or("?")
            ),
        ),
        Some(pid) => Check::warn("daemon", format!("pid {pid} is gone; start it with `budd daemon`")),
        None => Check::warn(
            "daemon",
            format!("{} names no pid; start `budd daemon` again", path.display()),
        ),
    }
}

/// A tool on `PATH`, wanted by the kit but not by Budd himself.
pub fn check_tool(name: &str, why: &str) -> Check {
    match which::which(name) {
        Ok(path) => Check::ok(name, path.display().to_string()),
        Err(_) => Check::warn(name, format!("not on PATH; {why}")),
    }
}

/// Git for Windows: `git` and `bash.exe`, on `PATH` or under Program Files, as the kit's hooks are
/// shell scripts.
pub fn check_git_for_windows() -> Check {
    let program_files = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files"));
    let git = which::which("git")
        .ok()
        .or_else(|| Some(program_files.join("Git").join("cmd").join("git.exe")).filter(|p| p.is_file()));
    let bash = which::which("bash.exe")
        .ok()
        .or_else(|| Some(program_files.join("Git").join("bin").join("bash.exe")).filter(|p| p.is_file()));
    match (git, bash) {
        (Some(git), Some(bash)) => Check::ok("git", format!("{} and {}", git.display(), bash.display())),
        (Some(git), None) => Check::warn(
            "git",
            format!(
                "{} found, but no bash.exe; install Git for Windows with its bash",
                git.display()
            ),
        ),
        _ => Check::fail("git", "Git for Windows is not installed; install it from git-scm.com"),
    }
}

/// The WebView2 runtime the office needs, from the registry.
pub fn check_webview2() -> Check {
    match webview2_version() {
        Some(version) => Check::ok("webview2", format!("runtime {version}")),
        None if cfg!(windows) => Check::fail(
            "webview2",
            "the WebView2 runtime is not installed; Windows 11 has it, Windows 10 gets it from Microsoft",
        ),
        None => Check::ok("webview2", "not needed on this platform"),
    }
}

/// The `pv` value under the Edge Update client key of the Evergreen WebView2 runtime, per machine or
/// per user. UNVERIFIED: written against Microsoft's documented key paths and the windows-sys
/// signatures, not yet run on a Windows machine; a wrong value size or a missing key reads as `None`.
#[cfg(windows)]
fn webview2_version() -> Option<String> {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    };

    const CLIENT: &str = r"Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let wide = |text: &str| -> Vec<u16> { text.encode_utf16().chain(std::iter::once(0)).collect() };
    let places: [(HKEY, String); 3] = [
        (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\WOW6432Node\{CLIENT}")),
        (HKEY_LOCAL_MACHINE, format!(r"SOFTWARE\{CLIENT}")),
        (HKEY_CURRENT_USER, format!(r"SOFTWARE\{CLIENT}")),
    ];
    for (root, sub_key) in places {
        let sub_key = wide(&sub_key);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: the strings are nul-terminated and outlive the calls; `key` receives a handle that is
        // closed below; the buffer is sized by the first query and its length passed to the second.
        unsafe {
            if RegOpenKeyExW(root, sub_key.as_ptr(), 0, KEY_READ, &mut key) != ERROR_SUCCESS {
                continue;
            }
            let value = wide("pv");
            let mut kind = 0u32;
            let mut size = 0u32;
            let probed = RegQueryValueExW(
                key,
                value.as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                std::ptr::null_mut(),
                &mut size,
            );
            if probed != ERROR_SUCCESS || kind != REG_SZ || size == 0 {
                RegCloseKey(key);
                continue;
            }
            let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
            let read = RegQueryValueExW(
                key,
                value.as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                buffer.as_mut_ptr().cast(),
                &mut size,
            );
            RegCloseKey(key);
            if read != ERROR_SUCCESS {
                continue;
            }
            let text = String::from_utf16_lossy(&buffer);
            let version = text.trim_end_matches('\0').trim().to_owned();
            // Edge Update writes "0.0.0.0" where the runtime was removed.
            if !version.is_empty() && version != "0.0.0.0" {
                return Some(version);
            }
        }
    }
    None
}

/// Only Windows has a registry.
#[cfg(not(windows))]
fn webview2_version() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_states_and_the_worst_of_them() {
        let checks = vec![
            Check::ok("budd", "0.0.0"),
            Check::warn("node", "not on PATH"),
            Check::ok("claude", "2.1.316"),
        ];
        assert_eq!(worst(&checks), State::Warn);
        assert_eq!(worst(&[]), State::Ok);
        assert_eq!(worst(&[Check::fail("x", "y"), Check::warn("a", "b")]), State::Fail);
        assert_eq!(checks[0].line(), "ok    budd       0.0.0");
        assert_eq!(checks[1].line(), "warn  node       not on PATH");
        let json = serde_json::to_value(&checks[1]).unwrap();
        assert_eq!(json["state"], "warn");
        assert_eq!(json["name"], "node");
    }

    #[test]
    fn the_account_line_wants_the_claude_ai_login() {
        assert_eq!(account_check(&None, false, "Not logged in").state, State::Fail);
        let ok = account_check(&Some("claude.ai".to_owned()), true, "a@b · Max plan");
        assert_eq!(ok.state, State::Ok);
        assert_eq!(ok.detail, "a@b · Max plan");
        let console = account_check(&Some("console".to_owned()), true, "a@b");
        assert_eq!(console.state, State::Warn);
        assert!(console.detail.contains("ANTHROPIC_API_KEY"), "{}", console.detail);
        assert!(auth_source("oauthToken").contains("CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(auth_source("apiKeyHelper").contains("apiKeyHelper"));
        assert!(auth_source("bedrock").contains("BEDROCK"));
        assert!(auth_source("something").contains("does not know"));
        // A version that does not say the method is taken as claude.ai.
        assert_eq!(account_check(&None, true, "a@b").state, State::Ok);
    }

    #[test]
    fn the_env_line_lists_what_the_sessions_do_not_get() {
        assert_eq!(env_check(&[]).state, State::Ok);
        let one = env_check(&["ANTHROPIC_API_KEY"]);
        assert_eq!(one.state, State::Warn);
        assert!(
            one.detail
                .starts_with("ANTHROPIC_API_KEY set here; Budd's sessions do not get it")
        );
        let two = env_check(&["ANTHROPIC_API_KEY", "CLAUDE_CODE_USE_BEDROCK"]);
        assert!(two.detail.contains("do not get them"));
    }

    #[test]
    fn the_mcp_output_must_name_this_binary_and_the_mcp_subcommand() {
        let output = "budd:\n  Scope: User config\n  Type: stdio\n  Command: /opt/budd\n  Args: mcp\n";
        assert!(registered_to(output, Path::new("/opt/budd")));
        assert!(!registered_to(output, Path::new("/new/budd")));
        let other = "budd:\n  Type: stdio\n  Command: /opt/budd\n  Args: hook Stop\n";
        assert!(!registered_to(other, Path::new("/opt/budd")));
        assert!(!registered_to("No MCP server found", Path::new("/opt/budd")));
    }

    #[test]
    fn the_file_checks_read_a_temp_home() {
        let temp = tempfile::tempdir().unwrap();
        let home = BuddHome::at(temp.path().join("budd"));
        let ctx = Context {
            home: home.clone(),
            exe: PathBuf::from("/opt/budd"),
            claude: None,
            version: "0.0.0".to_owned(),
        };
        assert_eq!(check_budd(&ctx).state, State::Ok);
        assert_eq!(check_claude(&ctx).state, State::Fail);
        assert_eq!(check_account(&ctx).state, State::Fail);
        assert_eq!(check_mcp(&ctx).state, State::Fail);
        assert_eq!(check_config_dir(&ctx).state, State::Fail);
        assert_eq!(check_hooks(&ctx).state, State::Warn, "no settings yet");
        assert_eq!(check_daemon(&ctx).state, State::Warn);
        // Setup's settings make the hooks line ok.
        std::fs::create_dir_all(home.claude_dir()).unwrap();
        let (settings, _) = settings::reconcile(None, "/opt/budd");
        std::fs::write(
            home.claude_dir().join("settings.json"),
            serde_json::to_string(&settings).unwrap(),
        )
        .unwrap();
        assert_eq!(check_config_dir(&ctx).state, State::Ok);
        assert_eq!(check_hooks(&ctx).state, State::Ok);
        std::fs::write(home.claude_dir().join("settings.json"), "{bad").unwrap();
        assert_eq!(check_hooks(&ctx).state, State::Fail);
        // The daemon advert with our own pid is alive; with a dead pid it is gone.
        crate::paths::ensure_private_dir(&home.run_dir()).unwrap();
        std::fs::write(
            home.run_dir().join("daemon.json"),
            format!(
                r#"{{"pid": {}, "version": "0.0.0", "endpoint": "x"}}"#,
                std::process::id()
            ),
        )
        .unwrap();
        let daemon = check_daemon(&ctx);
        assert_eq!(daemon.state, State::Ok, "{}", daemon.detail);
        std::fs::write(home.run_dir().join("daemon.json"), r#"{"pid": 4000000000}"#).unwrap();
        assert_eq!(check_daemon(&ctx).state, State::Warn);
        std::fs::write(home.run_dir().join("daemon.json"), "nope").unwrap();
        assert_eq!(check_daemon(&ctx).state, State::Warn);
        // The endpoint check: the run folder is private now, so a short path is ok.
        let endpoint = check_endpoint(&ctx);
        if cfg!(unix) {
            assert_eq!(endpoint.state, State::Ok, "{}", endpoint.detail);
            let long = Context {
                home: BuddHome::at(PathBuf::from("/").join("x".repeat(120))),
                ..ctx.clone()
            };
            assert_eq!(check_endpoint(&long).state, State::Fail);
        }
        let whole = run(&ctx);
        assert!(whole.len() >= 11);
        assert_eq!(whole[0].name, "budd");
        assert_eq!(worst(&whole), State::Fail);
    }

    #[test]
    fn the_claude_checks_run_the_fake() {
        let Some(exe) = crate::test_support::fake_claude() else {
            return;
        };
        let temp = tempfile::tempdir().unwrap();
        let home = BuddHome::at(temp.path().join("budd"));
        std::fs::create_dir_all(home.claude_dir()).unwrap();
        let auth = temp.path().join("auth.json");
        std::fs::write(
            home.claude_dir().join("fake-claude.env"),
            format!("FAKE_CLAUDE_AUTH={}\n", auth.display()),
        )
        .unwrap();
        let ctx = Context {
            home: home.clone(),
            exe: PathBuf::from("/opt/budd"),
            claude: Some(Claude::new(&exe, home.claude_dir())),
            version: "0.0.0".to_owned(),
        };
        let claude = check_claude(&ctx);
        assert_eq!(claude.state, State::Ok, "{}", claude.detail);
        assert!(claude.detail.starts_with("2.1.316 at "));
        assert_eq!(check_account(&ctx).state, State::Fail, "no auth file: logged out");
        std::fs::write(
            &auth,
            r#"{"loggedIn": true, "authMethod": "claude.ai", "email": "budd@example.com"}"#,
        )
        .unwrap();
        let account = check_account(&ctx);
        assert_eq!(account.state, State::Ok, "{}", account.detail);
        assert!(account.detail.starts_with("budd@example.com"));
        std::fs::write(&auth, r#"{"loggedIn": true, "authMethod": "console"}"#).unwrap();
        assert_eq!(check_account(&ctx).state, State::Warn);
        assert_eq!(check_mcp(&ctx).state, State::Warn, "not registered");
        ctx.claude
            .as_ref()
            .unwrap()
            .mcp_add(SERVER_NAME, Path::new("/opt/budd"), &["mcp"])
            .unwrap();
        let mcp = check_mcp(&ctx);
        assert_eq!(mcp.state, State::Ok, "{}", mcp.detail);
        let moved = Context {
            exe: PathBuf::from("/new/budd"),
            ..ctx.clone()
        };
        assert_eq!(check_mcp(&moved).state, State::Warn);
    }
}
