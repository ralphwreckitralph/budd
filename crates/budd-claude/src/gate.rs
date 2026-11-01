//! The destructive-action gate: a PreToolUse rule set that denies irreversible and outward-facing
//! actions, asks about reversible fan-out, and lets everything else through (from
//! ~/.claude/hooks/destructive-gate.py, rule for rule).
//!
//! Two tiers. DENY is for what cannot be undone: mail, database writes and resets, bulk deletes, history
//! rewrites, force pushes, remote deletions, container wipes, payment writes, worktree removal. It clears
//! only with a consent token the human typed, scoped to the exact category: `I_CONSENT=<slug> <command>`.
//! ASK is for what is reversible but fans out over an unenumerated set: a glob move, an in-place `sed`
//! over `find` output, a `>` onto an existing file, an MCP tool that edits a shared object. It becomes a
//! normal permission prompt carrying the reason.
//!
//! Every rule runs on the command as typed and again on whatever an `ssh` segment hands to a remote
//! shell, so a destructive action does not get cheaper by being aimed at a server. The gate reads the
//! command line and not the files it names: `bash deploy.sh` passes whatever is inside, and that hole is
//! deliberate (see the kit's references/guardrail.md).
//!
//! The module is pure: it reads no file and no environment. The two things it needs from the world, whether
//! a path is an existing file and whether the human typed a consent token, come in as callbacks on
//! [`GateInput`]. The hook (`hookrun`) runs it, prints [`hook_output`] and keeps the logs.

use fancy_regex::Regex as FancyRegex;
use regex::Regex;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::LazyLock;

/// Where the protocol is explained, as the refusal messages cite it.
pub const GUARDRAIL_DOC: &str = "references/guardrail.md in the budd kit";
/// The consent slug for payment writes. The author's gate called it `stripe-write`; both tokens clear it.
pub const PAYMENT_SLUG: &str = "payment-write";
/// The older name of [`PAYMENT_SLUG`], still accepted on a typed token.
pub const STRIPE_SLUG: &str = "stripe-write";
/// The phrase a refusal carries when a token is on the command but not in anything the human typed.
pub const NOT_TYPED: &str = "consent token was not typed by the human";

/// `rm -rf` targets under any of these are ordinary housekeeping, not destruction.
pub const SAFE_RM_FRAGMENTS: &[&str] = &[
    "/claude-501/",
    "/scratchpad",
    "/.scratch/",
    "/node_modules",
    "/.next",
    "/.turbo",
    "/dist",
    "/build",
    "/vendor/bin",
    "/storage/framework/cache",
    "/storage/logs",
    "/.pytest_cache",
    "/__pycache__",
    "/coverage",
    "/playwright-report",
    "/test-results",
];

/// Wrappers that run the command after them: a rule must read through them to the real command.
const WRAPPERS: &[&str] = &["sudo", "env", "time", "nohup", "xargs", "command"];
/// The wrappers an `ssh` can hide behind. `xargs` is not one: `xargs ssh` carries no remote command.
const TRANSPARENT_WRAPPERS: &[&str] = &["sudo", "env", "time", "nohup", "command"];
/// ssh flags that consume the following argument; everything after the host token is the remote command.
const SSH_VALUE_FLAGS: &[&str] = &[
    "-p", "-i", "-o", "-l", "-F", "-J", "-L", "-R", "-D", "-c", "-m", "-b", "-e", "-w", "-Q", "-S", "-W", "-E",
];
/// Hosted mail APIs: a call to one sends real mail.
const MAIL_HOSTS: &[&str] = &[
    "api.sendgrid.com",
    "api.mailgun.net",
    "api.postmarkapp.com",
    "api.resend.com",
    "api.sparkpost.com",
    "email.us-east-1.amazonaws.com",
    "api.brevo.com",
    "api.mailjet.com",
];

/// Answers whether the human typed a consent token: given the token (`I_CONSENT=<slug>`) and the command's
/// first word, whether one message the human typed in this session holds both.
pub type ConsentCheck<'a> = &'a dyn Fn(&str, &str) -> bool;

/// One tool call as Claude Code describes it to a PreToolUse hook, plus what the gate needs from the world.
pub struct GateInput<'a> {
    /// The tool: `Bash`, `Edit`, `Write`, `mcp__...`.
    pub tool_name: &'a str,
    /// The tool's input, whose shape depends on the tool; anything but an object counts as empty.
    pub tool_input: Value,
    /// The folder the command runs in, for relative paths in a `>` redirect.
    pub cwd: &'a str,
    /// Whether the human typed a consent token, or `None` when the transcript cannot be read, which
    /// refuses every token.
    pub transcript_lines_for_consent: Option<ConsentCheck<'a>>,
    /// Whether a path is an existing regular file, for the rules about overwriting one.
    pub is_file: &'a dyn Fn(&Path) -> bool,
}

/// What the gate decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The call goes on to the normal permission flow.
    Allow {
        /// The consent tokens spent on it, when any; each is announced and logged.
        consent_used: Vec<String>,
    },
    /// Reversible fan-out: the call becomes a permission prompt carrying `reason`.
    Ask {
        /// Which ask rule fired, for the log; no token clears an ask.
        slug: &'static str,
        /// The prompt's text.
        reason: String,
    },
    /// Blocked outright, until the human types the consent token.
    Deny {
        /// The category, which is the consent slug.
        slug: &'static str,
        /// The refusal, as the session reads it.
        reason: String,
        /// The command the human would type to approve it, or `None` when no token clears this one.
        consent_command: Option<String>,
    },
}

impl Verdict {
    /// The slug this verdict names, or `None` for a plain allow.
    pub fn slug(&self) -> Option<&str> {
        match self {
            Verdict::Allow { .. } => None,
            Verdict::Ask { slug, .. } | Verdict::Deny { slug, .. } => Some(slug),
        }
    }
}

/// The gate. It holds nothing yet; the rules are the module's statics, and a project's own patterns are
/// where they will go when the profile grows them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Gate;

impl Gate {
    /// The verdict on one tool call.
    pub fn evaluate(&self, input: &GateInput<'_>) -> Verdict {
        let tool_input = match &input.tool_input {
            Value::Object(map) => Value::Object(map.clone()),
            _ => json!({}),
        };
        if input.tool_name != "Bash" {
            return check_tool(input.tool_name, &tool_input, input.is_file);
        }
        let command = tool_input.get("command").map(json_text).unwrap_or_default();
        if command.trim().is_empty() {
            return Verdict::Allow {
                consent_used: Vec::new(),
            };
        }
        let mut scan = Scan {
            input,
            command: &command,
            granted: CONSENT_RE
                .captures_iter(&command)
                .filter_map(|c| c.get(1).map(|m| m.as_str().to_owned()))
                .collect(),
            used: Vec::new(),
            context: "",
        };
        match scan.check_bash() {
            Err(verdict) => verdict,
            Ok(()) => Verdict::Allow {
                consent_used: scan.used,
            },
        }
    }
}

/// The JSON `budd hook` prints for `verdict`, or `None` when there is nothing to say (an allow without a
/// token). The shape is Claude Code's: `hookSpecificOutput` with the decision, or a `systemMessage` when a
/// token was spent, naming `consent_log` where the use was recorded.
pub fn hook_output(verdict: &Verdict, consent_log: &Path) -> Option<Value> {
    match verdict {
        Verdict::Allow { consent_used } if consent_used.is_empty() => None,
        Verdict::Allow { consent_used } => {
            let spent: BTreeSet<&str> = consent_used.iter().map(String::as_str).collect();
            let spent: Vec<&str> = spent.into_iter().collect();
            Some(json!({
                "systemMessage": format!(
                    "Guardrail consent used: {}. Logged to {}.",
                    spent.join(", "),
                    consent_log.display()
                )
            }))
        }
        Verdict::Ask { reason, .. } => Some(decision("ask", reason)),
        Verdict::Deny { reason, .. } => Some(decision("deny", reason)),
    }
}

/// The JSON `budd hook` prints when the gate could not run (`detail` says why): it fails closed.
pub fn fail_closed_output(detail: &str) -> Value {
    decision("deny", &format!("destructive-gate: {detail}, failing closed."))
}

/// Claude Code's PreToolUse decision object.
fn decision(permission: &str, reason: &str) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": permission,
            "permissionDecisionReason": reason,
        }
    })
}

/// A JSON value as the Python `str()` of it: a string as is, anything else as its JSON text.
fn json_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

// --------------------------------------------------------------------------
// patterns
// --------------------------------------------------------------------------

/// A pattern compiled once. A pattern that does not compile is a programming error in this file, which
/// the tests catch; at run time the panic reaches the hook's backstop, which refuses the call.
fn plain(pattern: &'static str) -> Regex {
    match Regex::new(pattern) {
        Ok(regex) => regex,
        Err(error) => panic!("gate pattern {pattern:?} does not compile: {error}"),
    }
}

/// As [`plain`], for the patterns that need look-around.
fn fancy(pattern: &'static str) -> FancyRegex {
    match FancyRegex::new(pattern) {
        Ok(regex) => regex,
        Err(error) => panic!("gate pattern {pattern:?} does not compile: {error}"),
    }
}

/// Whether a look-around pattern matches. A matching error (a backtracking limit) counts as a match, so
/// an error inside the gate leans towards refusing.
fn fancy_matches(regex: &FancyRegex, text: &str) -> bool {
    regex.is_match(text).unwrap_or(true)
}

macro_rules! patterns {
    ($($name:ident = $pattern:expr;)*) => {
        $(static $name: LazyLock<Regex> = LazyLock::new(|| plain($pattern));)*
        /// Every plain pattern, for the test that compiles them all.
        #[cfg(test)]
        const PLAIN_PATTERNS: &[&str] = &[$($pattern),*];
    };
}

macro_rules! fancy_patterns {
    ($($name:ident = $pattern:expr;)*) => {
        $(static $name: LazyLock<FancyRegex> = LazyLock::new(|| fancy($pattern));)*
        /// Every look-around pattern, for the test that compiles them all.
        #[cfg(test)]
        const FANCY_PATTERNS: &[&str] = &[$($pattern),*];
    };
}

