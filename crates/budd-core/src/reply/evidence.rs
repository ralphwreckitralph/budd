//! What a ticket session leaves for the human to check its work by: each expectation of the ticket,
//! how it was proven, and for the ones on screen a BEFORE and an AFTER frame of the same page in the
//! same state. The pipeline's `evidence` tool keeps it in `<config>/evidence/<TICKET>/evidence.json`,
//! outside the worktree, so it outlives the change; the crew card shows it beside the session's reply.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static TICKET_PATTERN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b[A-Z][A-Z0-9]*-[0-9]+\b").unwrap());

/// One expectation of the ticket and how it was proven.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// The expectation's id, such as "E1".
    pub id: String,
    /// What the expectation says, word for word from the session's expectation document.
    pub text: String,
    /// Something on screen shows it, so it has (or is missing) a BEFORE and an AFTER frame.
    pub on_screen: bool,
    /// The BEFORE frame, only when the file is there.
    pub before: Option<PathBuf>,
    /// The AFTER frame, only when the file is there.
    pub after: Option<PathBuf>,
    /// The test or check that proves it, when no frame does, or alongside one.
    pub proof: Option<String>,
}

impl Item {
    /// Whether the expectation is proven: by a check, or by an AFTER frame.
    pub fn proven(&self) -> bool {
        self.proof.is_some() || self.after.is_some()
    }
}

/// A ticket's evidence set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// The ticket key, such as "CASBO-1055".
    pub ticket: String,
    /// Its expectations, in the set's order.
    pub items: Vec<Item>,
}

#[derive(Deserialize)]
struct File {
    ticket: String,
    items: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    text: Option<String>,
    screen: Option<bool>,
    before: Option<String>,
    after: Option<String>,
    proof: Option<String>,
}

impl Evidence {
    /// The chip's label: how many expectations are proven, of how many.
    pub fn title(&self) -> String {
        let proven = self.items.iter().filter(|item| item.proven()).count();
        format!("Evidence {proven}/{}", self.items.len())
    }

    /// What is still missing, in one line: "E1 and E2 have no BEFORE · E6 has no proof", or `None` when
    /// nothing is. The same gaps the pipeline's `evidence check` names.
    pub fn gaps(&self) -> Option<String> {
        let ids = |matching: fn(&Item) -> bool, what: &str| -> Option<String> {
            let found: Vec<&str> = self
                .items
                .iter()
                .filter(|item| matching(item))
                .map(|item| item.id.as_str())
                .collect();
            let (last, rest) = found.split_last()?;
            let list = if rest.is_empty() {
                (*last).to_owned()
            } else {
                format!("{} and {last}", rest.join(", "))
            };
            let verb = if rest.is_empty() { "has" } else { "have" };
            Some(format!("{list} {verb} {what}"))
        };
        let lines: Vec<String> = [
            ids(|item| item.on_screen && item.before.is_none(), "no BEFORE"),
            ids(|item| item.on_screen && item.after.is_none(), "no AFTER"),
            ids(|item| !item.proven(), "no proof"),
        ]
        .into_iter()
        .flatten()
        .collect();
        if lines.is_empty() {
            None
        } else {
            Some(lines.join(" · "))
        }
    }

    /// The ticket a session works on, from the command it was started with ("/lets-do-it-v2 CASBO-1055").
    /// Only a key written in capitals counts, so a session id ("b80eaf88-45d2-…") is never taken for one.
    pub fn ticket(intent: Option<&str>) -> Option<String> {
        let intent = intent?;
        if !intent.starts_with('/') {
            return None;
        }
        TICKET_PATTERN.find(intent).map(|found| found.as_str().to_owned())
    }

    /// Where a ticket's evidence set is kept: `<config_dir>/evidence/<ticket>/evidence.json`. The caller
    /// passes `config_dir` with its "~" already expanded.
    pub fn manifest(ticket: &str, config_dir: &Path) -> PathBuf {
        config_dir.join("evidence").join(ticket).join("evidence.json")
    }

