//! `budd evidence`: one evidence set per ticket, from the pipeline's `bin/evidence` (Python), verb for
//! verb and line for line.
//!
//! A ticket run proves its expectations (E1..En from the expectation document). For the ones a screen
//! shows, the human wants to SEE them: the same page in the same state before the change and after it.
//! The set lives at `<config dir>/evidence/<TICKET>/evidence.json`, outside every worktree, so it
//! outlives `git worktree remove`; Budd's office reads it there (`budd_core::reply::evidence`) and
//! shows the frames side by side on the session's card. Frames are copied in as `E1-before.png`,
//! `E1-after.jpg`, ..., and a frame's recipe as `E1.recipe.mjs`.
//!
//! `check` exits 1 naming every expectation not yet marked screen yes or no, every on-screen one
//! missing a frame, and every one with no proof at all (a proof line, or an AFTER frame). It is a
//! report for the hand-back, not a gate. Exit 0 ok, 1 gaps (check), 2 usage or input error.

use crate::paths::write_atomically;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::ser::PrettyFormatter;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// The verbs, one line each, printed under every refusal.
pub const USAGE: &str = "    budd evidence init   <TICKET> --expectations <expectations.json>
    budd evidence screen <TICKET> <E id> yes|no
    budd evidence add    <TICKET> <E id> before|after <image> [--url <url>] [--steps \"<what was done on the page>\"] [--recipe <file>]
    budd evidence proof  <TICKET> <E id> \"<the test or check that proves it>\"
    budd evidence check  <TICKET>
    budd evidence show   <TICKET>";

/// What `shoot` says: the frames module is a separate install, and this build has none.
pub const SHOTS_NOT_INSTALLED: &str = "evidence shots are not installed";

/// The image types a frame may be.
pub const IMAGE_EXTENSIONS: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".webp"];

/// A Jira-style key (`CASBO-1055`) or the kit's brief key (`BRIEF-20261101-fix-the-thing`).
static TICKET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[A-Z][A-Z0-9]*-[0-9]+|BRIEF-[0-9]{8}-[a-z0-9][a-z0-9-]*)$").unwrap_or_else(|e| panic!("{e}"))
});
/// An expectation id, `E1`.
static EID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^E[0-9]+$").unwrap_or_else(|e| panic!("{e}")));

/// Why a verb was refused; printed as `evidence: <why>` with the usage, exit 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused(pub String);

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

fn refused(why: impl Into<String>) -> Refused {
    Refused(why.into())
}

/// One expectation of the set, in the manifest's key order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// `E1`.
    pub id: String,
    /// The expectation's words, from the document.
    pub text: String,
    /// Whether a screen shows it: `None` until `screen` says.
    #[serde(default)]
    pub screen: Option<bool>,
    /// The BEFORE frame's file name in the set's folder.
    #[serde(default)]
    pub before: Option<String>,
    /// The AFTER frame's file name in the set's folder.
    #[serde(default)]
    pub after: Option<String>,
    /// The page the frames show.
    #[serde(default)]
    pub url: Option<String>,
    /// What was done on the page before the frame.
    #[serde(default)]
    pub steps: Option<String>,
    /// The test or check that proves it.
    #[serde(default)]
    pub proof: Option<String>,
}

impl Item {
    fn blank(id: &str, text: &str) -> Item {
        Item {
            id: id.to_owned(),
            text: text.to_owned(),
            screen: None,
            before: None,
            after: None,
            url: None,
            steps: None,
            proof: None,
        }
    }

    fn frame(&self, when: When) -> Option<&str> {
        match when {
            When::Before => self.before.as_deref(),
            When::After => self.after.as_deref(),
        }
        .filter(|name| !name.is_empty())
    }

    fn set_frame(&mut self, when: When, name: String) {
        match when {
            When::Before => self.before = Some(name),
            When::After => self.after = Some(name),
        }
    }

    fn has_proof(&self) -> bool {
        self.proof.as_deref().is_some_and(|proof| !proof.is_empty())
    }
}

/// The manifest, `evidence.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The ticket key.
    pub ticket: String,
    /// When the set was started.
    pub created: String,
    /// The expectations, in the document's order.
    pub items: Vec<Item>,
    /// When the set was last written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated: Option<String>,
}

/// Which frame of a pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    /// Taken while the worktree is still the base.
    Before,
    /// Taken on the tree that ships.
    After,
}