patterns! {
    CONSENT_RE = r"\bI_CONSENT=([A-Za-z0-9_-]+)";
    SEGMENT_SPLIT = r"(?:&&|\|\||[;\n|&])";
    ENV_ASSIGNMENT = r"^[A-Za-z_]\w*=";
    MAIL_MAILER = r#"\bMAIL_MAILER\s*=\s*["']?([A-Za-z_]+)"#;
    ARTISAN_MAIL = r"(Mail::|Notification::|->notify\(|->sendNow\(|Notifiable|mail:send|notifications:send)";
    SQL_WRITE = r#"(?i)\b(DROP\s+(TABLE|DATABASE|SCHEMA|INDEX|VIEW)|TRUNCATE\b|DELETE\s+FROM\b|INSERT\s+(IGNORE\s+)?INTO\b|REPLACE\s+INTO\b|UPDATE\s+[`"\w.]+[\s\S]{0,120}?\bSET\b|ALTER\s+TABLE\b|GRANT\b|REVOKE\b)"#;
    DB_DRIVER = r"(?i)(mysqli|\bPDO\b|->query\(|->exec\(|->prepare\(|->real_query\(|pymysql|psycopg|mysql\.connector)";
    DB_CLIENT_FLAGS = r"\b(mysql|psql|mariadb)\s+-";
    SQL_FILE_IN = r"<\s*\S+\.sql\b";
    ARTISAN_DB_RESET = r"artisan\s+(migrate:(fresh|refresh|reset|rollback)|db:wipe|db:seed)";
    REDIS_FLUSH = r"(?i)\b(FLUSHALL|FLUSHDB)\b";
    FIND_DELETE = r"-delete\b|-exec\s+rm\b";
    GIT_RESET_HARD = r"\breset\s+(--\S+\s+)*--hard\b";
    GIT_CLEAN = r"\bclean\b.*-\w*[fdx]";
    GIT_DISCARD_ALL = r"\b(checkout|restore)\b\s+(--\s+)?[.*]";
    GIT_BRANCH_FORCE = r"\bbranch\s+(-D|--delete\s+--force|-d\s+-f)";
    GIT_WORKTREE_REMOVE = r"\bworktree\s+remove\b";
    GIT_FILTER = r"\b(filter-branch|filter-repo)\b";
    GIT_RM = r"\bgit\s+(-\S+\s+)*rm\b";
    RECURSIVE_FLAG = r"\s-\w*r\b";
    GLOB_CHARS = r"[*?\[]";
    GIT_PUSH = r"\bpush\b";
    PUSH_DELETE = r"--delete\b|\s:\S";
    GH_VERB = r#"(?i)(?:-X|--method)\s*['"]?(DELETE|PUT|PATCH)\b"#;
    GH_API = r"\bgh\s+api\b";
    GH_DELETE = r"\bgh\s+(repo|release|ssh-key|gist|secret|variable|label|cache|workflow|extension)\s+delete\b";
    HTTP_VERB = r#"(?i)-X\s*['"]?(DELETE|PUT|PATCH)\b"#;
    HOSTED_GIT = r"(api\.github\.com|gitlab\.com/api|api\.bitbucket)";
    LOOPBACK = r"https?://(localhost|127\.0\.0\.1|0\.0\.0\.0|\[?::1\]?)";
    HTTP_URL = r"https?://";
    REMOTE_SPEC = r"^(rsync://|[A-Za-z0-9._%+-]+(@[A-Za-z0-9._-]+)?:)";
    RSYNC_DELETE = r"--delete(-after|-before|-during|-delay|-excluded)?\b";
    COMPOSE_DOWN_VOLUMES = r"\bdown\b.*(-v\b|--volumes\b)";
    VOLUME_RM = r"\bvolume\s+(rm|prune)\b";
    SYSTEM_PRUNE = r"\bsystem\s+prune\b";
    WORD_REMOVE = r"\bremove\b";
    STRIPE_CLI_WRITE = r"\bstripe\s+(trigger|post|delete|[a-z_]+\s+(create|update|delete|cancel|capture|confirm|refund|pay|void|finalize_invoice|send_invoice|attach|detach))\b";
    STRIPE_CODE_WRITE = r"(Stripe\\|StripeClient|Cashier|->stripe\(|->charge\(|->refund\(|->newSubscription\(|->invoiceFor\()";
    STRIPE_VERB = r#"(?i)(?:-X|--request)\s*['"]?(POST|DELETE|PUT|PATCH)\b"#;
    PHP_INLINE = r"\bphp\s+-r\b";
    QUEUE_WORKER = r"artisan\s+(queue:work|queue:listen|horizon)\b";
    QUEUE_SCOPED = r"--queue[=\s]\S+";
    SED_IN_PLACE = r"\bsed\s+(-\S*\s+)*-i\b|\bsed\s+-\S*i";
    PERL_IN_PLACE = r"\bperl\s+-\S*p?i";
    WORD_FIND = r"\bfind\b";
    SUBSTITUTION = r"\$\(";
    MAIL_TOOL = r"(?i)(gmail|mail|smtp|sendgrid|resend|postmark|outlook|exchange)";
    MAIL_VERB = r"(?i)(send|draft|reply|forward|compose|deliver)";
    MCP_MUTATE = r"(?i)(editJiraIssue|transitionJiraIssue|updateConfluencePage|update_canvas|update_draft|update_label|delete_label|unlabel_(message|thread)|Google_Drive__copy_file)";
}

fancy_patterns! {
    FORCE_PUSH = r"(--force\b|--force-with-lease\b|--mirror\b|(?<!\w)-f(?!\w))";
    CURL_DATA = r"(?<!\S)(-d|--data\S*|-F|--form)(?!\S)";
    CURL_GET = r"(?<!\S)(-G|--get)(?!\S)";
    OVERWRITE = r#"(?<![0-9<>&])>(?!>)\s*([^\s;|&<>()"']+)"#;
}

// --------------------------------------------------------------------------
// command parsing
// --------------------------------------------------------------------------

/// `text` split into words as Python's `shlex.split` does it (POSIX mode): whitespace separates, single
/// quotes are literal, double quotes let a backslash escape only `"` and `\`, a bare backslash escapes
/// the next character. `None` when a quote or a trailing backslash is left open.
pub fn shlex_split(text: &str) -> Option<Vec<String>> {
    enum State {
        Space,
        Word,
        Single,
        Double,
        Escape,
        DoubleEscape,
    }
    let mut out = Vec::new();
    let mut token = String::new();
    let mut quoted = false;
    let mut state = State::Space;
    let is_space = |c: char| matches!(c, ' ' | '\t' | '\r' | '\n');
    for c in text.chars() {
        state = match state {
            State::Space | State::Word => {
                if is_space(c) {
                    if !token.is_empty() || quoted {
                        out.push(std::mem::take(&mut token));
                        quoted = false;
                    }
                    State::Space
                } else if c == '\\' {
                    State::Escape
                } else if c == '\'' {
                    quoted = true;
                    State::Single
                } else if c == '"' {
                    quoted = true;
                    State::Double
                } else {
                    token.push(c);
                    State::Word
                }
            }
            State::Single => {
                if c == '\'' {
                    State::Word
                } else {
                    token.push(c);
                    State::Single
                }
            }
            State::Double => {
                if c == '"' {
                    State::Word
                } else if c == '\\' {
                    State::DoubleEscape
                } else {
                    token.push(c);
                    State::Double
                }
            }
            State::Escape => {
                token.push(c);
                State::Word
            }
            State::DoubleEscape => {
                if c != '\\' && c != '"' {
                    token.push('\\');
                }
                token.push(c);
                State::Double
            }
        };
    }
    match state {
        State::Space | State::Word => {
            if !token.is_empty() || quoted {
                out.push(token);
            }
            Some(out)
        }
        State::Single | State::Double | State::Escape | State::DoubleEscape => None,
    }
}

/// The shell line as token lists, one per pipeline or list element. The split on `&&`, `||`, `;`, `|`,
/// `&` and newlines happens before the words are read, so an operator inside quotes also splits, and a
/// part whose quotes are then left open falls back to a split on whitespace.
pub fn segments(command: &str) -> Vec<Vec<String>> {
    SEGMENT_SPLIT
        .split(command)
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .filter_map(|part| {
            let tokens = shlex_split(part).unwrap_or_else(|| part.split_whitespace().map(str::to_owned).collect());
            (!tokens.is_empty()).then_some(tokens)
        })
        .collect()
}

/// `tokens` without the leading `NAME=value` assignments.
fn strip_env(tokens: &[String]) -> &[String] {
    let skip = tokens.iter().take_while(|t| ENV_ASSIGNMENT.is_match(t)).count();
    &tokens[skip..]
}

/// The last component of a path, as Python's `Path(p).name`: `./awwt` is `awwt`, `/usr/bin/sudo` is `sudo`.
fn basename(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
    if name == "." { "" } else { name }
}

/// `tokens` with the assignments and the transparent wrappers stripped, so a rule reads the real command
/// and its real arguments: `... | xargs rm -rf x` is `rm -rf x`, not a command named xargs.
fn invocation(tokens: &[String]) -> &[String] {
    let mut rest = strip_env(tokens);
    while let Some(head) = rest.first()
        && WRAPPERS.contains(&basename(head))
    {
        rest = strip_env(&rest[1..]);
        while let Some(flag) = rest.first()
            && flag.starts_with('-')
        {
            rest = &rest[1..];
        }
    }
    rest
}

/// Whether the segment invokes one of `names`, seeing through the wrappers.
fn runs(tokens: &[String], names: &[&str]) -> bool {
    invocation(tokens)
        .first()
        .is_some_and(|head| names.contains(&basename(head)))
}

/// The tokens joined by single spaces, which is what the line patterns match against.
fn flat(tokens: &[String]) -> String {
    tokens.join(" ")
}

/// The remote command strings carried by the `ssh` segments of `command`. Without this the gate is blind
/// to every destructive action aimed at a server: the remote command is one quoted argument, so the
/// segment head is `ssh` and every rule declines.
pub fn remote_commands(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tokens in segments(command) {
        let mut rest = strip_env(&tokens);
        while let Some(head) = rest.first()
            && TRANSPARENT_WRAPPERS.contains(&basename(head))
        {
            rest = strip_env(&rest[1..]);
        }
        let Some(head) = rest.first() else { continue };
        if basename(head) != "ssh" {
            continue;
        }
        let args = &rest[1..];
        let mut i = 0;
        while i < args.len() {
            if SSH_VALUE_FLAGS.contains(&args[i].as_str()) {
                i += 2;
            } else if args[i].starts_with('-') {
                i += 1;
            } else {
                break;
            }
        }
        i += 1; // step over the [user@]host token itself
        if i < args.len() {
            let payload = args[i..].join(" ");
            let payload = payload.trim();
            if !payload.is_empty() {
                out.push(payload.to_owned());
            }
        }
    }
    out
}

/// Whether a path sits in a build or scratch area where deletion is routine. The path is normalised as
/// Python's `Path` does it (repeated slashes and `.` components dropped) and anchored with a `/`.
pub fn is_safe_target(target: &str) -> bool {
    let parts: Vec<&str> = target.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    let normalized = format!("/{}", parts.join("/"));
    SAFE_RM_FRAGMENTS.iter().any(|frag| normalized.contains(frag))
}

