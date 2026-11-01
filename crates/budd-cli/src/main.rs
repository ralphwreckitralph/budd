//! The `budd` binary.
//!
//! Subcommands land here as the adapters behind them land in `budd_claude` and `budd_engine`.

use budd_claude::claude_cli::{self, Claude};
use budd_claude::doctor;
use budd_claude::kit::{self, evidence, expect, fingerprint, handover};
use budd_claude::messenger::{self, Message, Outcome, Target, Via};
use budd_claude::paths::BuddHome;
use budd_claude::setup::{self, ConnectOptions, Terminal};
use budd_core::delivery::PathStyle;
use budd_proto::{Command as DaemonCommand, Request};
use clap::{Parser, Subcommand};
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

/// How long `budd reset` gives the daemon to forget and stop the assistant.
const RESET_WAIT: Duration = Duration::from_secs(30);

/// Budd: an office for your Claude Code background sessions.
#[derive(Parser)]
#[command(name = "budd", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// The lifecycle hook Claude Code runs on every session event (reads the event on stdin).
    Hook {
        /// The event's name, as the settings pass it; the event on stdin is the one that counts.
        #[arg(value_name = "EVENT")]
        event: Option<String>,
        /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
        #[arg(long, value_name = "DIR")]
        home: Option<PathBuf>,
        /// Also write every state and reply file into this folder (for the existing macOS app).
        #[arg(long, value_name = "DIR")]
        legacy_state_dir: Option<PathBuf>,
    },
    /// Budd's MCP server on stdio, started by Claude Code for each session.
    Mcp,
    /// The daemon: follows the sessions, serves the app and the tools, keeps the assistant alive.
    Daemon {
        /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
        #[arg(long, value_name = "DIR")]
        home: Option<PathBuf>,
        /// The Claude config folder of Budd's account, instead of `<home>/claude`.
        #[arg(long, value_name = "DIR")]
        config_dir: Option<PathBuf>,
        /// The assistant session's name, instead of "assistant".
        #[arg(long, value_name = "NAME")]
        assistant_name: Option<String>,
        /// The `claude` binary, instead of the one found.
        #[arg(long, value_name = "PATH")]
        claude: Option<PathBuf>,
    },
    /// Checks the install: claude, Budd's account, the hooks, the MCP server, the endpoint, the daemon.
    ///
    /// Exits 0 when nothing failed, 1 when something must be fixed before Budd works.
    Doctor {
        /// Print the checks as JSON, one object per check.
        #[arg(long)]
        json: bool,
        /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
        #[arg(long, value_name = "DIR")]
        home: Option<PathBuf>,
        /// The `claude` binary, instead of the one found.
        #[arg(long, value_name = "PATH")]
        claude: Option<PathBuf>,
    },
    /// Prepares Budd's home: the folders, the hooks, the MCP server, the assistant's persona, config.json.
    ///
    /// Safe to run again: what is already right is kept, and the human's own edits stay.
    Setup {
        /// Say what would be done without doing it.
        #[arg(long)]
        dry_run: bool,
        /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
        #[arg(long, value_name = "DIR")]
        home: Option<PathBuf>,
        /// The `claude` binary, instead of the one found.
        #[arg(long, value_name = "PATH")]
        claude: Option<PathBuf>,
    },
    /// Logs Budd's own Claude folder in to claude.ai, in a terminal window, and waits for it.
    Connect {
        /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
        #[arg(long, value_name = "DIR")]
        home: Option<PathBuf>,
        /// The `claude` binary, instead of the one found.
        #[arg(long, value_name = "PATH")]
        claude: Option<PathBuf>,
        /// Where the login opens: "terminal" (the platform's) or "iterm".
        #[arg(long, value_name = "NAME", default_value = "terminal")]
        terminal: String,
        /// How many seconds to wait for the login.
        #[arg(long, value_name = "SECONDS", default_value_t = setup::LOGIN_WAIT.as_secs())]
        wait: u64,
        /// A Claude config folder (usually ~/.claude) whose trusted folders Budd's folder takes on.
        #[arg(long, value_name = "DIR")]
        carry_trust_from: Option<PathBuf>,
        /// A Claude config folder whose memory for the assistant's folder is copied over, once.
        #[arg(long, value_name = "DIR")]
        import_memory_from: Option<PathBuf>,
    },
    /// Forgets what Budd learned while running and stops the assistant, so he starts over.
    ///
    /// The config, Budd's Claude folder and its login stay, unless `--sign-out` is given.
    Reset {
        /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
        #[arg(long, value_name = "DIR")]
        home: Option<PathBuf>,
        /// The `claude` binary, instead of the one found.
        #[arg(long, value_name = "PATH")]
        claude: Option<PathBuf>,
        /// Also log Budd's Claude folder out.
        #[arg(long)]
        sign_out: bool,
    },
    /// Connects to the daemon and prints every line it sends, as JSON.
    Watch {
        /// Print the raw JSON lines (the only form for now).
        #[arg(long)]
        json: bool,
        /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
        #[arg(long, value_name = "DIR")]
        home: Option<PathBuf>,
    },
    /// Type a message into a background session through `claude attach` and say whether it arrived.
    ///
    /// Prints one JSON line: {"outcome":"delivered","via":"pty"} or {"outcome":"unconfirmed","reason":"..."}.
    /// Exits 0 when delivered and 1 when not.
    Send {
        /// The Claude config folder the session runs under (`CLAUDE_CONFIG_DIR`).
        #[arg(long, value_name = "DIR")]
        config_dir: PathBuf,
        /// The session's short id, as `claude agents` lists it.
        #[arg(long, value_name = "SHORT ID")]
        session: String,
        /// The session's full session id (not needed to send).
        #[arg(long, value_name = "ID")]
        session_id: Option<String>,
        /// The session's transcript, where the message is confirmed; without it a send is never confirmed.
        #[arg(long, value_name = "PATH")]
        transcript: Option<PathBuf>,
        /// The session is asleep (retired while it waited on you), so `claude attach` wakes it first.
        #[arg(long)]
        asleep: bool,
        /// A picture to attach, as a path without spaces; repeatable.
        #[arg(long, value_name = "PATH")]
        picture: Vec<PathBuf>,
        /// A file to drop on the session, pasted as its escaped path after the text; repeatable.
        #[arg(long, value_name = "PATH")]
        file: Vec<PathBuf>,
        /// The `claude` binary, instead of the one Budd finds.
        #[arg(long, value_name = "EXE")]
        claude: Option<PathBuf>,
        /// The message.
        text: String,
    },
    /// Clear a background session's conversation, as `/clear` typed in its terminal would.
    ///
    /// Prints one JSON line as `send` does; the clear counts once `claude agents` lists the session
    /// with a new session id. Exits 0 when it did and 1 when not.
    Clear {
        /// The Claude config folder the session runs under (`CLAUDE_CONFIG_DIR`).
        #[arg(long, value_name = "DIR")]
        config_dir: PathBuf,
        /// The session's short id, as `claude agents` lists it.
        #[arg(long, value_name = "SHORT ID")]
        session: String,
        /// The session id it has now, which the clear replaces.
        #[arg(long, value_name = "ID")]
        session_id: String,
        /// The `claude` binary, instead of the one Budd finds.
        #[arg(long, value_name = "EXE")]
        claude: Option<PathBuf>,
    },
    /// The evidence set of a ticket: each expectation, how it was proven, and its BEFORE and AFTER frames.
    ///
    /// Sets live at `<config dir>/evidence/<TICKET>/`, where the config dir is `CLAUDE_CONFIG_DIR`, else
    /// Budd's own `<home>/claude`. Exits 0 ok, 1 with gaps (check), 2 on a usage or input error.
    Evidence {
        #[command(subcommand)]
        command: EvidenceCommand,
        /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
        #[arg(long, global = true, value_name = "DIR")]
        home: Option<PathBuf>,
    },
    /// The per-worktree handover document, kept under the worktree's git dir where no commit can carry it.
    Handover {
        #[command(subcommand)]
        command: HandoverCommand,
        /// The worktree (or a folder inside it); the current folder when not given.
        #[arg(long, global = true, visible_alias = "repo", value_name = "DIR")]
        worktree: Option<PathBuf>,
        /// Budd's home folder, instead of `BUDD_HOME` or `~/.budd`.
        #[arg(long, global = true, value_name = "DIR")]
        home: Option<PathBuf>,
    },
    /// The expectation document's shape: Problems, Expected, Potential risk, one sentence per bullet.
    Expect {
        #[command(subcommand)]
        command: ExpectCommand,
    },
    /// The fingerprint of a working tree: its git tree oid with untracked files in and build output out.
    ///
    /// Computed in a throwaway index, so the real index is never touched. Prints the oid, or exits 1.
    Fingerprint {
        /// The repository, or a folder inside it; the current folder when not given.
        #[arg(value_name = "REPO")]
        repo: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum EvidenceCommand {
    /// Start the set (or re-sync it with the document) from the `expectations.json` sidecar.
    Init {
        /// The ticket key.
        ticket: String,
        /// The sidecar `budd expect sidecar` wrote.
        #[arg(long, value_name = "FILE")]
        expectations: PathBuf,
    },
    /// Say whether a screen shows the expectation.
    Screen {
        /// The ticket key.
        ticket: String,
        /// The expectation id, such as E1.
        id: String,
        /// yes or no.
        answer: String,
    },
    /// Copy a BEFORE or AFTER frame into the set.
    Add {
        /// The ticket key.
        ticket: String,
        /// The expectation id, such as E1.
        id: String,
        /// before or after.
        when: String,
        /// The image (png, jpg, jpeg, gif or webp).
        image: PathBuf,
        /// The page the frame shows.
        #[arg(long, value_name = "URL")]
        url: Option<String>,
        /// What was done on the page before the frame.
        #[arg(long, value_name = "TEXT")]
        steps: Option<String>,
        /// The recipe that took it, kept as E<n>.recipe.mjs for the AFTER replay.
        #[arg(long, value_name = "FILE")]
        recipe: Option<PathBuf>,
    },
    /// Record the test or check that proves the expectation.
    Proof {
        /// The ticket key.
        ticket: String,
        /// The expectation id, such as E1.
        id: String,
        /// The proof.
        text: String,
    },
    /// Name every gap; exits 1 when there is one.
    Check {
        /// The ticket key.
        ticket: String,
    },
    /// List each expectation with its frames and proof.
    Show {
        /// The ticket key.
        ticket: String,
    },
    /// Take a frame headless: only with the evidence-shots module, which this build does not have.
    Shoot {
        /// Its arguments, which this build does not read.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        rest: Vec<String>,
    },
}

#[derive(Subcommand)]
enum HandoverCommand {
    /// Print the handover folder: `<git dir>/budd-handover`.
    Home,
    /// Write a section, replacing the one with the same heading; the body comes from --from or stdin.
    Append {
        /// The section's title.
        #[arg(value_name = "SECTION", required_unless_present = "title")]
        section: Option<String>,
        /// The section's title, as --title.
        #[arg(long, conflicts_with = "section", value_name = "TITLE")]
        title: Option<String>,
        /// The skill writing it, heading the section as `<skill> - <title>`.
        #[arg(long, value_name = "NAME")]
        skill: Option<String>,
        /// Read the body from this file instead of stdin.
        #[arg(long, visible_alias = "body-file", value_name = "FILE")]
        from: Option<PathBuf>,
        /// Stamp this session's token usage into the section (an id, or a transcript path).
        #[arg(long, value_name = "ID")]
        session: Option<String>,
    },
    /// Print the document, each section marked CURRENT or SUPERSEDED against the live tree.
    Show {
        /// Only this section (its title, or its full heading).
        #[arg(value_name = "SECTION")]
        section: Option<String>,
    },
    /// Print a session's token usage, as a section would carry it.
    Tokens {
        /// The session id, or a transcript path.
        #[arg(value_name = "SESSION")]
        session: String,
    },
}

#[derive(Subcommand)]
enum ExpectCommand {
    /// Check the document; exit 0 is the only pass.
    Check {
        /// The document.
        file: PathBuf,
        /// Print the findings as a JSON list.
        #[arg(long)]
        json: bool,
    },
    /// Check the document and write the acceptance sidecar, one item per Expected bullet as E1..En.
    Sidecar {
        /// The document.
        file: PathBuf,
        /// The ticket the sidecar is for.
        #[arg(long, value_name = "KEY")]
        ticket: Option<String>,
        /// Where to write `expectations.json`.
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
    },
}

/// The Budd home the command line or the environment names.
fn budd_home(home: Option<PathBuf>) -> Result<BuddHome, String> {
    match home {
        Some(home) => Ok(BuddHome::at(home)),
        None => BuddHome::resolve().map_err(|error| error.to_string()),
    }
}

/// Where to look for `claude` first: the command line's path, else the config file's `claude_path`.
fn claude_hint(flag: Option<PathBuf>, home: &BuddHome) -> Option<PathBuf> {
    flag.or_else(|| {
        budd_claude::config::Config::load(&home.config_path())
            .ok()
            .and_then(|config| config.claude_path)
    })
}

/// The `claude` Budd's own folder runs, when one is found: the hint, the usual places, then `PATH`.
fn budds_claude(flag: Option<PathBuf>, home: &BuddHome) -> Option<Claude> {
    let hint = claude_hint(flag, home);
    claude_cli::locate(hint.as_deref()).map(|exe| {
        let exe = std::path::absolute(&exe).unwrap_or(exe);
        Claude::new(exe, home.claude_dir())
    })
}

/// This binary, as the hooks and the MCP server must name it.
fn this_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|error| format!("finding this binary: {error}"))?;
    Ok(std::path::absolute(&exe).unwrap_or(exe))
}