impl When {
    /// `before` or `after`, as the verbs and the file names say it.
    pub fn name(self) -> &'static str {
        match self {
            When::Before => "before",
            When::After => "after",
        }
    }

    /// `BEFORE` or `AFTER`, as the gaps say it.
    fn shout(self) -> &'static str {
        match self {
            When::Before => "BEFORE",
            When::After => "AFTER",
        }
    }

    /// The word as typed, or a refusal.
    pub fn parse(word: &str) -> Result<When, Refused> {
        match word {
            "before" => Ok(When::Before),
            "after" => Ok(When::After),
            _ => Err(refused("add takes before or after")),
        }
    }
}

/// One verb with its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Starts or re-syncs the set from the expectation sidecar.
    Init {
        /// The ticket.
        ticket: String,
        /// The `expectations.json` sidecar `budd expect sidecar` wrote.
        expectations: PathBuf,
    },
    /// Says whether a screen shows the expectation.
    Screen {
        /// The ticket.
        ticket: String,
        /// `E1`.
        id: String,
        /// `yes` or `no`.
        answer: String,
    },
    /// Copies a frame in.
    Add {
        /// The ticket.
        ticket: String,
        /// `E1`.
        id: String,
        /// `before` or `after`.
        when: String,
        /// The image.
        image: PathBuf,
        /// The page it shows.
        url: Option<String>,
        /// What was done on the page.
        steps: Option<String>,
        /// The recipe that produced it, kept as `E<n>.recipe.mjs` for the AFTER replay.
        recipe: Option<PathBuf>,
    },
    /// Records the test or check that proves an expectation.
    Proof {
        /// The ticket.
        ticket: String,
        /// `E1`.
        id: String,
        /// The proof.
        text: String,
    },
    /// Names every gap; exit 1 when there is one.
    Check {
        /// The ticket.
        ticket: String,
    },
    /// Lists each expectation with its frames and proof.
    Show {
        /// The ticket.
        ticket: String,
    },
    /// Takes a frame: only with the evidence-shots module, which this build has not.
    Shoot,
}

/// The set's folder and file for `ticket` under `config_dir`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// The evidence root under the Claude config folder: `<config_dir>/evidence`.
    pub fn new(config_dir: &Path) -> Store {
        Store {
            root: config_dir.join("evidence"),
        }
    }

    /// `<root>/<ticket>`, for a ticket key that is one.
    pub fn folder(&self, ticket: &str) -> Result<PathBuf, Refused> {
        if !TICKET.is_match(ticket) {
            return Err(refused(format!("{ticket:?} is not a ticket key like CASBO-1055")));
        }
        Ok(self.root.join(ticket))
    }

    /// `<root>/<ticket>/evidence.json`.
    pub fn manifest(&self, ticket: &str) -> Result<PathBuf, Refused> {
        Ok(self.folder(ticket)?.join("evidence.json"))
    }

    /// The set as written, or why there is none yet.
    pub fn load(&self, ticket: &str) -> Result<Manifest, Refused> {
        let path = self.manifest(ticket)?;
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| refused(format!("{} is not an evidence set: {error}", path.display()))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(refused(format!(
                "no evidence set for {ticket}; run: evidence init {ticket} --expectations <file>"
            ))),
            Err(error) => Err(refused(format!("cannot read {}: {error}", path.display()))),
        }
    }

    /// Writes the set, stamped `updated`, through a temporary file beside it.
    pub fn save(&self, data: &mut Manifest) -> Result<(), Refused> {
        data.updated = Some(super::Utc::now().iso());
        let path = self.manifest(&data.ticket)?;
        let bytes = encode(data).map_err(|error| refused(format!("encoding the set: {error}")))?;
        write_atomically(&path, &bytes).map_err(|error| refused(format!("cannot write {}: {error}", path.display())))
    }
}

/// The manifest as the Python wrote it: one-space indent and a final newline.
pub fn encode(data: &Manifest) -> Result<Vec<u8>, serde_json::Error> {
    let mut bytes = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, PrettyFormatter::with_indent(b" "));
    data.serialize(&mut serializer)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn item<'a>(data: &'a mut Manifest, id: &str) -> Result<&'a mut Item, Refused> {
    if !EID.is_match(id) {
        return Err(refused(format!("{id:?} is not an expectation id like E1")));
    }
    match data.items.iter().position(|item| item.id == id) {
        Some(index) => Ok(&mut data.items[index]),
        None => {
            let ids: Vec<&str> = data.items.iter().map(|item| item.id.as_str()).collect();
            Err(refused(format!(
                "{} has no {id}; the ids are {}",
                data.ticket,
                ids.join(", ")
            )))
        }
    }
}