/// Crude but sufficient: whether byte `idx` of `text` is outside any quoted run.
fn outside_quotes(text: &str, idx: usize) -> bool {
    let head = &text[..idx];
    head.matches('\'').count().is_multiple_of(2) && head.matches('"').count().is_multiple_of(2)
}

fn has_glob(target: &str) -> bool {
    target.contains(['*', '?', '['])
}

/// `command` without its consent tokens, for the command the human is asked to approve.
fn without_tokens(command: &str) -> String {
    let mut out = String::new();
    for word in command.split(' ') {
        if CONSENT_RE.is_match(word) && word.starts_with("I_CONSENT=") {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// The first word of the command the token `I_CONSENT=<slug>` prefixes: the segment carrying the token,
/// else the first segment, after its assignments. Empty when there is none.
fn first_word_after(command: &str, token: &str) -> String {
    let segs = segments(command);
    let carrying = segs
        .iter()
        .find(|tokens| tokens.iter().any(|t| t == token))
        .or_else(|| segs.first());
    carrying
        .and_then(|tokens| strip_env(tokens).first().cloned())
        .unwrap_or_default()
}

// --------------------------------------------------------------------------
// the scan
// --------------------------------------------------------------------------

/// One pass of the rules over a Bash command, carrying the consent state. A rule that fires returns
/// `Err(verdict)`, which `?` carries out of the scan at once, as the Python `sys.exit` did; a deny the
/// human consented to is recorded in `used` and the scan goes on.
struct Scan<'a> {
    input: &'a GateInput<'a>,
    command: &'a str,
    /// The slugs of the tokens on the command.
    granted: Vec<String>,
    /// The tokens spent, as typed.
    used: Vec<String>,
    /// "(over ssh) " while scanning a remote payload, else empty.
    context: &'static str,
}

impl Scan<'_> {
    /// Blocks, unless the human typed a consent token for exactly this slug.
    fn deny(&mut self, slug: &'static str, what: &str, why: &str) -> Result<(), Verdict> {
        let tokens: Vec<String> = self
            .granted
            .iter()
            .filter(|granted| granted.as_str() == slug || (slug == PAYMENT_SLUG && granted.as_str() == STRIPE_SLUG))
            .cloned()
            .collect();
        let consent_command = Some(format!("I_CONSENT={slug} {}", without_tokens(self.command)));
        if tokens.is_empty() {
            return Err(Verdict::Deny {
                slug,
                reason: self.deny_reason(slug, what, why),
                consent_command,
            });
        }
        for granted in &tokens {
            let token = format!("I_CONSENT={granted}");
            let first_word = first_word_after(self.command, &token);
            let typed = self
                .input
                .transcript_lines_for_consent
                .is_some_and(|human_typed| human_typed(&token, &first_word));
            if typed {
                self.used.push(granted.clone());
                return Ok(());
            }
        }
        Err(Verdict::Deny {
            slug,
            reason: self.deny_reason(
                slug,
                what,
                &format!(
                    "The command carries I_CONSENT={}, but the {NOT_TYPED}: no message the human typed in this \
                     session holds that token together with the command. The agent must never write the token \
                     itself.",
                    tokens[0]
                ),
            ),
            consent_command,
        })
    }

    /// The refusal text, as the session reads it.
    fn deny_reason(&self, slug: &str, what: &str, why: &str) -> String {
        format!(
            "BLOCKED [{slug}]: {}{what}\n{why}\n\n\
             This is a hard gate. Do not retry, do not work around it, and do not add the consent token yourself.\n\
             State the plan and what it will affect, then ask the user to approve running it with the token prefix:\n    \
             I_CONSENT={slug} <the exact command>\n\
             Protocol: {GUARDRAIL_DOC}",
            self.context
        )
    }

    /// Routes the call to a permission prompt carrying the reason.
    fn ask(&self, slug: &'static str, what: &str, why: &str) -> Result<(), Verdict> {
        Err(Verdict::Ask {
            slug,
            reason: format!("Fan-out change: {}{what}\n{why}", self.context),
        })
    }

    /// The same rules run twice: once on what was typed, once on whatever is being handed to a remote
    /// shell. A destructive action should not get cheaper by being aimed at a server.
    fn check_bash(&mut self) -> Result<(), Verdict> {
        let command = self.command.to_owned();
        let remotes = remote_commands(&command);
        let scopes = std::iter::once(("", command.as_str())).chain(remotes.iter().map(|r| ("(over ssh) ", r.as_str())));
        for (scope, text) in scopes {
            self.context = scope;
            let segs = segments(text);
            self.check_mail(text, &segs)?;
            self.check_db(text, &segs)?;
            self.check_filesystem(&segs)?;
            self.check_git(&segs)?;
            self.check_remote_api(&segs)?;
            self.check_remote_sync(&segs)?;
            self.check_containers(&segs)?;
            self.check_payments(text, &segs)?;
            self.check_queue(&segs)?;
            self.check_mass_edit(text, &segs)?;
        }
        self.context = "";
        // Local only: a remote path cannot be stat'd from here.
        self.check_overwrite(&command)
    }

    fn check_mail(&mut self, command: &str, segs: &[Vec<String>]) -> Result<(), Verdict> {
        let low = command.to_lowercase();
        if let Some(captures) = MAIL_MAILER.captures(command)
            && let Some(mailer) = captures.get(1)
            && !matches!(mailer.as_str().to_lowercase().as_str(), "log" | "array" | "null")
        {
            self.deny(
                "mail",
                &format!(
                    "sets MAIL_MAILER={}, a transport that delivers real mail.",
                    mailer.as_str()
                ),
                "Local and test runs must stay on MAIL_MAILER=log. The local database holds real customer \
                 addresses, so a live transport can mail real people.",
            )?;
        }
        for host in MAIL_HOSTS {
            if low.contains(host) {
                self.deny(
                    "mail",
                    &format!("calls the mail provider {host}."),
                    "That sends real mail.",
                )?;
            }
        }
        for tokens in segs {
            let line = flat(tokens);
            if !line.contains("artisan") && !line.contains("tinker") {
                continue;
            }
            if ARTISAN_MAIL.is_match(&line) {
                self.deny(
                    "mail",
                    "runs an artisan/tinker path that dispatches mail or notifications.",
                    "Against a database holding real addresses this delivers to real people. Use a throwaway \
                     account and MAIL_MAILER=log instead.",
                )?;
            }
        }
        Ok(())
    }

    fn check_db(&mut self, command: &str, segs: &[Vec<String>]) -> Result<(), Verdict> {
        for tokens in segs {
            let line = flat(tokens);
            // A write does not need a CLI client. An inline php/python one-liner driving mysqli, PDO, or
            // psycopg reaches the same rows.
            let talks_to_db = runs(tokens, &["mysql", "mysqldump", "psql", "sqlite3", "mariadb"])
                || DB_CLIENT_FLAGS.is_match(&line)
                || DB_DRIVER.is_match(&line);
            if talks_to_db {
                if let Some(hit) = SQL_WRITE.find(command) {
                    self.deny(
                        "db-write",
                        &format!("runs `{}` against a live database.", hit.as_str().trim()),
                        "The local database holds real customer data and there is no snapshot to roll back to. \
                         Read the rows first, show the exact affected count, and get the change agreed before \
                         writing.",
                    )?;
                }
                if SQL_FILE_IN.is_match(&line) {
                    self.deny(
                        "db-write",
                        "feeds a .sql file into a database client.",
                        "The statements live in the file, not on this command line, so what they touch cannot \
                         be reviewed before they run. Print the file and agree the change first.",
                    )?;
                }
            }
            if runs(tokens, &["dropdb"]) {
                self.deny("db-reset", "drops a database.", "Irreversible.")?;
            }
            if ARTISAN_DB_RESET.is_match(&line) {
                self.deny(
                    "db-reset",
                    "runs a destructive artisan database command.",
                    "migrate:fresh/refresh/reset, db:wipe, and db:seed all discard or overwrite existing rows. \
                     There is no snapshot to restore from.",
                )?;
            }
            if runs(tokens, &["redis-cli"]) && REDIS_FLUSH.is_match(&line) {
                self.deny("db-reset", "flushes Redis.", "Drops every session and queued job.")?;
            }
        }
        Ok(())
    }

    fn check_filesystem(&mut self, segs: &[Vec<String>]) -> Result<(), Verdict> {
        for tokens in segs {
            let rest = invocation(tokens);
            let line = flat(tokens);
            let arguments = rest.get(1..).unwrap_or(&[]);

            if runs(tokens, &["rm"]) {
                let recursive = arguments
                    .iter()
                    .filter(|t| t.starts_with('-'))
                    .any(|f| f.to_lowercase().trim_start_matches('-').contains('r') || f == "--recursive");
                let targets: Vec<&String> = arguments.iter().filter(|t| !t.starts_with('-')).collect();
                let unsafe_targets: Vec<&str> = targets
                    .iter()
                    .filter(|t| !is_safe_target(t))
                    .map(|t| t.as_str())
                    .collect();
                // No target on the line at all: the list arrives on stdin (xargs).
                if targets.is_empty() {
                    self.deny(
                        "bulk-rm",
                        "deletes a file list supplied on stdin rather than named here.",
                        "Nothing in the command says what gets deleted, so the blast radius cannot be reviewed. \
                         Print the list first.",
                    )?;
                }
                if unsafe_targets.iter().any(|t| has_glob(t)) {
                    self.deny(
                        "bulk-rm",
                        &format!("deletes every path matching a glob: {}", unsafe_targets.join(", ")),
                        "A glob expands at run time, so the match set is not visible in the command. List the \
                         matches first, then delete them by name.",
                    )?;
                }
                if recursive && !unsafe_targets.is_empty() {
                    let all: Vec<&str> = targets.iter().map(|t| t.as_str()).collect();
                    self.deny(
                        "bulk-rm",
                        &format!("recursively deletes: {}", all.join(", ")),
                        "Recursive deletion outside build/scratch directories is not recoverable. List what \
                         would go first, then get it agreed.",
                    )?;
                }
            }

            if runs(tokens, &["mv"]) {
                let targets: Vec<&String> = arguments.iter().filter(|t| !t.starts_with('-')).collect();
                let sources = if targets.len() > 1 {
                    &targets[..targets.len() - 1]
                } else {
                    &targets[..]
                };
                if targets.is_empty() || sources.iter().any(|t| has_glob(t)) {
                    self.ask(
                        "mv-glob",
                        "a move over an unenumerated set of paths.",
                        "The source list expands at run time or arrives on stdin, so neither what moves nor what \
                         it overwrites is visible here. A two-argument rename is fine, this is not.",
                    )?;
                }
            }

            if runs(tokens, &["find"]) && FIND_DELETE.is_match(&line) {
                self.deny(
                    "bulk-rm",
                    "deletes every file matching a find expression.",
                    "The match set is unknown until it runs. Print the matches first.",
                )?;
            }

            if runs(tokens, &["shred", "srm"]) {
                self.deny("bulk-rm", "irrecoverably shreds files.", "No undo exists.")?;
            }
        }
        Ok(())
    }

    fn check_git(&mut self, segs: &[Vec<String>]) -> Result<(), Verdict> {
        for tokens in segs {
            if !runs(tokens, &["git"]) {
                continue;
            }
            let line = flat(strip_env(tokens));

            if GIT_RESET_HARD.is_match(&line) {
                self.deny(
                    "git-destroy",
                    "runs `git reset --hard`, discarding all uncommitted work.",
                    "Anything not committed is gone. Show `git status` and stash or commit first.",
                )?;
            }
            if GIT_CLEAN.is_match(&line) {
                self.deny(
                    "git-destroy",
                    "runs `git clean`, deleting untracked files.",
                    "Untracked files include local env files and scratch work that is not in any commit.",
                )?;
            }
            if GIT_DISCARD_ALL.is_match(&line) {
                self.deny(
                    "git-destroy",
                    "discards all local modifications in the working tree.",
                    "Same loss as reset --hard. Diff first.",
                )?;
            }
            if GIT_BRANCH_FORCE.is_match(&line) {
                self.deny("git-destroy", "force-deletes a branch.", "Unmerged commits are lost.")?;
            }
            // Without --force git refuses a dirty worktree, but it still deletes the gitignored files
            // (local settings, evidence, logs) without a word.
            if GIT_WORKTREE_REMOVE.is_match(&line) {
                self.deny(
                    "worktree-remove",
                    "runs `git worktree remove`, deleting the worktree directory.",
                    "Gitignored files in it (local settings, evidence, logs) go with it, and --force also drops \
                     uncommitted work. Show `git status --ignored` first.",
                )?;
            }
            if GIT_FILTER.is_match(&line) {
                self.deny("git-destroy", "rewrites repository history.", "Irreversible remotely.")?;
            }
            if GIT_RM.is_match(&line) && (RECURSIVE_FLAG.is_match(&line) || GLOB_CHARS.is_match(&line)) {
                self.deny(
                    "bulk-rm",
                    "runs `git rm` recursively or over a glob pathspec.",
                    "The pathspec expands at run time and the files leave the working tree as well as the \
                     index. Name the files individually.",
                )?;
            }
            if GIT_PUSH.is_match(&line) {
                if fancy_matches(&FORCE_PUSH, &line) {
                    self.deny(
                        "force-push",
                        "force-pushes, overwriting remote history.",
                        "Other people's commits on that branch are destroyed and CI history is rewritten.",
                    )?;
                }
                if PUSH_DELETE.is_match(&line) {
                    self.deny(
                        "remote-mutate",
                        "deletes a remote branch.",
                        "Deleting a remote branch can close its PR and orphan review history.",
                    )?;
                }
            }
        }
        Ok(())
    }

    fn check_remote_api(&mut self, segs: &[Vec<String>]) -> Result<(), Verdict> {
        for tokens in segs {
            // `gh` is the client actually used on these projects, so it needs the same treatment curl
            // gets. Additive calls (POST, or no method) still pass.
            if runs(tokens, &["gh"]) {
                let line = flat(strip_env(tokens));
                let verb = GH_VERB
                    .captures(&line)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().to_uppercase());
                if GH_API.is_match(&line)
                    && let Some(verb) = verb
                {
                    self.deny(
                        "remote-mutate",
                        &format!("sends {verb} through `gh api`, mutating or deleting a remote object."),
                        "This edits or removes issues, PRs, comments, branches, or releases for everyone. Name \
                         the exact object and get it agreed first.",
                    )?;
                }
                if GH_DELETE.is_match(&line) {
                    self.deny(
                        "remote-mutate",
                        "deletes a hosted-git object through `gh`.",
                        "Removal is immediate and shared. Name the exact object first.",
                    )?;
                }
                continue;
            }

            if !runs(tokens, &["curl", "wget", "http", "httpie"]) {
                continue;
            }
            let line = flat(tokens);
            let Some(verb) = HTTP_VERB
                .captures(&line)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_uppercase())
            else {
                continue;
            };
            if HOSTED_GIT.is_match(&line) {
                self.deny(
                    "remote-mutate",
                    &format!("sends {verb} to a hosted-git API, mutating or deleting a remote object."),
                    "This edits or removes issues, PRs, comments, branches, or releases for everyone. Name the \
                     exact object and get it agreed first.",
                )?;
            }
            // Any other service: a DELETE/PUT/PATCH over http is a write on someone else's box. A loopback
            // dev server is exempt, a named host is not, and that includes *.local vhosts, which front the
            // shared dev database.
            if LOOPBACK.is_match(&line) {
                continue;
            }
            // The payment provider has its own slug, so one payment-write token clears a Stripe DELETE
            // instead of also needing remote-write.
            if line.contains("api.stripe.com") {
                continue;
            }
            if HTTP_URL.is_match(&line) {
                self.deny(
                    "remote-write",
                    &format!("sends {verb} to a remote HTTP service."),
                    "This mutates or removes a record on a server, and the previous state cannot be restored \
                     from here. Read the record first, state exactly what changes, and get it agreed.",
                )?;
            }
        }
        Ok(())
    }

    /// rsync that deletes on a server. Plain additive rsync still passes.
    fn check_remote_sync(&mut self, segs: &[Vec<String>]) -> Result<(), Verdict> {
        for tokens in segs {
            if !runs(tokens, &["rsync"]) {
                continue;
            }
            let line = flat(tokens);
            if !RSYNC_DELETE.is_match(&line) {
                continue;
            }
            let rest = invocation(tokens);
            let dest = rest
                .get(1..)
                .unwrap_or(&[])
                .iter()
                .rfind(|t| !t.starts_with('-'))
                .map(String::as_str)
                .unwrap_or("");
            if dest.starts_with(['/', '.', '~']) || !REMOTE_SPEC.is_match(dest) {
                continue;
            }
            self.deny(
                "remote-write",
                &format!("mirrors onto {dest} with --delete, removing server files absent from the source."),
                "Anything on the server that is not in the local tree is deleted, including files put there by \
                 someone else or by a previous release. List what differs first, then get the deploy agreed.",
            )?;
        }
        Ok(())
    }

    fn check_containers(&mut self, segs: &[Vec<String>]) -> Result<(), Verdict> {
        for tokens in segs {
            let line = flat(tokens);
            if runs(tokens, &["docker", "podman"]) || line.contains("docker compose") {
                if COMPOSE_DOWN_VOLUMES.is_match(&line) {
                    self.deny(
                        "container-wipe",
                        "tears down the stack and deletes its volumes.",
                        "That destroys the database, including any data not in a seeder. Plain `down` keeps \
                         volumes.",
                    )?;
                }
                if VOLUME_RM.is_match(&line) {
                    self.deny("container-wipe", "removes Docker volumes.", "Data is not recoverable.")?;
                }
                if SYSTEM_PRUNE.is_match(&line) {
                    self.deny(
                        "container-wipe",
                        "prunes the Docker system.",
                        "Reaches every project on this machine, not just this one.",
                    )?;
                }
            }
            if (runs(tokens, &["awwt"]) || line.contains("/awwt") || line.starts_with("./awwt"))
                && WORD_REMOVE.is_match(&line)
            {
                self.deny(
                    "worktree-remove",
                    "removes a worktree pair.",
                    "Uncommitted work in either the frontend or backend worktree is lost. Show `git status` for \
                     both first.",
                )?;
            }
        }
        Ok(())
    }

    /// Payment writes, test mode included, are outward-facing: a test-mode object still fires webhook
    /// events to every endpoint registered on the account, and on a shared sandbox account those endpoints
    /// are other people's environments.
    fn check_payments(&mut self, command: &str, segs: &[Vec<String>]) -> Result<(), Verdict> {
        if command.contains("sk_live_") || command.contains("rk_live_") {
            self.deny(
                PAYMENT_SLUG,
                "puts a live-mode Stripe key on the command line.",
                "A live key moves real money and bills real customers. Local work uses the sk_test_ key only.",
            )?;
        }
        let why = "Test mode still fires webhook events to every endpoint registered on the account, which on \
                   the shared sandbox includes other team environments. First list the endpoints (GET \
                   /v1/webhook_endpoints, or `stripe webhook_endpoints list`), tell the user who will receive \
                   the events, and get it agreed.";
        for tokens in segs {
            let line = flat(tokens);
            if runs(tokens, &["curl", "wget", "http", "httpie"]) && line.contains("api.stripe.com") {
                let verb = STRIPE_VERB.is_match(&line);
                // -G / --get turns --data into a query string, so that stays a read.
                let implicit_post = fancy_matches(&CURL_DATA, &line) && !CURL_GET.is_match(&line).unwrap_or(false);
                if verb || implicit_post {
                    self.deny(PAYMENT_SLUG, "writes to the Stripe API.", why)?;
                }
            }
            if runs(tokens, &["stripe"]) && STRIPE_CLI_WRITE.is_match(&line) {
                self.deny(PAYMENT_SLUG, "creates or changes Stripe objects through the CLI.", why)?;
            }
            if (line.contains("tinker") || PHP_INLINE.is_match(&line)) && STRIPE_CODE_WRITE.is_match(&line) {
                self.deny(PAYMENT_SLUG, "drives the Stripe SDK or Cashier from inline PHP.", why)?;
            }
        }
        Ok(())
    }

    /// A queue worker runs whatever is already queued, not only what we queued.
    fn check_queue(&self, segs: &[Vec<String>]) -> Result<(), Verdict> {
        for tokens in segs {
            let line = flat(tokens);
            if !QUEUE_WORKER.is_match(&line) || QUEUE_SCOPED.is_match(&line) {
                continue;
            }
            self.ask(
                "queue-worker",
                "starts a queue worker on the default queue.",
                "A worker runs every job already waiting, oldest first, including unrelated jobs other runs left \
                 in the shared `jobs` table, and those can call paid APIs or send mail. List the waiting jobs \
                 first, and scope the worker with --queue=<name> (plus --once) to run only yours.",
            )?;
        }
        Ok(())
    }

    fn check_mass_edit(&self, command: &str, segs: &[Vec<String>]) -> Result<(), Verdict> {
        for tokens in segs {
            let line = flat(tokens);
            if !SED_IN_PLACE.is_match(&line) && !PERL_IN_PLACE.is_match(&line) {
                continue;
            }
            let fans_out = line.contains('*')
                || command.contains("xargs")
                || WORD_FIND.is_match(command)
                || SUBSTITUTION.is_match(&line)
                || line.contains('`');
            if fans_out {
                self.ask(
                    "mass-edit",
                    "an in-place stream edit across an unenumerated set of files.",
                    "The file list is only known at run time, so the blast radius is not reviewable from the \
                     command. Prefer enumerating the files and editing them individually.",
                )?;
            }
        }
        Ok(())
    }

    /// A single `>` truncates before it writes. Only flagged for files that exist.
    fn check_overwrite(&self, command: &str) -> Result<(), Verdict> {
        for found in OVERWRITE.captures_iter(command) {
            let Ok(captures) = found else {
                // A matching error inside the gate: refuse rather than pass.
                return self.ask(
                    "overwrite",
                    "a shell redirect the gate could not read.",
                    "The command could not be scanned for a `>` onto an existing file. Check it by hand.",
                );
            };
            let (Some(whole), Some(target)) = (captures.get(0), captures.get(1)) else {
                continue;
            };
            if !outside_quotes(command, whole.start()) {
                continue;
            }
            let target = target.as_str();
            if target.starts_with('&') || target.starts_with("/dev/") || is_safe_target(target) {
                continue;
            }
            let path = Path::new(target);
            let path = if path.is_absolute() || target.starts_with('/') {
                path.to_path_buf()
            } else {
                Path::new(self.input.cwd).join(target)
            };
            if !(self.input.is_file)(&path) {
                continue;
            }
            self.ask(
                "overwrite",
                &format!("overwrites the existing file {target} by shell redirect."),
                "A single `>` truncates the file before anything is written, so the previous contents are gone \
                 with no diff to review. Use `>>` to append, or go through the Edit tool so the change is \
                 visible.",
            )?;
        }
        Ok(())
    }
}

