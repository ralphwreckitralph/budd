//! Account status, MCP server status, folder trust and the settings Budd writes (from AvatarCore/ClaudeAccount.swift).
//!
//! Budd has a Claude config folder of his own, apart from the human's. This module reads what Claude
//! Code says about such a folder (`claude auth status --json`, `claude mcp list`), carries the human's
//! folder trust over to it, and keeps its `settings.json` wired to Budd's hooks.

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::LazyLock;

/// The setting that names Budd's config folder: the single answer to "which account". Connect writes it
/// and Reset removes it; unset means not connected.
pub const CONFIG_DIR_KEY: &str = "claudeConfigDir";
/// The folder Connect creates and logs in: Budd's alone, apart from the human's own Claude.
pub const BUDD_FOLDER: &str = "~/.claude-budd";
/// The name Budd's own MCP server is registered under in his folder.
pub const SERVER_NAME: &str = "budd";

/// Who a Claude config folder is logged in as, from `claude auth status --json`. Every field but
/// `logged_in` is optional, as the output belongs to Claude Code and may change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountStatus {
    /// Whether the folder has an account at all.
    pub logged_in: bool,
    /// The account's email.
    pub email: Option<String>,
    /// The organization, from `orgName`.
    pub organization: Option<String>,
    /// The subscription, from `subscriptionType`: "max", "team", "pro".
    pub plan: Option<String>,
    /// How the folder logged in, from `authMethod`: "claude.ai", "console", "none".
    pub auth_method: Option<String>,
    /// The folder the status is about, from `configDirectory`.
    pub config_directory: Option<String>,
}

impl AccountStatus {
    /// The status in the command's output, or `None` when it is not JSON with a `loggedIn` flag.
    /// Empty strings read as absent.
    pub fn decode(text: &str) -> Option<AccountStatus> {
        let object: Value = serde_json::from_str(text).ok()?;
        let logged_in = object.get("loggedIn")?.as_bool()?;
        let text = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Some(AccountStatus {
            logged_in,
            email: text("email"),
            organization: text("orgName"),
            plan: text("subscriptionType"),
            auth_method: text("authMethod"),
            config_directory: text("configDirectory"),
        })
    }

    /// One line for the account card: "ralph@example.com · Team plan · GlobalNet".
    pub fn summary(&self) -> String {
        if !self.logged_in {
            return "Not logged in".to_owned();
        }
        let plan = self.plan.as_deref().map(|plan| {
            let mut chars = plan.chars();
            let first: String = chars.next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
            format!("{first}{} plan", chars.as_str())
        });
        [
            Some(self.email.clone().unwrap_or_else(|| "Logged in".to_owned())),
            plan,
            self.organization.clone(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    }
}

/// How an MCP server came out of `claude mcp list`'s health check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpState {
    /// "✔ Connected".
    Connected,
    /// "! Needs authentication".
    NeedsLogin,
    /// "✘ Failed to connect".
    Failed,
    /// "⏸ Pending approval".
    Pending,
    /// A mark this module does not know.
    Other,
}

/// One MCP server a config folder's sessions load, as `claude mcp list` reports it after its health check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerStatus {
    /// The server's name, such as "claude.ai Atlassian".
    pub name: String,
    /// How it came out of the health check.
    pub state: McpState,
    /// What `claude mcp list` said after the dash, such as "✘ Failed to connect".
    pub detail: String,
}

/// "claude.ai Slack - GCI: https://mcp.slack.com/… - ✔ Connected" is the server "claude.ai Slack - GCI":
/// the name runs to the first ": ", and the state follows the last " - " that a status mark starts.
static SERVER_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.+?): (.+) - ([✔✘!⏸].*)$").unwrap());

impl McpServerStatus {
    /// The servers in the output of `claude mcp list`, in its order; its other lines are skipped.
    pub fn parse(output: &str) -> Vec<McpServerStatus> {
        output
            .lines()
            .filter_map(|raw| SERVER_LINE.captures(raw.trim()))
            .map(|caps| {
                let detail = caps[3].to_owned();
                let state = match detail.chars().next() {
                    Some('✔') => McpState::Connected,
                    Some('!') if detail.to_lowercase().contains("auth") => McpState::NeedsLogin,
                    Some('!') => McpState::Other,
                    Some('✘') => McpState::Failed,
                    Some('⏸') => McpState::Pending,
                    _ => McpState::Other,
                };
                McpServerStatus {
                    name: caps[1].to_owned(),
                    state,
                    detail,
                }
            })
            .collect()
    }
}