/// Runs one verb against the sets under `config_dir`, printing what the Python printed to `out`, and
/// gives the exit code: 0, or 1 from `check` with gaps. A refusal is the caller's to print, with exit 2.
pub fn run(config_dir: &Path, command: &Command, out: &mut dyn Write) -> Result<i32, Refused> {
    let store = Store::new(config_dir);
    let say = |out: &mut dyn Write, line: String| -> Result<(), Refused> {
        writeln!(out, "{line}").map_err(|error| refused(format!("writing output: {error}")))
    };
    match command {
        Command::Init { ticket, expectations } => {
            let wanted = acceptance(expectations, ticket)?;
            let mut data = match store.load(ticket) {
                Ok(data) => data,
                Err(_) => Manifest {
                    ticket: ticket.clone(),
                    created: super::Utc::now().iso(),
                    items: Vec::new(),
                    updated: None,
                },
            };
            // A re-run keeps the frames and proofs already taken, and follows the document's ids and words.
            let kept = std::mem::take(&mut data.items);
            for (id, text) in &wanted {
                let mut item = kept
                    .iter()
                    .find(|item| &item.id == id)
                    .cloned()
                    .unwrap_or_else(|| Item::blank(id, text));
                item.text = text.clone();
                data.items.push(item);
            }
            store.save(&mut data)?;
            say(
                out,
                format!(
                    "evidence: {ticket} has {} expectations at {}",
                    wanted.len(),
                    store.folder(ticket)?.display()
                ),
            )?;
            Ok(0)
        }
        Command::Screen { ticket, id, answer } => {
            let yes = match answer.as_str() {
                "yes" => true,
                "no" => false,
                _ => return Err(refused("screen takes yes or no")),
            };
            let mut data = store.load(ticket)?;
            item(&mut data, id)?.screen = Some(yes);
            store.save(&mut data)?;
            Ok(0)
        }
        Command::Add {
            ticket,
            id,
            when,
            image,
            url,
            steps,
            recipe,
        } => {
            let when = When::parse(when)?;
            let ext = extension(image);
            if !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
                return Err(refused(format!(
                    "{} is not an image ({})",
                    image.display(),
                    IMAGE_EXTENSIONS.join(", ")
                )));
            }
            if !image.is_file() {
                return Err(refused(format!("{} does not exist", image.display())));
            }
            if let Some(recipe) = recipe
                && !recipe.is_file()
            {
                return Err(refused(format!("{} does not exist", recipe.display())));
            }
            let folder = store.folder(ticket)?;
            let mut data = store.load(ticket)?;
            let found = item(&mut data, id)?;
            let name = format!("{id}-{}{ext}", when.name());
            let old = found.frame(when).map(str::to_owned);
            std::fs::copy(image, folder.join(&name))
                .map_err(|error| refused(format!("cannot copy {} into the set: {error}", image.display())))?;
            if let Some(old) = old
                && old != name
                && folder.join(&old).is_file()
            {
                let _ = std::fs::remove_file(folder.join(&old));
            }
            if let Some(recipe) = recipe {
                std::fs::copy(recipe, folder.join(format!("{id}.recipe.mjs")))
                    .map_err(|error| refused(format!("cannot copy {} into the set: {error}", recipe.display())))?;
            }
            found.set_frame(when, name.clone());
            found.screen = Some(true);
            if let Some(url) = url.as_deref().filter(|url| !url.is_empty()) {
                found.url = Some(url.to_owned());
            }
            if let Some(steps) = steps.as_deref().filter(|steps| !steps.is_empty()) {
                found.steps = Some(steps.to_owned());
            }
            store.save(&mut data)?;
            say(
                out,
                format!("evidence: {id} {} -> {}", when.name(), folder.join(&name).display()),
            )?;
            Ok(0)
        }
        Command::Proof { ticket, id, text } => {
            let text = text.trim();
            if text.is_empty() {
                return Err(refused("the proof is empty"));
            }
            let mut data = store.load(ticket)?;
            item(&mut data, id)?.proof = Some(text.to_owned());
            store.save(&mut data)?;
            Ok(0)
        }
        Command::Check { ticket } => {
            let missing = gaps(&store.load(ticket)?);
            for line in &missing {
                say(out, line.clone())?;
            }
            let verdict = if missing.is_empty() {
                "complete".to_owned()
            } else {
                format!("{} gap(s)", missing.len())
            };
            say(out, format!("evidence {ticket}: {verdict}"))?;
            Ok(if missing.is_empty() { 0 } else { 1 })
        }
        Command::Show { ticket } => {
            let data = store.load(ticket)?;
            say(
                out,
                format!("Evidence for {ticket}: {}", store.manifest(ticket)?.display()),
            )?;
            for item in &data.items {
                let frames = if item.screen == Some(true) {
                    [When::Before, When::After]
                        .iter()
                        .map(|when| {
                            format!(
                                "{} {}",
                                when.name(),
                                if item.frame(*when).is_some() { "yes" } else { "MISSING" }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(" / ")
                } else {
                    "no screen".to_owned()
                };
                let proof = match &item.proof {
                    Some(proof) if !proof.is_empty() => format!("; proof: {proof}"),
                    _ => String::new(),
                };
                say(out, format!("- {}: {frames}{proof}", item.id))?;
            }
            Ok(0)
        }
        Command::Shoot => Err(refused(SHOTS_NOT_INSTALLED)),
    }
}

/// What is still missing, one line each, in the set's order.
pub fn gaps(data: &Manifest) -> Vec<String> {
    let mut out = Vec::new();
    for item in &data.items {
        match item.screen {
            None => out.push(format!("{} not marked screen yes or no", item.id)),
            Some(true) => {
                for when in [When::Before, When::After] {
                    if item.frame(when).is_none() {
                        out.push(format!("{} has no {} frame", item.id, when.shout()));
                    }
                }
            }
            Some(false) => {}
        }
        if !item.has_proof() && item.frame(When::After).is_none() {
            out.push(format!("{} has no proof", item.id));
        }
    }
    out
}

/// The `(id, text)` pairs of the sidecar's acceptance list, checked to be for `ticket`.
fn acceptance(expectations: &Path, ticket: &str) -> Result<Vec<(String, String)>, Refused> {
    let source: Value = std::fs::read(expectations)
        .map_err(|error| refused(format!("cannot read {}: {error}", expectations.display())))
        .and_then(|bytes| {
            serde_json::from_slice(&bytes)
                .map_err(|error| refused(format!("cannot read {}: {error}", expectations.display())))
        })?;
    let wanted: Vec<(String, String)> = source
        .get("acceptance")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|entry| {
                    let id = entry.get("id").and_then(Value::as_str).unwrap_or("").to_owned();
                    let text = entry.get("text").and_then(Value::as_str).unwrap_or("").to_owned();
                    (id, text)
                })
                .collect()
        })
        .unwrap_or_default();
    if wanted.is_empty() || wanted.iter().any(|(id, _)| !EID.is_match(id)) {
        return Err(refused(format!(
            "{} has no acceptance list of E ids (the expect sidecar)",
            expectations.display()
        )));
    }
    if let Some(theirs) = source.get("ticket").and_then(Value::as_str).filter(|t| !t.is_empty())
        && theirs != ticket
    {
        return Err(refused(format!(
            "{} is for {theirs}, not {ticket}",
            expectations.display()
        )));
    }
    Ok(wanted)
}

