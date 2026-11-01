//! Jira ticket keys in what the human typed, and the project folder and command each key starts in.

use regex::Regex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

static KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([A-Za-z][A-Za-z0-9]{1,9})-(\d{1,6})\b").unwrap());

/// Jira ticket keys in what the human typed into Start Tickets: "casbo-1104, CASBO-1105" is
/// `["CASBO-1104", "CASBO-1105"]`, each once, in the order typed.
pub struct TicketKeys;

impl TicketKeys {
    /// Every key in `text`, upper-cased, each once, in the order typed.
    pub fn parse(text: &str) -> Vec<String> {
        let mut keys: Vec<String> = Vec::new();
        for found in KEY.captures_iter(text) {
            let key = format!("{}-{}", found[1].to_uppercase(), &found[2]);
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
        keys
    }

    /// The project part of a key: "CASBO-1104" is "CASBO".
    pub fn prefix(key: &str) -> String {
        key.split('-').next().unwrap_or(key).to_uppercase()
    }
}

/// Where a ticket starts: the project folder its key's prefix belongs to, and the command that runs
/// it there. Each project keeps its own pipeline, so the command is the project's, not Budd's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketProject {
    /// The project folder, as written in the setting ("~/webserver/gamecenter").
    pub folder: String,
    /// The slash command that starts a ticket there, such as "/lets-do-it".
    pub command: String,
}

impl TicketProject {
    /// A project in `folder` started with `command`.
    pub fn new(folder: impl Into<String>, command: impl Into<String>) -> Self {
        TicketProject {
            folder: folder.into(),
            command: command.into(),
        }
    }

    /// The folder's last name, which is how the human calls a project ("gamecenter").
    pub fn name(&self) -> String {
        Path::new(&self.folder)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.folder.clone())
    }
}

/// Projects by Jira prefix, prefixes sorted.
pub type TicketTable = BTreeMap<String, TicketProject>;

/// A key with the project it starts in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routed {
    /// The ticket key.
    pub key: String,
    /// The project its prefix belongs to.
    pub project: TicketProject,
}

/// The projects Start Tickets can start a key in, by Jira prefix: what the app knows without being
/// told (its built-in table, which may be empty), and the command a setting falls back to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TicketProjects {
    /// Projects known without a setting.
    pub built_in: TicketTable,
    /// The command a setting entry without one gets.
    pub default_command: String,
}

impl TicketProjects {
    /// The table from the `ticketProjects` setting, an object of prefix to `{folder, command}`, on top
    /// of the built-in one: an entry adds a project or replaces a built-in one, and an entry with no
    /// folder is ignored, so a typo can never send a ticket somewhere unnamed. The command defaults to
    /// `default_command`.
    pub fn table(&self, setting: Option<&Value>) -> TicketTable {
        let mut table = self.built_in.clone();
        let entries = setting.and_then(Value::as_object);
        for (prefix, value) in entries.into_iter().flatten() {
            let Some(folder) = value
                .get("folder")
                .and_then(Value::as_str)
                .filter(|folder| !folder.is_empty())
            else {
                continue;
            };
            let command = value
                .get("command")
                .and_then(Value::as_str)
                .filter(|command| !command.is_empty())
                .unwrap_or(&self.default_command);
            table.insert(prefix.to_uppercase(), TicketProject::new(folder, command));
        }
        table
    }

    /// Each key with the project it starts in, and the keys whose prefix no project claims. Nothing
    /// should start while any key is unclaimed: guessing a project would put a ticket in the wrong repo.
    pub fn route(keys: &[String], table: &TicketTable) -> (Vec<Routed>, Vec<String>) {
        let mut routed = Vec::new();
        let mut unclaimed = Vec::new();
        for key in keys {
            match table.get(&TicketKeys::prefix(key)) {
                Some(project) => routed.push(Routed {
                    key: key.clone(),
                    project: project.clone(),
                }),
                None => unclaimed.push(key.clone()),
            }
        }
        (routed, unclaimed)
    }