fn run_doctor(json: bool, home: Option<PathBuf>, claude: Option<PathBuf>) -> Result<bool, String> {
    let home = budd_home(home)?;
    let hint = claude_hint(claude, &home);
    let context = doctor::Context::resolve(home, hint.as_deref());
    let checks = doctor::run(&context);
    if json {
        let text = serde_json::to_string_pretty(&checks).map_err(|error| error.to_string())?;
        println!("{text}");
    } else {
        for check in &checks {
            println!("{}", check.line());
        }
    }
    Ok(doctor::worst(&checks) != doctor::State::Fail)
}

fn run_setup(dry_run: bool, home: Option<PathBuf>, claude: Option<PathBuf>) -> Result<(), String> {
    let home = budd_home(home)?;
    let claude = budds_claude(claude, &home);
    if claude.is_none() {
        eprintln!(
            "budd: claude was not found, so the MCP server is not registered; run setup again once it is installed"
        );
    }
    let exe = this_exe()?;
    let lines = setup::setup(&home, claude.as_ref(), &exe, dry_run).map_err(|error| error.to_string())?;
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

fn run_connect(
    home: Option<PathBuf>,
    claude: Option<PathBuf>,
    terminal: &str,
    wait: u64,
    carry_trust_from: Option<PathBuf>,
    import_memory_from: Option<PathBuf>,
) -> Result<(), String> {
    let home = budd_home(home)?;
    let terminal =
        Terminal::parse(terminal).ok_or_else(|| format!("{terminal:?} is not a terminal; use terminal or iterm"))?;
    let claude = budds_claude(claude, &home).ok_or_else(|| setup::Error::NoClaude.to_string())?;
    let options = ConnectOptions {
        terminal,
        wait: Duration::from_secs(wait),
        carry_trust_from,
        import_memory_from,
    };
    let mut out = std::io::stdout();
    setup::connect(&home, &claude, &options, &mut out)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn run_reset(home: Option<PathBuf>, claude: Option<PathBuf>, sign_out: bool) -> Result<(), String> {
    let home = budd_home(home)?;
    let claude = budds_claude(claude, &home);
    let endpoint = home.endpoint();
    let mut via_daemon = || {
        let pid = doctor::running_daemon(&home)?;
        Some(ask_daemon(&endpoint, DaemonCommand::Reset, RESET_WAIT).map_err(|error| format!("pid {pid}: {error}")))
    };
    let report = setup::reset(&home, claude.as_ref(), sign_out, &mut via_daemon).map_err(|error| error.to_string())?;
    if report.via_daemon {
        println!("The daemon forgot what it learned and stopped the assistant; the keep-alive starts a fresh one.");
    } else {
        println!("No daemon running; the state was reset on disk.");
    }
    if report.state_deleted {
        println!("Deleted {}", home.state_dir().join(setup::STATE_FILE).display());
    }
    if report.drops_removed > 0 {
        println!(
            "Removed {} dropped files from {}",
            report.drops_removed,
            home.drops_dir().display()
        );
    }
    if report.signed_out {
        println!("Signed out of Budd's Claude folder; run `budd connect` to log in again.");
    }
    Ok(())
}

/// Sends `command` to the daemon at `endpoint` and waits for its reply, for up to `wait`.
fn ask_daemon(endpoint: &budd_claude::ipc::Endpoint, command: DaemonCommand, wait: Duration) -> Result<(), String> {
    let runtime = tokio::runtime::Runtime::new().map_err(|error| format!("starting the runtime: {error}"))?;
    let request = Request { id: Some(1), command };
    let reply = runtime
        .block_on(budd_engine::client::request(endpoint, &request, wait))
        .map_err(|error| {
            if budd_claude::ipc::is_not_running(&error) {
                format!("no budd daemon at {endpoint}")
            } else {
                error.to_string()
            }
        })?;
    if reply.ok {
        Ok(())
    } else {
        Err(reply.error.unwrap_or_else(|| "the daemon refused".to_owned()))
    }
}

/// Sends the daemon's log to `<home>/logs/daemon.log`, at the level `BUDD_LOG` names (info by default).
fn log_to(home: &budd_claude::paths::BuddHome) -> Result<(), String> {
    let logs = home.logs_dir();
    std::fs::create_dir_all(&logs).map_err(|error| format!("creating {}: {error}", logs.display()))?;
    let file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(logs.join("daemon.log"))
        .map_err(|error| format!("opening the log: {error}"))?;
    let filter = tracing_subscriber::EnvFilter::try_from_env("BUDD_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .try_init()
        .map_err(|error| format!("setting up the log: {error}"))
}

fn run_daemon(options: budd_engine::daemon::Options) -> Result<(), String> {
    let home = budd_home(options.home.clone())?;
    log_to(&home)?;
    let runtime = tokio::runtime::Runtime::new().map_err(|error| format!("starting the runtime: {error}"))?;
    let mut options = options;
    options.home = Some(home.root().to_path_buf());
    runtime
        .block_on(budd_engine::daemon::run(options))
        .map_err(|error| error.to_string())
}

fn run_watch(home: Option<PathBuf>) -> Result<(), String> {
    let home = budd_home(home)?;
    let endpoint = home.endpoint();
    let runtime = tokio::runtime::Runtime::new().map_err(|error| format!("starting the runtime: {error}"))?;
    let mut out = std::io::stdout();
    runtime
        .block_on(budd_engine::client::watch(&endpoint, "watch", &mut out))
        .map_err(|error| {
            if budd_claude::ipc::is_not_running(&error) {
                format!("no budd daemon at {endpoint}")
            } else {
                error.to_string()
            }
        })
}

/// The Claude config folder the kit's state lives under.
fn kit_config_dir(home: Option<PathBuf>) -> Result<PathBuf, String> {
    Ok(kit::config_dir(&budd_home(home)?))
}

fn run_evidence(command: EvidenceCommand, home: Option<PathBuf>) -> ExitCode {
    let command = match command {
        EvidenceCommand::Init { ticket, expectations } => evidence::Command::Init { ticket, expectations },
        EvidenceCommand::Screen { ticket, id, answer } => evidence::Command::Screen { ticket, id, answer },
        EvidenceCommand::Add {
            ticket,
            id,
            when,
            image,
            url,
            steps,
            recipe,
        } => evidence::Command::Add {
            ticket,
            id,
            when,
            image,
            url,
            steps,
            recipe,
        },
        EvidenceCommand::Proof { ticket, id, text } => evidence::Command::Proof { ticket, id, text },
        EvidenceCommand::Check { ticket } => evidence::Command::Check { ticket },
        EvidenceCommand::Show { ticket } => evidence::Command::Show { ticket },
        EvidenceCommand::Shoot { rest: _ } => evidence::Command::Shoot,
    };
    let outcome = kit_config_dir(home).and_then(|config_dir| {
        let mut out = std::io::stdout().lock();
        let code = evidence::run(&config_dir, &command, &mut out).map_err(|why| why.to_string());
        let _ = out.flush();
        code
    });
    match outcome {
        Ok(code) => ExitCode::from(code as u8),
        Err(why) => {
            eprintln!("evidence: {why}");
            eprintln!("{}", evidence::USAGE);
            ExitCode::from(2)
        }
    }
}

fn run_handover(command: HandoverCommand, worktree: Option<PathBuf>, home: Option<PathBuf>) -> ExitCode {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let worktree = worktree.unwrap_or_else(|| cwd.clone());
    let cwd_text = cwd.to_string_lossy().into_owned();
    let tokens_of = |session: Option<&str>| -> Result<handover::TokenFacts, String> {
        match session.filter(|s| !s.is_empty()) {
            Some(session) => Ok(handover::token_facts(
                &kit_config_dir(home.clone())?,
                session,
                &cwd_text,
            )),
            None => Ok(handover::TokenFacts::none()),
        }
    };
    let result: Result<(), (i32, String)> = (|| {
        let open = || handover::Handover::open(&worktree).map_err(|error| (error.exit_code(), error.to_string()));
        match command {
            HandoverCommand::Home => {
                println!("{}", open()?.home().display());
                Ok(())
            }
            HandoverCommand::Append {
                section,
                title,
                skill,
                from,
                session,
            } => {
                let handover = open()?;
                let title = title.or(section).unwrap_or_default();
                let body = match from {
                    Some(file) => std::fs::read_to_string(&file)
                        .map_err(|error| (2, format!("append: --from {}: {error}", file.display())))?,
                    None => {
                        let mut body = String::new();
                        let mut stdin = std::io::stdin();
                        if !stdin.is_terminal() {
                            stdin
                                .read_to_string(&mut body)
                                .map_err(|error| (2, format!("append: reading stdin: {error}")))?;
                        }
                        body
                    }
                };
                let tokens = tokens_of(session.as_deref()).map_err(|why| (2, why))?;
                let section = handover::NewSection {
                    skill: skill.as_deref(),
                    title: &title,
                    body: &body,
                };
                let document = handover
                    .append(&section, &tokens, &kit::Utc::now())
                    .map_err(|error| (error.exit_code(), error.to_string()))?;
                println!("{}", document.display());
                Ok(())
            }
            HandoverCommand::Show { section } => {
                let handover = open()?;
                let mut out = std::io::stdout().lock();
                handover
                    .show(section.as_deref(), &mut out)
                    .map_err(|error| (error.exit_code(), error.to_string()))?;
                let _ = out.flush();
                Ok(())
            }
            HandoverCommand::Tokens { session } => {
                let facts = tokens_of(Some(&session)).map_err(|why| (2, why))?;
                print!("{}", facts.lines());
                Ok(())
            }
        }
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err((code, why)) => {
            eprintln!("budd handover: {why}");
            ExitCode::from(code as u8)
        }
    }
}

fn run_expect(command: ExpectCommand) -> ExitCode {
    let (file, json, sidecar) = match command {
        ExpectCommand::Check { file, json } => (file, json, None),
        ExpectCommand::Sidecar { file, ticket, out } => (file, false, Some((ticket, out))),
    };
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("cannot read {}: {error}", file.display());
            return ExitCode::from(2);
        }
    };
    let found = expect::validate(&text);
    if json {
        println!("{}", serde_json::Value::from(found.errors.clone()));
    } else {
        for error in &found.errors {
            println!("FAIL {error}");
        }
    }
    if !found.ok() {
        return ExitCode::FAILURE;
    }
    if !json {
        println!("{}", found.summary());
    }
    if let Some((ticket, out)) = sidecar {
        let written = expect::encode_sidecar(&found.sidecar(ticket.as_deref()))
            .map_err(|error| error.to_string())
            .and_then(|text| {
                // The handover folder it goes in may not exist before its first section.
                if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                std::fs::write(&out, text).map_err(|error| error.to_string())
            });
        if let Err(error) = written {
            eprintln!("cannot write {}: {error}", out.display());
            return ExitCode::from(2);
        }
        println!("sidecar: {} (E1..E{})", out.display(), found.expected.len());
    }
    ExitCode::SUCCESS
}

fn run_fingerprint(repo: Option<PathBuf>) -> ExitCode {
    let repo = repo.unwrap_or_else(|| PathBuf::from("."));
    match fingerprint::fingerprint(&repo) {
        Ok(oid) => {
            println!("{oid}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("budd fingerprint: {}: {error}", repo.display());
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Hook {
            event: _,
            home,
            legacy_state_dir,
        } => {
            budd_claude::hookrun::main(budd_claude::hookrun::Options { home, legacy_state_dir });
            ExitCode::SUCCESS
        }
        Command::Mcp => {
            budd_claude::mcp_stdio::main();
            ExitCode::SUCCESS
        }
        Command::Daemon {
            home,
            config_dir,
            assistant_name,
            claude,
        } => exit_code(run_daemon(budd_engine::daemon::Options {
            home,
            config_dir,
            assistant_name,
            claude,
        })),
        Command::Doctor { json, home, claude } => match run_doctor(json, home, claude) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(error) => exit_code(Err(error)),
        },
        Command::Setup { dry_run, home, claude } => exit_code(run_setup(dry_run, home, claude)),
        Command::Connect {
            home,
            claude,
            terminal,
            wait,
            carry_trust_from,
            import_memory_from,
        } => exit_code(run_connect(
            home,
            claude,
            &terminal,
            wait,
            carry_trust_from,
            import_memory_from,
        )),
        Command::Reset { home, claude, sign_out } => exit_code(run_reset(home, claude, sign_out)),
        Command::Watch { json: _, home } => exit_code(run_watch(home)),
        Command::Send {
            config_dir,
            session,
            session_id,
            transcript,
            asleep,
            picture,
            file,
            claude,
            text,
        } => {
            let target = Target {
                short_id: session,
                session_id: session_id.unwrap_or_default(),
                transcript,
                asleep,
            };
            let message = Message {
                text,
                pictures: picture,
                files: file,
            };
            report(
                claude_at(claude, config_dir).map(|claude| messenger::send(&claude, &target, &message, path_style())),
            )
        }
        Command::Clear {
            config_dir,
            session,
            session_id,
            claude,
        } => {
            let target = Target {
                short_id: session,
                session_id,
                transcript: None,
                asleep: false,
            };
            report(claude_at(claude, config_dir).map(|claude| messenger::clear(&claude, &target)))
        }
        Command::Evidence { command, home } => run_evidence(command, home),
        Command::Handover {
            command,
            worktree,
            home,
        } => run_handover(command, worktree, home),
        Command::Expect { command } => run_expect(command),
        Command::Fingerprint { repo } => run_fingerprint(repo),
    }
}

/// The `claude` to drive: `exe` when given, else the one Budd finds; or why there is none. The path
/// is made absolute, since every run of it starts in another folder.
fn claude_at(exe: Option<PathBuf>, config_dir: PathBuf) -> Result<Claude, String> {
    let exe = exe
        .or_else(|| claude_cli::locate(None))
        .ok_or_else(|| "claude was not found; pass --claude <exe>".to_owned())?;
    let exe = std::path::absolute(&exe).unwrap_or(exe);
    Ok(Claude::new(exe, config_dir))
}

/// How a dropped file's path is written for the session's shell on this platform.
fn path_style() -> PathStyle {
    if cfg!(windows) {
        PathStyle::WindowsQuoted
    } else {
        PathStyle::Posix
    }
}

/// Success, or the error on stderr and a failing exit code.
fn exit_code(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("budd: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Prints the outcome as one JSON line and turns it into the exit code.
fn report(outcome: Result<Outcome, String>) -> ExitCode {
    let (line, delivered) = match outcome {
        Ok(Outcome::Delivered { via }) => {
            let via = match via {
                Via::Pty => "pty",
            };
            (serde_json::json!({"outcome": "delivered", "via": via}), true)
        }
        Ok(Outcome::Unconfirmed(reason)) | Err(reason) => {
            (serde_json::json!({"outcome": "unconfirmed", "reason": reason}), false)
        }
    };
    println!("{line}");
    if delivered {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