/// Which folders a Claude config folder trusts. Claude Code asks once per folder whether to trust it, and
/// a background session cannot answer, so a session started in a folder Budd's account never trusted
/// would wait forever. A new folder of Budd's takes on the trust the human already gave under their own.
pub struct FolderTrust;

impl FolderTrust {
    /// The folders `config` (the parsed `.claude.json` of a config folder) trusts, sorted.
    pub fn trusted(config: &Value) -> Vec<String> {
        let mut folders: Vec<String> = config
            .get("projects")
            .and_then(Value::as_object)
            .map(|projects| {
                projects
                    .iter()
                    .filter(|(_, project)| project.get("hasTrustDialogAccepted").and_then(Value::as_bool) == Some(true))
                    .map(|(path, _)| path.clone())
                    .collect()
            })
            .unwrap_or_default();
        folders.sort();
        folders
    }

    /// `config` trusting `folders` as well, every other setting as it was.
    pub fn trusting<S: AsRef<str>>(config: Value, folders: &[S]) -> Value {
        let mut config = match config {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        let mut projects = match config.remove("projects") {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        };
        for folder in folders {
            let mut project = match projects.remove(folder.as_ref()) {
                Some(Value::Object(map)) => map,
                _ => Map::new(),
            };
            project.insert("hasTrustDialogAccepted".to_owned(), Value::Bool(true));
            projects.insert(folder.as_ref().to_owned(), Value::Object(project));
        }
        config.insert("projects".to_owned(), Value::Object(projects));
        Value::Object(config)
    }
}

/// The `settings.json` of Budd's folder: auto mode, and Budd's own binary as the hook on every event.
pub mod settings {
    use serde_json::{Map, Value, json};

    /// The version of Budd's wiring, under the top-level `_budd` marker.
    pub const VERSION: u64 = 2;
    /// The events Budd's hook runs on.
    pub const EVENTS: &[&str] = &[
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "Notification",
        "Stop",
        "StopFailure",
        "SessionEnd",
    ];
    /// Seconds a hook may take before Claude Code gives up on it.
    pub const TIMEOUT: u64 = 10;

    /// Budd's hook on `event`: exec form, so no shell quoting of the binary's path can go wrong.
    pub fn hook(budd_exe: &str, event: &str) -> Value {
        json!({
            "hooks": [{"type": "command", "command": budd_exe, "args": ["hook", event], "timeout": TIMEOUT}]
        })
    }

    /// Whether a hook entry is Budd's: every command in it is `budd_exe` or the binary a previous
    /// reconcile recorded, so a moved binary's entries are replaced and the human's own hooks kept.
    fn is_budds(entry: &Value, exes: &[&str]) -> bool {
        entry
            .get("hooks")
            .and_then(Value::as_array)
            .filter(|hooks| !hooks.is_empty())
            .is_some_and(|hooks| {
                hooks.iter().all(|hook| {
                    hook.get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|command| exes.contains(&command))
                })
            })
    }