    /// One line per project for the Start Tickets card, prefixes in order:
    /// "CASBO keys start `/lets-do-it-v2 <KEY>` in gamecenter."
    pub fn describe(table: &TicketTable) -> Vec<String> {
        table
            .iter()
            .map(|(prefix, project)| format!("{prefix} keys start `{} <KEY>` in {}.", project.command, project.name()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn keys(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|key| (*key).to_owned()).collect()
    }

    #[test]
    fn finds_each_key_once_in_the_order_typed() {
        assert_eq!(
            TicketKeys::parse("casbo-1104, CASBO-1105\nCASBO-1106 casbo-1104"),
            ["CASBO-1104", "CASBO-1105", "CASBO-1106"]
        );
        assert!(TicketKeys::parse("start the footer one").is_empty());
    }

    #[test]
    fn a_keys_prefix_is_its_project() {
        assert_eq!(TicketKeys::prefix("CASBO-1104"), "CASBO");
        assert_eq!(TicketKeys::prefix("gnx-12"), "GNX");
    }

    fn gamecenter() -> TicketProject {
        TicketProject::new("~/webserver/gamecenter", "/lets-do-it-v2")
    }

    /// The Swift app's built-in table, as a fixture: Game Center's CASBO tickets.
    fn projects() -> TicketProjects {
        TicketProjects {
            built_in: TicketTable::from([("CASBO".to_owned(), gamecenter())]),
            default_command: "/lets-do-it-v2".to_owned(),
        }
    }

    #[test]
    fn game_center_is_built_in() {
        assert_eq!(
            projects().table(None),
            TicketTable::from([("CASBO".to_owned(), gamecenter())])
        );
    }

    #[test]
    fn a_setting_adds_projects_beside_the_built_in_ones() {
        let setting = json!({
            "gnx": {"folder": "~/webserver/globalnet-portal", "command": "/lets-do-it"},
            "OPS": {"folder": "~/webserver/ops"},
        });
        let table = projects().table(Some(&setting));
        assert_eq!(table.get("CASBO"), Some(&gamecenter()));
        assert_eq!(
            table.get("GNX"),
            Some(&TicketProject::new("~/webserver/globalnet-portal", "/lets-do-it"))
        );
        assert_eq!(table.get("OPS").map(|p| p.command.as_str()), Some("/lets-do-it-v2"));
    }

    #[test]
    fn a_setting_can_move_a_built_in_project() {
        let table = projects().table(Some(&json!({"CASBO": {"folder": "~/webserver/gamecenterv2"}})));
        assert_eq!(
            table.get("CASBO").map(|p| p.folder.as_str()),
            Some("~/webserver/gamecenterv2")
        );
    }

    #[test]
    fn an_entry_without_a_folder_is_ignored() {
        let setting = json!({"GNX": {"command": "/lets-do-it"}, "OPS": {"folder": ""}, "BAD": "~/webserver/x"});
        assert_eq!(
            projects().table(Some(&setting)),
            TicketTable::from([("CASBO".to_owned(), gamecenter())])
        );
    }

    #[test]
    fn each_key_goes_to_its_prefixes_project() {
        let globalnet = TicketProject::new("~/webserver/globalnet-portal", "/lets-do-it-v2");
        let table = TicketTable::from([
            ("CASBO".to_owned(), gamecenter()),
            ("GNX".to_owned(), globalnet.clone()),
        ]);
        let (routed, unclaimed) = TicketProjects::route(&keys(&["CASBO-1104", "GNX-12", "CASBO-1105"]), &table);
        assert_eq!(
            routed.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
            ["CASBO-1104", "GNX-12", "CASBO-1105"]
        );
        assert_eq!(
            routed.iter().map(|r| r.project.clone()).collect::<Vec<_>>(),
            [gamecenter(), globalnet, gamecenter()]
        );
        assert!(unclaimed.is_empty());
    }

    #[test]
    fn a_key_no_project_claims_is_never_guessed() {
        let table = TicketTable::from([("CASBO".to_owned(), gamecenter())]);
        let (routed, unclaimed) = TicketProjects::route(&keys(&["CASBO-1104", "GNX-12"]), &table);
        assert_eq!(
            routed.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
            ["CASBO-1104"]
        );
        assert_eq!(unclaimed, ["GNX-12"]);
    }

    #[test]
    fn the_card_says_where_each_prefix_starts() {
        let table = TicketTable::from([
            (
                "GNX".to_owned(),
                TicketProject::new("~/webserver/globalnet-portal", "/lets-do-it"),
            ),
            ("CASBO".to_owned(), gamecenter()),
        ]);
        assert_eq!(
            TicketProjects::describe(&table),
            [
                "CASBO keys start `/lets-do-it-v2 <KEY>` in gamecenter.",
                "GNX keys start `/lets-do-it <KEY>` in globalnet-portal.",
            ]
        );
    }
}