// --------------------------------------------------------------------------
// non-Bash tools
// --------------------------------------------------------------------------

/// The rules for every tool but Bash. Mail is evaluated first and denies outright, so a tool that is both
/// a mail tool and a mutation (Gmail update_draft) never downgrades to a prompt.
fn check_tool(tool_name: &str, tool_input: &Value, is_file: &dyn Fn(&Path) -> bool) -> Verdict {
    let no_prefix = |what: &str, why: &str| format!("Fan-out change: {what}\n{why}");
    if tool_name.starts_with("mcp__") && MAIL_TOOL.is_match(tool_name) && MAIL_VERB.is_match(tool_name) {
        return Verdict::Deny {
            slug: "mail",
            reason: format!(
                "BLOCKED [mail]: `{tool_name}` sends real mail to real recipients.\n\
                 There is no consent token for this one, because an agent should not be the thing that presses \
                 send.\n\
                 Draft the message in the conversation and let the user send it.\n\
                 Protocol: {GUARDRAIL_DOC}"
            ),
            consent_command: None,
        };
    }
    if tool_name == "Write" {
        let target = tool_input.get("file_path").map(json_text).unwrap_or_default();
        if !target.is_empty() && !is_safe_target(&target) && is_file(Path::new(&target)) {
            return Verdict::Ask {
                slug: "write-replace",
                reason: no_prefix(
                    &format!("replaces the entire contents of {target}."),
                    "Write overwrites the whole file rather than editing part of it, so anything not carried \
                     over is lost without a diff. Confirm a full replacement is intended.",
                ),
            };
        }
    }
    if tool_name.starts_with("mcp__") && MCP_MUTATE.is_match(tool_name) {
        return Verdict::Ask {
            slug: "mcp-mutate",
            reason: no_prefix(
                &format!("`{tool_name}` edits or removes an object other people can see."),
                "Unlike posting a new comment, this changes or deletes something that already exists, and the \
                 previous state is not recoverable from here. Confirm the exact object.",
            ),
        };
    }
    if matches!(tool_name, "Edit" | "MultiEdit") && tool_input.get("replace_all") == Some(&Value::Bool(true)) {
        let file = tool_input
            .get("file_path")
            .map(json_text)
            .filter(|f| !f.is_empty())
            .unwrap_or_else(|| "a file".to_owned());
        return Verdict::Ask {
            slug: "replace-all",
            reason: no_prefix(
                &format!("replace_all across {file}."),
                "Every occurrence is rewritten, including ones neither of us has looked at. Confirm that is \
                 intended, or scope the edits individually.",
            ),
        };
    }
    Verdict::Allow {
        consent_used: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the Python suite expected of a case, plus the slug this port names.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Expect {
        Deny(&'static str),
        Ask(&'static str),
        Allow,
        /// An allow with a consent notice.
        Notice(&'static str),
    }
    use Expect::{Allow, Ask, Deny, Notice};

    const NO_FILE: &dyn Fn(&Path) -> bool = &|_| false;
    const TYPED: ConsentCheck<'static> = &|_, _| true;

    fn evaluate(
        tool: &str,
        tool_input: Value,
        consent: Option<ConsentCheck<'_>>,
        is_file: &dyn Fn(&Path) -> bool,
    ) -> Verdict {
        Gate.evaluate(&GateInput {
            tool_name: tool,
            tool_input,
            cwd: "/tmp",
            transcript_lines_for_consent: consent,
            is_file,
        })
    }

    fn bash(command: &str) -> Verdict {
        evaluate("Bash", json!({"command": command}), Some(TYPED), NO_FILE)
    }

    /// The Python suite's `classify`: what the hook's stdout says.
    fn classify(verdict: &Verdict) -> &'static str {
        let Some(output) = hook_output(verdict, Path::new("/home/state/consent.log")) else {
            return "allow";
        };
        match output["hookSpecificOutput"]["permissionDecision"].as_str() {
            Some("deny") => "deny",
            Some("ask") => "ask",
            Some(_) => "other",
            None if output.get("systemMessage").is_some() => "notice",
            None => "allow",
        }
    }

    /// The cases of ~/.claude/hooks/test_destructive_gate.py, in its order and with its labels, each with
    /// the slug this port names.
    fn python_cases() -> Vec<(&'static str, &'static str, Value, Expect)> {
        let b = |command: &str| json!({"command": command});
        vec![
            // --- mail ---
            (
                "mail: live mailer",
                "Bash",
                b("MAIL_MAILER=smtp php artisan queue:work"),
                Deny("mail"),
            ),
            (
                "mail: log mailer ok",
                "Bash",
                b("MAIL_MAILER=log php artisan queue:work --queue=bb1-test --once"),
                Allow,
            ),
            (
                "mail: sendgrid",
                "Bash",
                b("curl -X POST https://api.sendgrid.com/v3/mail/send -d @x"),
                Deny("mail"),
            ),
            (
                "mail: tinker Mail::",
                "Bash",
                b("php artisan tinker --execute='Mail::to($u)->send($m)'"),
                Deny("mail"),
            ),
            (
                "mail: artisan migrate ok",
                "Bash",
                b("MAIL_MAILER=log docker compose exec assigns-api php artisan migrate"),
                Allow,
            ),
            // --- queue workers ---
            (
                "queue: default queue:work asks",
                "Bash",
                b("docker compose exec assigns-api php artisan queue:work"),
                Ask("queue-worker"),
            ),
            (
                "queue: --once alone still asks",
                "Bash",
                b("docker exec assigns-api-B-S3 php artisan queue:work --once"),
                Ask("queue-worker"),
            ),
            (
                "queue: queue:listen asks",
                "Bash",
                b("php artisan queue:listen"),
                Ask("queue-worker"),
            ),
            (
                "queue: horizon asks",
                "Bash",
                b("php artisan horizon"),
                Ask("queue-worker"),
            ),
            (
                "queue: scoped --queue= ok",
                "Bash",
                b("docker exec assigns-api-B-S3 php artisan queue:work --queue=bs3-test --once"),
                Allow,
            ),
            (
                "queue: scoped --queue space ok",
                "Bash",
                b("php artisan queue:work --queue bs3-test --stop-when-empty"),
                Allow,
            ),
            (
                "queue: queue:failed read ok",
                "Bash",
                b("php artisan queue:failed"),
                Allow,
            ),
            (
                "queue: over ssh asks",
                "Bash",
                b(r#"ssh devserver "cd /app && php artisan queue:work""#),
                Ask("queue-worker"),
            ),
            // --- stripe ---
            (
                "stripe: curl POST",
                "Bash",
                b("curl -X POST https://api.stripe.com/v1/refunds -u sk_test_x: -d charge=ch_1"),
                Deny("payment-write"),
            ),
            (
                "stripe: curl implicit POST -d",
                "Bash",
                b("curl https://api.stripe.com/v1/customers -u sk_test_x: -d email=a@example.test"),
                Deny("payment-write"),
            ),
            (
                "stripe: curl DELETE",
                "Bash",
                b("curl -X DELETE https://api.stripe.com/v1/customers/cus_1 -u sk_test_x:"),
                Deny("payment-write"),
            ),
            (
                "stripe: curl GET ok",
                "Bash",
                b("curl -s https://api.stripe.com/v1/webhook_endpoints -u sk_test_x:"),
                Allow,
            ),
            (
                "stripe: curl GET -G -d ok",
                "Bash",
                b("curl -s -G https://api.stripe.com/v1/charges -u sk_test_x: --data-urlencode limit=3"),
                Allow,
            ),
            (
                "stripe: live key",
                "Bash",
                b("curl -s https://api.stripe.com/v1/balance -u sk_live_abc:"),
                Deny("payment-write"),
            ),
            (
                "stripe: cli trigger",
                "Bash",
                b("stripe trigger payment_intent.succeeded"),
                Deny("payment-write"),
            ),
            (
                "stripe: cli create",
                "Bash",
                b("stripe customers create --email a@example.test"),
                Deny("payment-write"),
            ),
            (
                "stripe: cli refund",
                "Bash",
                b("stripe refunds create --charge ch_1"),
                Deny("payment-write"),
            ),
            ("stripe: cli list ok", "Bash", b("stripe webhook_endpoints list"), Allow),
            (
                "stripe: cli listen ok",
                "Bash",
                b("stripe listen --forward-to localhost:10581/api/stripe/webhook"),
                Allow,
            ),
            (
                "stripe: tinker sdk",
                "Bash",
                b(
                    r#"docker exec assigns-api php artisan tinker --execute='\Stripe\Refund::create(["charge"=>"ch_1"]);'"#,
                ),
                Deny("payment-write"),
            ),
            (
                "stripe: tinker cashier",
                "Bash",
                b("php artisan tinker --execute='$u->charge(100, $pm);'"),
                Deny("payment-write"),
            ),
            (
                "stripe: tinker config read ok",
                "Bash",
                b(r#"php artisan tinker --execute='echo config("mail.default");'"#),
                Allow,
            ),
            (
                "stripe: consent clears",
                "Bash",
                b("I_CONSENT=stripe-write curl -X POST https://api.stripe.com/v1/refunds -u sk_test_x: -d charge=ch_1"),
                Notice("stripe-write"),
            ),
            (
                "stripe: consent clears DELETE",
                "Bash",
                b("I_CONSENT=stripe-write curl -X DELETE https://api.stripe.com/v1/customers/cus_1 -u sk_test_x:"),
                Notice("stripe-write"),
            ),
            (
                "stripe: wrong slug denies",
                "Bash",
                b("I_CONSENT=remote-write curl -X DELETE https://api.stripe.com/v1/customers/cus_1"),
                Deny("payment-write"),
            ),
            // --- db ---
            (
                "db: delete from",
                "Bash",
                b("mysql -h127.0.0.1 -P3307 -uroot -proot realstate -e 'DELETE FROM users WHERE id=5'"),
                Deny("db-write"),
            ),
            (
                "db: update set",
                "Bash",
                b("mysql -uroot realstate -e 'UPDATE users SET active=0'"),
                Deny("db-write"),
            ),
            (
                "db: select ok",
                "Bash",
                b("mysql -uroot realstate -e 'SELECT id FROM users LIMIT 5'"),
                Allow,
            ),
            (
                "db: migrate:fresh",
                "Bash",
                b("php artisan migrate:fresh --seed"),
                Deny("db-reset"),
            ),
            ("db: migrate ok", "Bash", b("php artisan migrate"), Allow),
            ("db: redis flushall", "Bash", b("redis-cli FLUSHALL"), Deny("db-reset")),
            ("db: dropdb", "Bash", b("dropdb realstate"), Deny("db-reset")),
            // --- filesystem ---
            (
                "fs: rm -rf home",
                "Bash",
                b("rm -rf /Users/devsgrw/personal-files/peyman"),
                Deny("bulk-rm"),
            ),
            (
                "fs: rm -rf node_modules ok",
                "Bash",
                b("rm -rf /repo/frontend/node_modules"),
                Allow,
            ),
            (
                "fs: rm -rf scratch ok",
                "Bash",
                b("rm -rf /private/tmp/claude-501/x/scratchpad/tmp"),
                Allow,
            ),
            (
                "fs: rm -rf mixed targets",
                "Bash",
                b("rm -rf /repo/node_modules /repo/src"),
                Deny("bulk-rm"),
            ),
            ("fs: rm single file ok", "Bash", b("rm /tmp/foo.txt"), Allow),
            (
                "fs: find -delete",
                "Bash",
                b("find . -name '*.log' -delete"),
                Deny("bulk-rm"),
            ),
            ("fs: find -print ok", "Bash", b("find . -name '*.log' -print"), Allow),
            // --- git ---
            (
                "git: reset --hard",
                "Bash",
                b("git reset --hard origin/testing"),
                Deny("git-destroy"),
            ),
            ("git: reset soft ok", "Bash", b("git reset --soft HEAD~1"), Allow),
            ("git: clean -fd", "Bash", b("git clean -fd"), Deny("git-destroy")),
            ("git: checkout dot", "Bash", b("git checkout -- ."), Deny("git-destroy")),
            ("git: checkout branch ok", "Bash", b("git checkout testing"), Allow),
            (
                "git: branch -D",
                "Bash",
                b("git branch -D a18-bugfix"),
                Deny("git-destroy"),
            ),
            (
                "git: push force",
                "Bash",
                b("git push --force origin a18-bugfix"),
                Deny("force-push"),
            ),
            (
                "git: push -f",
                "Bash",
                b("git push -f origin a18-bugfix"),
                Deny("force-push"),
            ),
            (
                "git: push lease",
                "Bash",
                b("git push --force-with-lease origin a18-bugfix"),
                Deny("force-push"),
            ),
            (
                "git: push delete",
                "Bash",
                b("git push origin --delete a18-bugfix"),
                Deny("remote-mutate"),
            ),
            ("git: push normal ok", "Bash", b("git push origin a18-bugfix"), Allow),
            ("git: status ok", "Bash", b("git status --porcelain"), Allow),
            // --- remote api ---
            (
                "api: DELETE github",
                "Bash",
                b("curl -X DELETE -H 'Authorization: Bearer $T' https://api.github.com/repos/o/r/git/refs/heads/x"),
                Deny("remote-mutate"),
            ),
            (
                "api: PATCH github",
                "Bash",
                b(r#"curl -X PATCH https://api.github.com/repos/o/r/pulls/12 -d '{"state":"closed"}'"#),
                Deny("remote-mutate"),
            ),
            (
                "api: POST comment ok",
                "Bash",
                b("curl -X POST https://api.github.com/repos/o/r/issues/12/comments -d @body.json"),
                Allow,
            ),
            (
                "api: GET ok",
                "Bash",
                b("curl -s https://api.github.com/repos/o/r/pulls"),
                Allow,
            ),
            // --- containers / worktrees ---
            (
                "docker: down -v",
                "Bash",
                b("docker compose down -v"),
                Deny("container-wipe"),
            ),
            ("docker: down ok", "Bash", b("docker compose down"), Allow),
            (
                "docker: volume rm",
                "Bash",
                b("docker volume rm assigns_db"),
                Deny("container-wipe"),
            ),
            (
                "docker: system prune",
                "Bash",
                b("docker system prune -af"),
                Deny("container-wipe"),
            ),
            ("docker: ps ok", "Bash", b("docker compose ps"), Allow),
            (
                "awwt: remove",
                "Bash",
                b("./awwt remove a18-bugfix"),
                Deny("worktree-remove"),
            ),
            ("awwt: list ok", "Bash", b("./awwt list"), Allow),
            (
                "git worktree: remove",
                "Bash",
                b("git worktree remove ../worktrees/gamecenter-x"),
                Deny("worktree-remove"),
            ),
            (
                "git worktree: remove via -C",
                "Bash",
                b("git -C ~/webserver/gamecenter worktree remove ~/webserver/worktrees/gamecenter-x"),
                Deny("worktree-remove"),
            ),
            (
                "git worktree: remove --force",
                "Bash",
                b("git worktree remove --force ../worktrees/gamecenter-x"),
                Deny("worktree-remove"),
            ),
            (
                "git worktree: consent clears",
                "Bash",
                b("I_CONSENT=worktree-remove git worktree remove ../worktrees/gamecenter-x"),
                Notice("worktree-remove"),
            ),
            (
                "git worktree: wrong slug denies",
                "Bash",
                b("I_CONSENT=git-destroy git worktree remove ../worktrees/gamecenter-x"),
                Deny("worktree-remove"),
            ),
            (
                "git worktree: add ok",
                "Bash",
                b("git worktree add ../worktrees/gamecenter-x -b feat/x origin/main"),
                Allow,
            ),
            ("git worktree: list ok", "Bash", b("git worktree list"), Allow),
            ("git worktree: prune ok", "Bash", b("git worktree prune"), Allow),
            // --- mass edit ---
            (
                "sed: glob in place",
                "Bash",
                b("sed -i '' 's/foo/bar/g' src/*.ts"),
                Ask("mass-edit"),
            ),
            (
                "sed: via xargs",
                "Bash",
                b("grep -rl foo . | xargs sed -i 's/foo/bar/'"),
                Ask("mass-edit"),
            ),
            (
                "sed: single file ok",
                "Bash",
                b("sed -i '' 's/foo/bar/' src/index.ts"),
                Allow,
            ),
            ("sed: no -i ok", "Bash", b("sed 's/foo/bar/' src/*.ts"), Allow),
            // --- consent tokens ---
            (
                "consent: right slug clears",
                "Bash",
                b("I_CONSENT=container-wipe docker compose down -v"),
                Notice("container-wipe"),
            ),
            (
                "consent: wrong slug denies",
                "Bash",
                b("I_CONSENT=bulk-rm docker compose down -v"),
                Deny("container-wipe"),
            ),
            (
                "consent: rm cleared",
                "Bash",
                b("I_CONSENT=bulk-rm rm -rf /Users/devsgrw/junk"),
                Notice("bulk-rm"),
            ),
            (
                "consent: force push cleared",
                "Bash",
                b("I_CONSENT=force-push git push --force origin x"),
                Notice("force-push"),
            ),
            // --- non-bash tools ---
            (
                "edit: replace_all",
                "Edit",
                json!({"file_path": "/r/a.ts", "old_string": "a", "new_string": "b", "replace_all": true}),
                Ask("replace-all"),
            ),
            (
                "edit: normal ok",
                "Edit",
                json!({"file_path": "/r/a.ts", "old_string": "a", "new_string": "b"}),
                Allow,
            ),
            (
                "mcp: gmail send",
                "mcp__claude_ai_Gmail__send_message",
                json!({"to": "x@y.com"}),
                Deny("mail"),
            ),
            (
                "mcp: gmail read ok",
                "mcp__claude_ai_Gmail__get_thread",
                json!({"id": "1"}),
                Allow,
            ),
            (
                "mcp: drive read ok",
                "mcp__claude_ai_Google_Drive__read_file_content",
                json!({"id": "1"}),
                Allow,
            ),
            // --- remote: the same rule must fire through ssh ---
            (
                "ssh: rm -rf remote",
                "Bash",
                b(r#"ssh gamecenter-staging "rm -rf /var/www/vhosts/releases""#),
                Deny("bulk-rm"),
            ),
            (
                "ssh: reset --hard remote",
                "Bash",
                b(r#"ssh gamecenter-staging "git reset --hard HEAD""#),
                Deny("git-destroy"),
            ),
            (
                "ssh: force push remote",
                "Bash",
                b(r#"ssh gamecenter-staging "git push --force origin develop""#),
                Deny("force-push"),
            ),
            (
                "ssh: volume prune remote",
                "Bash",
                b(r#"ssh devserver "docker volume prune -f""#),
                Deny("container-wipe"),
            ),
            (
                "ssh: with -p flag",
                "Bash",
                b(r#"ssh -p 2222 devserver "rm -rf /srv/data""#),
                Deny("bulk-rm"),
            ),
            (
                "ssh: read-only remote ok",
                "Bash",
                b(r#"ssh gamecenter-staging "ls -la /var/www""#),
                Allow,
            ),
            (
                "ssh: tail log ok",
                "Bash",
                b(r#"ssh gamecenter-staging "tail -50 /var/log/nginx/error.log""#),
                Allow,
            ),
            ("ssh: no command ok", "Bash", b("ssh gamecenter-staging"), Allow),
            // --- rsync ---
            (
                "rsync: --delete to remote",
                "Bash",
                b("rsync -az --delete ./dist/ gamecenter-staging:/var/www/current/"),
                Deny("remote-write"),
            ),
            (
                "rsync: no delete flag ok",
                "Bash",
                b("rsync -az ./dist/ gamecenter-staging:/var/www/current/"),
                Allow,
            ),
            (
                "rsync: local --delete ok",
                "Bash",
                b("rsync -az --delete ./dist/ /tmp/mirror/"),
                Allow,
            ),
            // --- delete forms that are not rm -r ---
            ("fs: rm glob", "Bash", b("rm /repo/version2/*.php"), Deny("bulk-rm")),
            (
                "fs: rm glob in node_modules ok",
                "Bash",
                b("rm /repo/node_modules/*.log"),
                Allow,
            ),
            (
                "fs: rm via xargs",
                "Bash",
                b("find . -name '*.vue' | xargs rm"),
                Deny("bulk-rm"),
            ),
            ("fs: git rm -r", "Bash", b("git rm -r src/"), Deny("bulk-rm")),
            ("fs: git rm single ok", "Bash", b("git rm src/App.vue"), Allow),
            (
                "fs: mv glob",
                "Bash",
                b("mv /repo/src/*.vue /repo/old/"),
                Ask("mv-glob"),
            ),
            ("fs: mv rename ok", "Bash", b("mv /repo/a.txt /repo/b.txt"), Allow),
            // --- gh ---
            (
                "gh: api DELETE",
                "Bash",
                b("gh api -X DELETE repos/DWAsia/x/git/refs/heads/feature"),
                Deny("remote-mutate"),
            ),
            (
                "gh: api --method PATCH",
                "Bash",
                b("gh api --method PATCH repos/DWAsia/x/pulls/1"),
                Deny("remote-mutate"),
            ),
            (
                "gh: repo delete",
                "Bash",
                b("gh repo delete DWAsia/x --yes"),
                Deny("remote-mutate"),
            ),
            ("gh: api GET ok", "Bash", b("gh api repos/DWAsia/x/pulls"), Allow),
            (
                "gh: pr create ok",
                "Bash",
                b("gh pr create --fill --base develop"),
                Allow,
            ),
            ("gh: pr view ok", "Bash", b("gh pr view 42 --json state"), Allow),
            // --- http writes beyond hosted git ---
            (
                "http: DELETE remote host",
                "Bash",
                b("curl -X DELETE https://manageapiv2-asia-dev.gnstg.net/license/5"),
                Deny("remote-write"),
            ),
            (
                "http: DELETE localhost ok",
                "Bash",
                b("curl -X DELETE http://localhost:8080/license/5"),
                Allow,
            ),
            (
                "http: GET remote ok",
                "Bash",
                b("curl -s https://manageapiv2-asia-dev.gnstg.net/license"),
                Allow,
            ),
            // --- sql parity ---
            (
                "db: insert into",
                "Bash",
                b("mysql -uroot pantherv2 -e 'INSERT INTO Outlet VALUES (1)'"),
                Deny("db-write"),
            ),
            (
                "db: update with join",
                "Bash",
                b("mysql -uroot pantherv2 -e 'UPDATE a JOIN b ON a.i=b.i SET a.x=1'"),
                Deny("db-write"),
            ),
            (
                "db: sql file piped in",
                "Bash",
                b("mysql -uroot pantherv2 < /tmp/wipe.sql"),
                Deny("db-write"),
            ),
            (
                "db: mysqldump read ok",
                "Bash",
                b("mysqldump -uroot pantherv2 Outlet > /tmp/out.sql"),
                Allow,
            ),
            (
                "db: show tables ok",
                "Bash",
                b("mysql -uroot pantherv2 -e 'SHOW TABLES'"),
                Allow,
            ),
            // --- mcp mutate vs additive ---
            (
                "mcp: jira edit",
                "mcp__claude_ai_Atlassian__editJiraIssue",
                json!({"issueIdOrKey": "CASBO-1"}),
                Ask("mcp-mutate"),
            ),
            (
                "mcp: jira transition",
                "mcp__claude_ai_Atlassian__transitionJiraIssue",
                json!({"issueIdOrKey": "CASBO-1"}),
                Ask("mcp-mutate"),
            ),
            (
                "mcp: jira comment ok",
                "mcp__claude_ai_Atlassian__addCommentToJiraIssue",
                json!({"issueIdOrKey": "CASBO-1"}),
                Allow,
            ),
            (
                "mcp: slack send ok",
                "mcp__claude_ai_Slack__slack_send_message",
                json!({"text": "hi"}),
                Allow,
            ),
            (
                "mcp: jira read ok",
                "mcp__claude_ai_Atlassian__getJiraIssue",
                json!({"issueIdOrKey": "CASBO-1"}),
                Allow,
            ),
            (
                "mcp: gmail update_draft still denies",
                "mcp__claude_ai_Gmail__update_draft",
                json!({"id": "1"}),
                Deny("mail"),
            ),
            // --- misc sanity ---
            ("misc: ls ok", "Bash", b("ls -la"), Allow),
            ("misc: npm test ok", "Bash", b("npm run test -- --run"), Allow),
            ("misc: pnpm build ok", "Bash", b("pnpm build"), Allow),
        ]
    }

    #[test]
    fn every_python_case_decides_the_same() {
        let cases = python_cases();
        assert_eq!(cases.len(), 129, "the Python suite has 129 cases");
        let mut failures = Vec::new();
        for (label, tool, tool_input, expected) in cases {
            let verdict = evaluate(tool, tool_input, Some(TYPED), NO_FILE);
            let got = classify(&verdict);
            let (want, slug) = match expected {
                Deny(slug) => ("deny", Some(slug)),
                Ask(slug) => ("ask", Some(slug)),
                Allow => ("allow", None),
                Notice(slug) => ("notice", Some(slug)),
            };
            let got_slug = match &verdict {
                Verdict::Allow { consent_used } => consent_used.first().map(String::as_str),
                other => other.slug(),
            };
            if got != want || got_slug != slug {
                failures.push(format!("{label}: expected {want} {slug:?}, got {got} {got_slug:?}"));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn every_pattern_compiles() {
        for pattern in PLAIN_PATTERNS {
            assert!(Regex::new(pattern).is_ok(), "{pattern}");
        }
        for pattern in FANCY_PATTERNS {
            assert!(FancyRegex::new(pattern).is_ok(), "{pattern}");
        }
    }

    #[test]
    fn the_refusal_reads_as_the_python_one_did() {
        let verdict = bash("rm -rf /Users/devsgrw/personal-files/peyman");
        let Verdict::Deny {
            slug,
            reason,
            consent_command,
        } = &verdict
        else {
            panic!("{verdict:?}");
        };
        assert_eq!(*slug, "bulk-rm");
        assert_eq!(
            reason,
            "BLOCKED [bulk-rm]: recursively deletes: /Users/devsgrw/personal-files/peyman\n\
             Recursive deletion outside build/scratch directories is not recoverable. List what would go first, then get it agreed.\n\n\
             This is a hard gate. Do not retry, do not work around it, and do not add the consent token yourself.\n\
             State the plan and what it will affect, then ask the user to approve running it with the token prefix:\n    \
             I_CONSENT=bulk-rm <the exact command>\n\
             Protocol: references/guardrail.md in the budd kit"
        );
        assert_eq!(
            consent_command.as_deref(),
            Some("I_CONSENT=bulk-rm rm -rf /Users/devsgrw/personal-files/peyman")
        );
        let output = hook_output(&verdict, Path::new("/h/state/consent.log")).unwrap();
        assert_eq!(output["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(output["hookSpecificOutput"]["permissionDecisionReason"], *reason);

        // Over ssh the reason says so, and the wrong token leaves the approval command clean.
        let remote = bash(r#"I_CONSENT=git-destroy ssh box "rm -rf /srv""#);
        let Verdict::Deny {
            reason,
            consent_command,
            ..
        } = &remote
        else {
            panic!("{remote:?}");
        };
        assert!(reason.starts_with("BLOCKED [bulk-rm]: (over ssh) recursively deletes: /srv\n"));
        assert_eq!(
            consent_command.as_deref(),
            Some(r#"I_CONSENT=bulk-rm ssh box "rm -rf /srv""#)
        );

        let asked = bash("mv /repo/src/*.vue /repo/old/");
        let Verdict::Ask { reason, .. } = &asked else {
            panic!("{asked:?}")
        };
        assert!(reason.starts_with("Fan-out change: a move over an unenumerated set of paths.\n"));
        let output = hook_output(&asked, Path::new("/h")).unwrap();
        assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "ask");

        let mail = evaluate("mcp__claude_ai_Gmail__send_message", json!({}), None, NO_FILE);
        let Verdict::Deny {
            reason,
            consent_command,
            ..
        } = &mail
        else {
            panic!("{mail:?}");
        };
        assert!(
            reason.starts_with(
                "BLOCKED [mail]: `mcp__claude_ai_Gmail__send_message` sends real mail to real recipients.\n"
            )
        );
        assert_eq!(*consent_command, None, "no token clears sending mail");

        assert_eq!(
            fail_closed_output("unreadable hook input")["hookSpecificOutput"]["permissionDecisionReason"],
            "destructive-gate: unreadable hook input, failing closed."
        );
    }

    #[test]
    fn a_spent_token_is_announced_once_per_slug() {
        let verdict = bash("I_CONSENT=bulk-rm rm -rf /a && I_CONSENT=bulk-rm rm -rf /b");
        assert_eq!(
            verdict,
            Verdict::Allow {
                consent_used: vec!["bulk-rm".to_owned(), "bulk-rm".to_owned()]
            }
        );
        let output = hook_output(&verdict, Path::new("/h/state/consent.log")).unwrap();
        assert_eq!(
            output["systemMessage"],
            "Guardrail consent used: bulk-rm. Logged to /h/state/consent.log."
        );
        assert_eq!(hook_output(&bash("ls"), Path::new("/h")), None);
    }

    #[test]
    fn consent_needs_the_human_to_have_typed_it() {
        let command = "I_CONSENT=bulk-rm rm -rf /Users/devsgrw/junk";
        // Typed: the token clears the deny.
        assert_eq!(classify(&bash(command)), "notice");
        // Absent from the transcript: refused, and the refusal says why.
        let absent = evaluate("Bash", json!({"command": command}), Some(&|_, _| false), NO_FILE);
        let Verdict::Deny {
            slug,
            reason,
            consent_command,
        } = &absent
        else {
            panic!("{absent:?}");
        };
        assert_eq!(*slug, "bulk-rm");
        assert!(reason.contains(NOT_TYPED), "{reason}");
        assert!(reason.starts_with("BLOCKED [bulk-rm]: recursively deletes: /Users/devsgrw/junk\nThe command carries I_CONSENT=bulk-rm, but the consent token was not typed by the human"), "{reason}");
        assert_eq!(
            consent_command.as_deref(),
            Some("I_CONSENT=bulk-rm rm -rf /Users/devsgrw/junk")
        );
        // No transcript to read: every token is refused.
        let unreadable = evaluate("Bash", json!({"command": command}), None, NO_FILE);
        assert!(matches!(&unreadable, Verdict::Deny { reason, .. } if reason.contains(NOT_TYPED)));
        // A token the human typed for another command does not carry over: the callback sees the token
        // and the command's first word, from the segment that carries the token.
        let seen = std::cell::RefCell::new(Vec::new());
        let recording = |token: &str, first: &str| {
            seen.borrow_mut().push((token.to_owned(), first.to_owned()));
            false
        };
        let _ = evaluate("Bash", json!({"command": command}), Some(&recording), NO_FILE);
        let _ = evaluate(
            "Bash",
            json!({"command": "cd /w && I_CONSENT=worktree-remove sudo git worktree remove ../x"}),
            Some(&recording),
            NO_FILE,
        );
        assert_eq!(
            seen.borrow().as_slice(),
            [
                ("I_CONSENT=bulk-rm".to_owned(), "rm".to_owned()),
                ("I_CONSENT=worktree-remove".to_owned(), "sudo".to_owned()),
            ]
        );
        // The callback is only asked when a token is on the command.
        let never = |_: &str, _: &str| panic!("asked without a token");
        assert_eq!(
            classify(&evaluate(
                "Bash",
                json!({"command": "rm -rf /x"}),
                Some(&never),
                NO_FILE
            )),
            "deny"
        );
        assert_eq!(
            classify(&evaluate("Bash", json!({"command": "ls"}), Some(&never), NO_FILE)),
            "allow"
        );
    }

    #[test]
    fn stripe_write_and_payment_write_both_clear_a_payment_write() {
        let curl = "curl -X POST https://api.stripe.com/v1/refunds -u sk_test_x: -d charge=ch_1";
        let denied = bash(curl);
        assert_eq!(denied.slug(), Some("payment-write"));
        assert!(
            matches!(&denied, Verdict::Deny { consent_command: Some(c), .. } if c == &format!("I_CONSENT=payment-write {curl}"))
        );
        assert_eq!(
            bash(&format!("I_CONSENT=payment-write {curl}")),
            Verdict::Allow {
                consent_used: vec!["payment-write".to_owned()]
            }
        );
        assert_eq!(
            bash(&format!("I_CONSENT=stripe-write {curl}")),
            Verdict::Allow {
                consent_used: vec!["stripe-write".to_owned()]
            }
        );
        // The alias goes one way: a payment token does not clear another slug, and vice versa.
        assert_eq!(bash("I_CONSENT=payment-write rm -rf /x").slug(), Some("bulk-rm"));
        assert_eq!(bash("I_CONSENT=stripe-write git push -f").slug(), Some("force-push"));
        // The Stripe CLI, the SDK and a live key all name the same slug.
        assert_eq!(
            bash("stripe refunds create --charge ch_1").slug(),
            Some("payment-write")
        );
        assert_eq!(
            bash("php -r 'Stripe\\Charge::create([]);'").slug(),
            Some("payment-write")
        );
        assert_eq!(bash("echo rk_live_123").slug(), Some("payment-write"));
    }

    #[test]
    fn a_redirect_onto_an_existing_file_asks() {
        let existing = |path: &Path| path == Path::new("/tmp/notes.txt") || path == Path::new("/abs/out.log");
        let run = |command: &str| classify(&evaluate("Bash", json!({"command": command}), Some(TYPED), &existing));
        assert_eq!(run("echo hi > notes.txt"), "ask", "relative to cwd");
        assert_eq!(run("echo hi >notes.txt"), "ask");
        assert_eq!(run("echo hi > /abs/out.log"), "ask");
        assert_eq!(run("echo hi >> notes.txt"), "allow", "append keeps the contents");
        assert_eq!(run("echo hi > fresh.txt"), "allow", "a new file has nothing to lose");
        assert_eq!(run("cmd 2>&1 > /dev/null"), "allow");
        assert_eq!(
            run("cmd > /tmp/claude-501/x/scratchpad/notes.txt"),
            "allow",
            "scratch is fine"
        );
        assert_eq!(run("echo '> notes.txt'"), "allow", "inside quotes");
        assert_eq!(run("test 1 -gt 0 && echo 'a >b'"), "allow");
        let asked = evaluate(
            "Bash",
            json!({"command": "echo hi > notes.txt"}),
            Some(TYPED),
            &existing,
        );
        assert!(
            matches!(&asked, Verdict::Ask { slug: "overwrite", reason } if reason.starts_with("Fan-out change: overwrites the existing file notes.txt by shell redirect.\n"))
        );
    }

    #[test]
    fn write_onto_an_existing_file_asks_and_edits_pass() {
        let existing = |path: &Path| path == Path::new("/repo/src/main.rs") || path.ends_with("node_modules/x.js");
        let write = |path: &str| evaluate("Write", json!({"file_path": path}), None, &existing);
        assert!(matches!(
            write("/repo/src/main.rs"),
            Verdict::Ask {
                slug: "write-replace",
                ..
            }
        ));
        assert_eq!(classify(&write("/repo/src/new.rs")), "allow");
        assert_eq!(
            classify(&write("/repo/node_modules/x.js")),
            "allow",
            "build output is fine to replace"
        );
        assert_eq!(classify(&write("")), "allow");
        assert_eq!(
            classify(&evaluate("Write", json!("not an object"), None, &existing)),
            "allow"
        );
        let multi = evaluate(
            "MultiEdit",
            json!({"file_path": "/r/a.ts", "replace_all": true}),
            None,
            NO_FILE,
        );
        assert!(
            matches!(&multi, Verdict::Ask { slug: "replace-all", reason } if reason.contains("replace_all across /r/a.ts."))
        );
        let not_a_bool = evaluate(
            "Edit",
            json!({"file_path": "/r/a.ts", "replace_all": "true"}),
            None,
            NO_FILE,
        );
        assert_eq!(classify(&not_a_bool), "allow");
        assert_eq!(
            classify(&evaluate("Read", json!({"file_path": "/r/a.ts"}), None, NO_FILE)),
            "allow"
        );
        assert_eq!(classify(&evaluate("", json!({}), None, NO_FILE)), "allow");
        // An empty command is nothing to judge.
        assert_eq!(
            classify(&evaluate("Bash", json!({"command": "   "}), None, NO_FILE)),
            "allow"
        );
        assert_eq!(classify(&evaluate("Bash", json!({}), None, NO_FILE)), "allow");
    }

    #[test]
    fn words_split_as_pythons_shlex_does() {
        let split = |text: &str| shlex_split(text).map(|words| words.join("|"));
        assert_eq!(split("a  b\tc").as_deref(), Some("a|b|c"));
        assert_eq!(split("a 'b c' d").as_deref(), Some("a|b c|d"));
        assert_eq!(split(r#"a "b c" d"#).as_deref(), Some("a|b c|d"));
        assert_eq!(split(r#"x "a\"b" y"#).as_deref(), Some(r#"x|a"b|y"#));
        assert_eq!(
            split(r#""a\nb""#).as_deref(),
            Some(r"a\nb"),
            "only the quote and the backslash are escapes inside double quotes"
        );
        assert_eq!(split(r"a\ b").as_deref(), Some("a b"));
        assert_eq!(
            split("a '' b").as_deref(),
            Some("a||b"),
            "an empty quoted word is a word"
        );
        assert_eq!(split("a'b'c").as_deref(), Some("abc"));
        assert_eq!(
            split(r"--execute='\Stripe\Refund::create();'").as_deref(),
            Some(r"--execute=\Stripe\Refund::create();")
        );
        assert_eq!(split("").as_deref(), Some(""));
        assert_eq!(split("  ").as_deref(), Some(""));
        assert_eq!(split("a 'open"), None);
        assert_eq!(split(r#"a "open"#), None);
        assert_eq!(split(r"a \"), None);
        // The segment split happens before the words are read, operators in quotes included.
        assert_eq!(
            segments(r#"ssh box "cd /app && php artisan queue:work""#),
            vec![
                vec!["ssh", "box", "\"cd", "/app"],
                vec!["php", "artisan", "queue:work\""],
            ]
        );
        assert_eq!(
            segments("a | b ; c\nd && e || f & g"),
            vec![
                vec!["a"],
                vec!["b"],
                vec!["c"],
                vec!["d"],
                vec!["e"],
                vec!["f"],
                vec!["g"]
            ]
        );
        assert_eq!(segments(" ;; "), Vec::<Vec<String>>::new());
    }

    #[test]
    fn wrappers_hosts_and_safe_paths() {
        let words = |text: &str| text.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert!(runs(&words("sudo -n env FOO=1 rm -rf x"), &["rm"]));
        assert!(runs(&words("FOO=1 /usr/bin/time xargs -0 rm"), &["rm"]));
        assert!(!runs(&words("echo rm"), &["rm"]));
        assert_eq!(invocation(&words("sudo")), Vec::<String>::new().as_slice());
        assert_eq!(basename("./awwt"), "awwt");
        assert_eq!(basename("/usr/bin/sudo"), "sudo");
        assert_eq!(basename("dir/"), "dir");
        assert_eq!(basename("."), "");
        assert_eq!(
            remote_commands(r#"sudo ssh -p 2222 -i key user@box "rm -rf /srv" && ls"#),
            vec!["rm -rf /srv"]
        );
        assert_eq!(remote_commands("ssh -v box"), Vec::<String>::new());
        assert_eq!(remote_commands("xargs ssh box ls"), Vec::<String>::new());
        assert!(is_safe_target("/repo/node_modules"));
        assert!(is_safe_target("node_modules/x"));
        assert!(is_safe_target("./dist//out"));
        assert!(is_safe_target("/private/tmp/claude-501/x/scratchpad/y"));
        assert!(!is_safe_target("/repo/src"));
        assert!(
            is_safe_target("distribution"),
            "a quirk kept from the Python: /dist is matched as a substring"
        );
        assert!(outside_quotes("echo 'a' > b", 9));
        assert!(!outside_quotes("echo 'a > b'", 8));
        assert_eq!(without_tokens("I_CONSENT=bulk-rm rm -rf /x"), "rm -rf /x");
        assert_eq!(
            first_word_after("cd /a && I_CONSENT=x FOO=1 git x", "I_CONSENT=x"),
            "git"
        );
        assert_eq!(first_word_after("echo 'I_CONSENT=x' ; ls", "I_CONSENT=x"), "echo");
        assert_eq!(first_word_after("", "I_CONSENT=x"), "");
    }
}