    /// `existing` (the settings file as it is, or `None` without one) with `permissions.defaultMode`
    /// set to auto, Budd's hook first on each of [`EVENTS`], and the `_budd` marker, together with
    /// whether that differs from `existing`. Only entries whose commands are Budd's binary, now or as
    /// last recorded, are replaced; the human's other hooks and settings stay as they were.
    pub fn reconcile(existing: Option<Value>, budd_exe: &str) -> (Value, bool) {
        let mut settings = match &existing {
            Some(Value::Object(map)) => map.clone(),
            _ => Map::new(),
        };
        let previous = settings
            .get("_budd")
            .and_then(|marker| marker.get("command"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let exes: Vec<&str> = [Some(budd_exe), previous.as_deref()].into_iter().flatten().collect();

        let mut permissions = match settings.remove("permissions") {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        };
        permissions.insert("defaultMode".to_owned(), Value::String("auto".to_owned()));
        settings.insert("permissions".to_owned(), Value::Object(permissions));

        let mut hooks = match settings.remove("hooks") {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        };
        for event in EVENTS {
            let theirs: Vec<Value> = match hooks.remove(*event) {
                Some(Value::Array(entries)) => entries.into_iter().filter(|e| !is_budds(e, &exes)).collect(),
                _ => Vec::new(),
            };
            let mut entries = vec![hook(budd_exe, event)];
            entries.extend(theirs);
            hooks.insert((*event).to_owned(), Value::Array(entries));
        }
        settings.insert("hooks".to_owned(), Value::Object(hooks));
        settings.insert("_budd".to_owned(), json!({"version": VERSION, "command": budd_exe}));

        let result = Value::Object(settings);
        let changed = existing.as_ref() != Some(&result);
        (result, changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_who_the_folder_is_logged_in_as() {
        let json = r#"{"loggedIn": true, "authMethod": "claude.ai", "email": "budd@example.com", "orgName": "GlobalNet", "subscriptionType": "team"}"#;
        let status = AccountStatus::decode(json).expect("decodes");
        assert_eq!(
            status,
            AccountStatus {
                logged_in: true,
                email: Some("budd@example.com".to_owned()),
                organization: Some("GlobalNet".to_owned()),
                plan: Some("team".to_owned()),
                auth_method: Some("claude.ai".to_owned()),
                config_directory: None,
            }
        );
        assert_eq!(status.summary(), "budd@example.com · Team plan · GlobalNet");
    }

    #[test]
    fn a_folder_nobody_logged_in_is_not_logged_in() {
        let json = r#"{"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty"}"#;
        let status = AccountStatus::decode(json).expect("decodes");
        assert!(!status.logged_in);
        assert_eq!(status.summary(), "Not logged in");
    }

    #[test]
    fn something_else_is_no_status() {
        assert_eq!(AccountStatus::decode("Not JSON"), None);
        assert_eq!(AccountStatus::decode(r#"{"email": "x"}"#), None);
        assert_eq!(AccountStatus::decode(r#"{"loggedIn": "yes"}"#), None);
    }

    #[test]
    fn empty_strings_are_absent_and_the_summary_fills_in() {
        let json = r#"{"loggedIn": true, "email": "", "orgName": "", "subscriptionType": "", "configDirectory": "/Users/a/.claude-budd"}"#;
        let status = AccountStatus::decode(json).expect("decodes");
        assert_eq!(status.email, None);
        assert_eq!(status.plan, None);
        assert_eq!(status.config_directory.as_deref(), Some("/Users/a/.claude-budd"));
        assert_eq!(status.summary(), "Logged in");
    }

    #[test]
    fn reads_each_server_and_its_state() {
        let output = "Checking MCP server health…\n\n\
            claude.ai Slack - GCI: https://mcp.slack.com/mcp - ! Needs authentication\n\
            claude.ai Atlassian: https://mcp.atlassian.com/v1/mcp - ✔ Connected\n\
            plugin:figma:figma: https://mcp.figma.com/mcp (HTTP) - ! Needs authentication\n\
            budd: /Applications/Budd.app/Contents/MacOS/AssistantAvatar --mcp - ✔ Connected\n\
            figma-desktop: http://127.0.0.1:3845/mcp (HTTP) - ✘ Failed to connect — ECONNREFUSED\n\
            local: npx thing - ⏸ Pending approval\n";
        let servers = McpServerStatus::parse(output);
        assert_eq!(
            servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            [
                "claude.ai Slack - GCI",
                "claude.ai Atlassian",
                "plugin:figma:figma",
                "budd",
                "figma-desktop",
                "local"
            ]
        );
        assert_eq!(
            servers.iter().map(|s| s.state).collect::<Vec<_>>(),
            [
                McpState::NeedsLogin,
                McpState::Connected,
                McpState::NeedsLogin,
                McpState::Connected,
                McpState::Failed,
                McpState::Pending
            ]
        );
        assert_eq!(servers[4].detail, "✘ Failed to connect — ECONNREFUSED");
        assert_eq!(
            McpServerStatus::parse("x: y - ! Something else")[0].state,
            McpState::Other
        );
    }

    #[test]
    fn no_servers_is_an_empty_list() {
        assert_eq!(
            McpServerStatus::parse("No MCP servers configured. Use `claude mcp add` to add a server."),
            []
        );
    }

    #[test]
    fn carries_trust_over_and_keeps_the_rest() {
        let old = json!({
            "projects": {
                "/w/assistant": {"hasTrustDialogAccepted": true, "mcpServers": {}},
                "/w/gamecenter": {"hasTrustDialogAccepted": true},
                "/w/untrusted": {"hasTrustDialogAccepted": false},
            }
        });
        assert_eq!(FolderTrust::trusted(&old), ["/w/assistant", "/w/gamecenter"]);
        let new = json!({"userID": "x", "projects": {"/w/gamecenter": {"allowedTools": ["Bash"]}}});
        let merged = FolderTrust::trusting(new, &FolderTrust::trusted(&old));
        assert_eq!(merged["userID"], "x");
        let projects = merged["projects"].as_object().expect("projects");
        assert_eq!(projects["/w/assistant"]["hasTrustDialogAccepted"], true);
        assert_eq!(projects["/w/gamecenter"]["hasTrustDialogAccepted"], true);
        assert_eq!(projects["/w/gamecenter"]["allowedTools"], json!(["Bash"]));
        assert!(!projects.contains_key("/w/untrusted"));
        // A config with no projects at all, or none that is an object, still takes the trust.
        assert_eq!(FolderTrust::trusted(&json!({})), Vec::<String>::new());
        let fresh = FolderTrust::trusting(json!({}), &["/w/a"]);
        assert_eq!(fresh["projects"]["/w/a"]["hasTrustDialogAccepted"], true);
    }

    #[test]
    fn reconcile_writes_the_mode_the_hooks_and_the_marker() {
        let (settings, changed) = settings::reconcile(None, "/Applications/Budd.app/Contents/MacOS/budd");
        assert!(changed);
        assert_eq!(settings["permissions"]["defaultMode"], "auto");
        assert_eq!(settings["_budd"]["version"], settings::VERSION);
        for event in settings::EVENTS {
            let entries = settings["hooks"][*event].as_array().expect("entries");
            assert_eq!(entries.len(), 1, "{event}");
            let hook = &entries[0]["hooks"][0];
            assert_eq!(hook["type"], "command");
            assert_eq!(hook["command"], "/Applications/Budd.app/Contents/MacOS/budd");
            assert_eq!(hook["args"], json!(["hook", event]));
            assert_eq!(hook["timeout"], settings::TIMEOUT);
        }
        // Doing it again changes nothing.
        let (again, changed) =
            settings::reconcile(Some(settings.clone()), "/Applications/Budd.app/Contents/MacOS/budd");
        assert!(!changed);
        assert_eq!(again, settings);
    }

    #[test]
    fn reconcile_keeps_the_humans_hooks_and_settings() {
        let existing = json!({
            "model": "opus",
            "permissions": {"defaultMode": "default", "allow": ["Bash(ls)"]},
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash|Edit",
                    "hooks": [{"type": "command", "command": "python3 ~/.claude/hooks/destructive-gate.py", "timeout": 15}]
                }],
                "PreCompact": [{"hooks": [{"type": "command", "command": "echo compact"}]}]
            }
        });
        let (settings, changed) = settings::reconcile(Some(existing), "/opt/budd");
        assert!(changed);
        assert_eq!(settings["model"], "opus");
        assert_eq!(settings["permissions"]["defaultMode"], "auto");
        assert_eq!(settings["permissions"]["allow"], json!(["Bash(ls)"]));
        let pre = settings["hooks"]["PreToolUse"].as_array().expect("entries");
        assert_eq!(pre.len(), 2);
        assert_eq!(pre[0]["hooks"][0]["command"], "/opt/budd");
        assert_eq!(pre[1]["matcher"], "Bash|Edit");
        assert_eq!(
            settings["hooks"]["PreCompact"][0]["hooks"][0]["command"],
            "echo compact"
        );
    }

    #[test]
    fn reconcile_replaces_a_moved_binary_and_nothing_else() {
        let (old, _) = settings::reconcile(None, "/old/budd");
        let (moved, changed) = settings::reconcile(Some(old), "/new/budd");
        assert!(changed);
        for event in settings::EVENTS {
            let entries = moved["hooks"][*event].as_array().expect("entries");
            assert_eq!(entries.len(), 1, "{event}");
            assert_eq!(entries[0]["hooks"][0]["command"], "/new/budd");
        }
        assert_eq!(moved["_budd"]["command"], "/new/budd");
        // Without the marker, an entry of an unknown binary is the human's and stays.
        let stranger = json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "/other/budd", "args": ["hook", "Stop"]}]}]}});
        let (kept, _) = settings::reconcile(Some(stranger), "/new/budd");
        assert_eq!(kept["hooks"]["Stop"].as_array().expect("entries").len(), 2);
        // A file that is not an object is started over.
        let (fresh, changed) = settings::reconcile(Some(json!("garbage")), "/new/budd");
        assert!(changed);
        assert_eq!(fresh["permissions"]["defaultMode"], "auto");
    }
}