    /// The set in `data`, its frames resolved in `folder` and kept only when `exists` says the file is there.
    /// `None` when the data is not an evidence set, or lists nothing.
    pub fn decode(data: &[u8], folder: &Path, exists: impl Fn(&Path) -> bool) -> Option<Evidence> {
        let file: File = serde_json::from_slice(data).ok()?;
        if file.items.is_empty() {
            return None;
        }
        // A frame is a plain file name in the set's own folder; anything else is not ours to open.
        let frame = |name: Option<String>| -> Option<PathBuf> {
            let name = name.filter(|name| !name.is_empty() && !name.contains('/'))?;
            let path = folder.join(name);
            exists(&path).then_some(path)
        };
        let items = file
            .items
            .into_iter()
            .map(|entry| Item {
                id: entry.id,
                text: entry.text.unwrap_or_default(),
                on_screen: entry.screen == Some(true),
                before: frame(entry.before),
                after: frame(entry.after),
                proof: entry.proof.filter(|proof| !proof.trim().is_empty()),
            })
            .collect();
        Some(Evidence {
            ticket: file.ticket,
            items,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn finds_the_ticket_in_the_command_it_was_started_with() {
        assert_eq!(
            Evidence::ticket(Some("/lets-do-it-v2 CASBO-1055")).as_deref(),
            Some("CASBO-1055")
        );
        assert_eq!(
            Evidence::ticket(Some("/lets-finalize-it CASBO-1025 (from Step 9)")).as_deref(),
            Some("CASBO-1025")
        );
        assert_eq!(
            Evidence::ticket(Some("/lets-learn-it b80eaf88-45d2-4ed4-865e-2e13ac259beb")),
            None
        );
        assert_eq!(Evidence::ticket(Some("what about CASBO-1 today")), None);
        assert_eq!(Evidence::ticket(None), None);
    }

    #[test]
    fn reads_the_set_and_keeps_only_frames_that_are_there() {
        let json = r#"{"ticket":"CASBO-1055","created":"x","items":[
             {"id":"E1","text":"Header reads Bet Date/Time","screen":true,"before":"E1-before.png","after":"E1-after.jpg","url":"http://localhost:20600","steps":"Get a Report","proof":null},
             {"id":"E2","text":"Sorts by ReqDateTime","screen":true,"before":"E2-before.png","after":null,"proof":"  "},
             {"id":"E3","text":"CSV header","screen":false,"before":null,"after":null,"proof":"csvExport.spec.js"},
             {"id":"E4","text":"Not yet looked at","screen":null},
             {"id":"E5","text":"Escapes the folder","screen":true,"before":"../../secret.png","after":"/etc/x.png"}]}"#;
        let folder = Path::new("/ev/CASBO-1055");
        let there: HashSet<PathBuf> = [
            "/ev/CASBO-1055/E1-before.png",
            "/ev/CASBO-1055/E1-after.jpg",
            "/ev/secret.png",
            "/etc/x.png",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        let evidence = Evidence::decode(json.as_bytes(), folder, |path| there.contains(path)).unwrap();
        assert_eq!(evidence.ticket, "CASBO-1055");
        assert_eq!(
            evidence.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["E1", "E2", "E3", "E4", "E5"]
        );
        assert_eq!(
            evidence.items[0]
                .before
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str()),
            Some("E1-before.png")
        );
        assert!(evidence.items[0].after.is_some());
        assert_eq!(
            evidence.items[1].before, None,
            "a frame whose file is gone is not offered"
        );
        assert_eq!(evidence.items[1].proof, None, "a blank proof is no proof");
        assert!(!evidence.items[2].on_screen);
        assert_eq!(evidence.items[2].proof.as_deref(), Some("csvExport.spec.js"));
        assert!(!evidence.items[3].on_screen);
        assert!(
            evidence.items[4].before.is_none() && evidence.items[4].after.is_none(),
            "frames live in the set's own folder"
        );
        assert_eq!(
            evidence.items.iter().map(Item::proven).collect::<Vec<_>>(),
            [true, false, true, false, false]
        );
        assert_eq!(evidence.title(), "Evidence 2/5");
        assert_eq!(
            evidence.gaps().as_deref(),
            Some("E2 and E5 have no BEFORE · E2 and E5 have no AFTER · E2, E4 and E5 have no proof")
        );
    }

    #[test]
    fn a_complete_set_has_no_gaps() {
        let frame = PathBuf::from("/ev/f.png");
        let evidence = Evidence {
            ticket: "CASBO-1".into(),
            items: vec![
                Item {
                    id: "E1".into(),
                    text: "a".into(),
                    on_screen: true,
                    before: Some(frame.clone()),
                    after: Some(frame),
                    proof: None,
                },
                Item {
                    id: "E2".into(),
                    text: "b".into(),
                    on_screen: false,
                    before: None,
                    after: None,
                    proof: Some("spec".into()),
                },
            ],
        };
        assert_eq!(evidence.gaps(), None);
        assert_eq!(evidence.title(), "Evidence 2/2");
    }

    #[test]
    fn refuses_what_is_not_an_evidence_set() {
        let folder = Path::new("/ev");
        assert_eq!(Evidence::decode(b"{}", folder, |_| true), None);
        assert_eq!(
            Evidence::decode(br#"{"ticket":"CASBO-1","items":[]}"#, folder, |_| true),
            None
        );
        assert_eq!(Evidence::decode(b"not json", folder, |_| true), None);
    }

    #[test]
    fn keeps_the_set_in_the_config_folder() {
        assert_eq!(
            Evidence::manifest("CASBO-1", Path::new("/c")),
            PathBuf::from("/c/evidence/CASBO-1/evidence.json")
        );
    }
}