/// The file's extension with its dot, lower-cased, as `os.path.splitext` finds it; empty when none.
fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| format!(".{}", ext.to_lowercase()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use budd_core::reply::Evidence;

    /// The suite of evidence.test.sh: a set built step by step in a scratch config folder, read back
    /// from evidence.json.
    struct Bench {
        _temp: tempfile::TempDir,
        config: PathBuf,
        scratch: PathBuf,
    }

    impl Bench {
        fn new() -> Bench {
            let temp = tempfile::tempdir().unwrap();
            let config = temp.path().join("config");
            let scratch = temp.path().join("scratch");
            std::fs::create_dir_all(&scratch).unwrap();
            std::fs::write(
                scratch.join("exp.json"),
                r#"{"ticket":"CASBO-1","acceptance":[{"id":"E1","text":"Header reads Bet Date/Time"},{"id":"E2","text":"CSV header renamed"}]}"#,
            )
            .unwrap();
            std::fs::write(scratch.join("shot.png"), "png").unwrap();
            std::fs::write(scratch.join("shot.jpg"), "jpg").unwrap();
            std::fs::write(scratch.join("notes.txt"), "txt").unwrap();
            Bench {
                _temp: temp,
                config,
                scratch,
            }
        }

        /// `run()` as the shell's `run` wrapper: the exit code (2 on a refusal) and everything printed.
        fn run(&self, command: Command) -> (i32, String) {
            let mut out = Vec::new();
            let code = match run(&self.config, &command, &mut out) {
                Ok(code) => code,
                Err(why) => {
                    out.extend_from_slice(format!("evidence: {why}\n").as_bytes());
                    2
                }
            };
            (code, String::from_utf8(out).unwrap())
        }

        fn set(&self) -> PathBuf {
            self.config.join("evidence").join("CASBO-1")
        }

        fn json(&self) -> Value {
            serde_json::from_slice(&std::fs::read(self.set().join("evidence.json")).unwrap()).unwrap()
        }

        fn item(&self, index: usize) -> Value {
            self.json()["items"][index].clone()
        }

        fn path(&self, name: &str) -> PathBuf {
            self.scratch.join(name)
        }

        fn init(&self, ticket: &str, file: &str) -> (i32, String) {
            self.run(Command::Init {
                ticket: ticket.into(),
                expectations: self.path(file),
            })
        }

        fn check(&self, ticket: &str) -> (i32, String) {
            self.run(Command::Check { ticket: ticket.into() })
        }

        fn add(&self, id: &str, when: &str, file: &str, url: Option<&str>, steps: Option<&str>) -> (i32, String) {
            self.run(Command::Add {
                ticket: "CASBO-1".into(),
                id: id.into(),
                when: when.into(),
                image: self.path(file),
                url: url.map(str::to_owned),
                steps: steps.map(str::to_owned),
                recipe: None,
            })
        }

        fn proof(&self, ticket: &str, id: &str, text: &str) -> (i32, String) {
            self.run(Command::Proof {
                ticket: ticket.into(),
                id: id.into(),
                text: text.into(),
            })
        }
    }

    #[test]
    fn the_shell_suite_case_by_case() {
        let b = Bench::new();

        // init writes one item per expectation
        assert_eq!(b.init("CASBO-1", "exp.json").0, 0);
        assert_eq!(b.json()["items"][0]["id"], "E1");
        assert_eq!(b.json()["items"][1]["id"], "E2");
        assert_eq!(b.json()["items"].as_array().unwrap().len(), 2);

        // a fresh set has every gap
        let (code, out) = b.check("CASBO-1");
        assert_eq!(code, 1);
        assert!(out.contains("E1 not marked screen"), "{out}");
        assert!(out.contains("E2 has no proof"), "{out}");

        // add copies the frame in by id
        let (code, _) = b.add(
            "E1",
            "before",
            "shot.png",
            Some("http://localhost:20600/x"),
            Some("filter SPP-STG, Get a Report"),
        );
        assert_eq!(code, 0);
        assert!(b.set().join("E1-before.png").is_file());

        // add marks it on screen with a recipe
        assert_eq!(b.item(0)["screen"], true);
        assert_eq!(b.item(0)["url"], "http://localhost:20600/x");

        // half a pair is a gap
        let (code, out) = b.check("CASBO-1");
        assert_eq!(code, 1);
        assert!(out.contains("E1 has no AFTER frame"), "{out}");

        // after keeps its own extension
        assert_eq!(b.add("E1", "after", "shot.jpg", None, None).0, 0);
        assert!(b.set().join("E1-after.jpg").is_file());
        assert_eq!(b.item(0)["steps"], "filter SPP-STG, Get a Report");

        // re-adding with a new type drops the old file
        assert_eq!(b.add("E1", "after", "shot.png", None, None).0, 0);
        assert!(b.set().join("E1-after.png").is_file());
        assert!(!b.set().join("E1-after.jpg").exists());

        // no screen plus a proof is complete
        assert_eq!(
            b.run(Command::Screen {
                ticket: "CASBO-1".into(),
                id: "E2".into(),
                answer: "no".into(),
            })
            .0,
            0
        );
        assert_eq!(
            b.proof("CASBO-1", "E2", "unit-fe csvExport.spec.js asserts the 6th header")
                .0,
            0
        );
        let (code, out) = b.check("CASBO-1");
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("complete"));

        // show lists each expectation
        let (code, out) = b.run(Command::Show {
            ticket: "CASBO-1".into(),
        });
        assert_eq!(code, 0);
        assert!(out.contains("E1: before yes / after yes"), "{out}");
        assert!(out.contains("E2: no screen; proof: unit-fe"), "{out}");

        // re-init keeps frames and proofs
        std::fs::write(
            b.path("exp2.json"),
            r#"{"ticket":"CASBO-1","acceptance":[{"id":"E1","text":"Header reads Bet Date/Time exactly"},{"id":"E2","text":"CSV"},{"id":"E3","text":"new"}]}"#,
        )
        .unwrap();
        assert_eq!(b.init("CASBO-1", "exp2.json").0, 0);
        assert_eq!(b.item(0)["before"], "E1-before.png");
        assert!(b.item(0)["text"].as_str().unwrap().ends_with("exactly"));
        assert!(b.item(1)["proof"].as_str().is_some_and(|p| !p.is_empty()));
        assert_eq!(b.json()["items"].as_array().unwrap().len(), 3);

        // refuses a non-image
        assert_eq!(b.add("E1", "before", "notes.txt", None, None).0, 2);
        // refuses a missing file
        assert_eq!(b.add("E1", "before", "none.png", None, None).0, 2);
        // refuses an unknown id
        let (code, out) = b.proof("CASBO-1", "E9", "x");
        assert_eq!(code, 2);
        assert!(out.contains("has no E9"), "{out}");
        // refuses a bad ticket key
        assert_eq!(b.check("../etc").0, 2);
        // refuses another ticket's document
        assert_eq!(b.init("CASBO-2", "exp.json").0, 2);
        // check before init says how to start
        let (code, out) = b.check("CASBO-3");
        assert_eq!(code, 2);
        assert!(out.contains("evidence init CASBO-3"), "{out}");
    }

    #[test]
    fn the_office_reads_what_the_writer_wrote() {
        let b = Bench::new();
        b.init("CASBO-1", "exp.json");
        b.add("E1", "before", "shot.png", None, None);
        b.add("E1", "after", "shot.jpg", None, None);
        b.proof("CASBO-1", "E2", "spec");
        let bytes = std::fs::read(b.set().join("evidence.json")).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(
            text.starts_with("{\n \"ticket\": \"CASBO-1\",\n \"created\": \""),
            "{text}"
        );
        assert!(text.ends_with("}\n"));
        let read = Evidence::decode(&bytes, &b.set(), |path| path.is_file()).unwrap();
        assert_eq!(read.ticket, "CASBO-1");
        assert_eq!(read.title(), "Evidence 2/2");
        assert_eq!(read.gaps(), None);
        assert!(read.items[0].on_screen && read.items[0].before.is_some() && read.items[0].after.is_some());
        assert_eq!(read.items[1].proof.as_deref(), Some("spec"));
        assert_eq!(read.items[0].text, "Header reads Bet Date/Time");
    }

    #[test]
    fn a_recipe_is_kept_beside_the_frame_and_a_brief_key_is_a_ticket() {
        let b = Bench::new();
        std::fs::write(
            b.path("brief.json"),
            r#"{"acceptance":[{"id":"E1","text":"it works"}]}"#,
        )
        .unwrap();
        let key = "BRIEF-20261101-fix-the-thing";
        assert_eq!(b.init(key, "brief.json").0, 0);
        std::fs::write(b.path("E1.recipe.mjs"), "export default async () => {};").unwrap();
        let (code, out) = b.run(Command::Add {
            ticket: key.into(),
            id: "E1".into(),
            when: "before".into(),
            image: b.path("shot.png"),
            url: None,
            steps: None,
            recipe: Some(b.path("E1.recipe.mjs")),
        });
        assert_eq!(code, 0, "{out}");
        let folder = b.config.join("evidence").join(key);
        assert!(folder.join("E1.recipe.mjs").is_file());
        assert!(folder.join("E1-before.png").is_file());
        let (code, out) = b.run(Command::Shoot);
        assert_eq!(code, 2);
        assert!(out.contains(SHOTS_NOT_INSTALLED));
        for bad in ["casbo-1", "BRIEF-2026-x", "BRIEF-20261101-", "A-", "-1"] {
            assert_eq!(b.check(bad).0, 2, "{bad} must not be a ticket key");
        }
    }
}
