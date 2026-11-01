//! `budd handover`: the per-worktree handover document, from the pipeline's `handover.sh`.
//!
//! Several skills take a change from a ticket to an open PR, and the reasoning behind each step dies
//! with the session that did it. This keeps one Markdown file per worktree, `HANDOVER.md`, and each
//! skill writes its own section as its last step. Read it with `show` and you are caught up.
//!
//! WHERE IT LIVES, and why there: `<absolute git dir>/budd-handover/`, which for a linked worktree is
//! `<main repo>/.git/worktrees/<name>/budd-handover`. That is not working-tree content: `git status`
//! never scans it, `git add` cannot stage it, `git push` never sends it, and, the load-bearing one, it
//! cannot move the finalize fingerprint, so a section written between the seal and the commit does not
//! invalidate the seal it describes. What it does not survive is `git worktree remove`, which is how
//! you say the work is finished.
//!
//! A section is keyed by its heading (`## <skill> - <title>`, or `## <title>`): writing one with a
//! heading the document already has replaces that section in place, anything else is appended. A lock
//! file in the folder keeps two writers from interleaving. Every section carries the facts that tell a
//! reader whether it still describes the tree in front of them: the branch, the head, a marker of the
//! working tree, the seal, and the session's token usage.
//!
//! Fails closed: a repository it cannot resolve is an error, never an empty handover.

use super::{Utc, git};
use crate::hookrun::transcript_io::{context_tokens, locate_transcript};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt;
use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// The folder under the git dir.
pub const FOLDER: &str = "budd-handover";
/// The document's name in it.
pub const DOCUMENT: &str = "HANDOVER.md";
/// The writers' lock, beside the document.
pub const LOCK: &str = ".lock";
/// Where the finalize record is, under the git dir; its `fingerprint` is the `seal` fact.
pub const SEAL_RECORD: &str = "budd-finalize/record.json";
/// How long a writer waits for the lock before giving up.
const LOCK_WAIT: Duration = Duration::from_secs(10);
/// A lock older than this was left by a writer that died, and is taken over.
const LOCK_STALE: Duration = Duration::from_secs(60);

/// Why a verb failed. `NoDocument` exits 1, as `show` did; everything else exits 2.
#[derive(Debug)]
pub enum Error {
    /// The folder is not inside a git repository, or git is not there.
    NotARepository {
        /// The folder.
        path: PathBuf,
        /// What git said.
        detail: String,
    },
    /// `show` on a worktree with no handover yet.
    NoDocument {
        /// Where it would be.
        home: PathBuf,
    },
    /// `append` with nothing to say.
    EmptyBody,
    /// Another writer held the lock for too long.
    Locked {
        /// The lock file.
        path: PathBuf,
    },
    /// A file could not be read or written.
    Io {
        /// What was being done.
        what: &'static str,
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotARepository { path, detail } => {
                let detail = detail.trim();
                if detail.is_empty() {
                    write!(f, "not a git repository: {}", path.display())
                } else {
                    write!(f, "not a git repository: {} ({detail})", path.display())
                }
            }
            Error::NoDocument { home } => write!(f, "No handover for this worktree yet ({})", home.display()),
            Error::EmptyBody => f.write_str("append: empty body, a section that says nothing is worse than none"),
            Error::Locked { path } => write!(f, "another writer holds {}", path.display()),
            Error::Io { what, path, source } => write!(f, "{what} {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for Error {}

impl Error {
    /// The exit code the verb ends with.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::NoDocument { .. } => 1,
            _ => 2,
        }
    }
}

fn io_error(what: &'static str, path: &Path) -> impl FnOnce(io::Error) -> Error {
    let path = path.to_path_buf();
    move |source| Error::Io { what, path, source }
}

/// A section to write: its heading parts and its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSection<'a> {
    /// The skill that writes it, heading the section, or `None` for a plain title.
    pub skill: Option<&'a str>,
    /// The section's title.
    pub title: &'a str,
    /// The Markdown body.
    pub body: &'a str,
}

impl NewSection<'_> {
    /// `<skill> - <title>`, or the title alone: the text after `## `, which keys the section.
    pub fn heading(&self) -> String {
        match self.skill {
            Some(skill) => format!("{skill} - {}", self.title),
            None => self.title.to_owned(),
        }
    }
}

/// A session's token usage, as the facts table shows it. Every figure is grouped by `message.id`
/// first: a message is rewritten to the transcript as it streams, so a naive sum overstates the cost
/// by more than double. The numbers are CUMULATIVE for the session at the moment of writing, not a
/// per-section delta; deltas are deliberately not computed, since sections can be written from
/// different sessions in any order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenFacts {
    /// The session id as given, or empty.
    pub session: String,
    /// "<fresh> fresh + <w> cache write + <r> cache read", or why it is not recorded.
    pub input: String,
    /// "<out> (<thinking> thinking)", or "not recorded".
    pub output: String,
    /// "<n> messages, cumulative at this point", or why not.
    pub messages: String,
    /// What the session's last model call read, as the office counts it: "<n> tokens" or "not recorded".
    pub context: String,
}

impl TokenFacts {
    /// No session was named.
    pub fn none() -> TokenFacts {
        TokenFacts {
            session: String::new(),
            input: "not recorded (no --session passed)".to_owned(),
            output: "not recorded".to_owned(),
            messages: "-".to_owned(),
            context: "not recorded".to_owned(),
        }
    }

    fn missing(session: &str, why: &str) -> TokenFacts {
        TokenFacts {
            session: session.to_owned(),
            input: format!("not recorded ({why})"),
            output: "not recorded".to_owned(),
            messages: why.to_owned(),
            context: "not recorded".to_owned(),
        }
    }

    /// The first eight characters of the session id, as the facts table shows it.
    pub fn short_id(&self) -> String {
        self.session.chars().take(8).collect()
    }

    /// The lines `tokens` prints.
    pub fn lines(&self) -> String {
        format!(
            "in:      {}\nout:     {}\ncontext: {}\nsession: {} {}\n",
            self.input,
            self.output,
            self.context,
            self.short_id(),
            self.messages
        )
    }
}

/// The usage of the session `session`: its transcript is the path when `session` names a file, else the
/// newest `<config_dir>/projects/*/<session>.jsonl`. Never guesses: an unresolvable session says so.
pub fn token_facts(config_dir: &Path, session: &str, cwd: &str) -> TokenFacts {
    if session.is_empty() {
        return TokenFacts::none();
    }
    let given = Path::new(session);
    let transcript = if given.is_file() {
        given.to_path_buf()
    } else {
        let located = locate_transcript(config_dir, session, cwd);
        if !located.is_file() {
            return TokenFacts::missing(session, "transcript not found");
        }
        located
    };
    let Some(sum) = sum_usage(&transcript) else {
        return TokenFacts::missing(session, "transcript unreadable");
    };
    let context = match context_tokens(&transcript) {
        Some(tokens) => format!("{tokens} tokens"),
        None => "not recorded".to_owned(),
    };
    TokenFacts {
        session: session.to_owned(),
        input: format!(
            "{} fresh + {} cache write + {} cache read",
            sum.input, sum.cache_write, sum.cache_read
        ),
        output: format!("{} ({} thinking)", sum.output, sum.thinking),
        messages: format!("{} messages, cumulative at this point", sum.messages),
        context,
    }
}

/// The transcript's usage added up, one row per message id.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct UsageSum {
    messages: u64,
    input: u64,
    cache_write: u64,
    cache_read: u64,
    output: u64,
    thinking: u64,
}

fn sum_usage(transcript: &Path) -> Option<UsageSum> {
    let file = std::fs::File::open(transcript).ok()?;
    let mut seen: HashSet<String> = HashSet::new();
    let mut sum = UsageSum::default();
    let count = |value: Option<&Value>| value.and_then(Value::as_u64).unwrap_or(0);
    for line in BufReader::new(file).lines() {
        let line = line.ok()?;
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(usage) = entry
            .get("message")
            .and_then(|m| m.get("usage"))
            .filter(|u| u.is_object())
        else {
            continue;
        };
        let key = entry
            .get("message")
            .and_then(|m| m.get("id"))
            .or_else(|| entry.get("uuid"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if !seen.insert(key) {
            continue;
        }
        sum.messages += 1;
        sum.input += count(usage.get("input_tokens"));
        sum.cache_write += count(usage.get("cache_creation_input_tokens"));
        sum.cache_read += count(usage.get("cache_read_input_tokens"));
        sum.output += count(usage.get("output_tokens"));
        sum.thinking += count(
            usage
                .get("output_tokens_details")
                .and_then(|d| d.get("thinking_tokens")),
        );
    }
    Some(sum)
}

/// The facts every section carries. `branch` and `head` answer "which commit"; `tree` answers "and were
/// there uncommitted edits on top": a hash of `git status --porcelain=v1 -uall`, so it moves when a
/// tracked file is edited or an untracked file appears. It is deliberately NOT the finalize fingerprint;
/// a second copy of that algorithm would drift from the first and then lie. `seal` IS the real
/// fingerprint, read out of the sealed record when one exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// The branch, or "(detached)".
    pub branch: String,
    /// The short head, or "(no commits)".
    pub head: String,
    /// "<12 hex> (clean)" or "<12 hex> (dirty)".
    pub tree: String,
    /// The sealed fingerprint's first 12 characters, or "unsealed".
    pub seal: String,
}

/// A worktree's handover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handover {
    worktree: PathBuf,
    git_dir: PathBuf,
}

impl Handover {
    /// The handover of the repository `worktree` is in, resolved through `git rev-parse`.
    pub fn open(worktree: &Path) -> Result<Handover, Error> {
        let not_a_repo = |detail: String| Error::NotARepository {
            path: worktree.to_path_buf(),
            detail,
        };
        if !worktree.is_dir() {
            return Err(not_a_repo("not a directory".to_owned()));
        }
        let git_dir = git(worktree, ["rev-parse", "--absolute-git-dir"], &[]);
        let Some(git_dir) = git_dir.line().filter(|dir| !dir.is_empty()) else {
            return Err(not_a_repo(git_dir.stderr));
        };
        let toplevel = git(worktree, ["rev-parse", "--show-toplevel"], &[])
            .line()
            .filter(|top| !top.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| worktree.to_path_buf());
        Ok(Handover {
            worktree: toplevel,
            git_dir: PathBuf::from(git_dir),
        })
    }

    /// The worktree's top level.
    pub fn worktree(&self) -> &Path {
        &self.worktree
    }

    /// The handover folder: `<git dir>/budd-handover`.
    pub fn home(&self) -> PathBuf {
        self.git_dir.join(FOLDER)
    }

    /// The document: `<home>/HANDOVER.md`.
    pub fn document(&self) -> PathBuf {
        self.home().join(DOCUMENT)
    }

    /// The facts as they are now.
    pub fn facts(&self) -> Facts {
        let branch = git(&self.worktree, ["branch", "--show-current"], &[])
            .line()
            .filter(|branch| !branch.is_empty())
            .unwrap_or_else(|| "(detached)".to_owned());
        let head = git(&self.worktree, ["rev-parse", "--short", "HEAD"], &[])
            .line()
            .filter(|head| !head.is_empty())
            .unwrap_or_else(|| "(no commits)".to_owned());
        let status = git(&self.worktree, ["status", "--porcelain=v1", "-uall"], &[]);
        let tree = if status.ok {
            let digest = Sha256::digest(status.stdout.as_bytes());
            let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
            let state = if status.stdout.trim().is_empty() {
                "clean"
            } else {
                "dirty"
            };
            format!("{} ({state})", &hex[..12])
        } else {
            "(unknown)".to_owned()
        };
        let seal = std::fs::read(self.git_dir.join(SEAL_RECORD))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|record| record.get("fingerprint").and_then(Value::as_str).map(str::to_owned))
            .filter(|fingerprint| !fingerprint.is_empty())
            .map(|fingerprint| fingerprint.chars().take(12).collect())
            .unwrap_or_else(|| "unsealed".to_owned());
        Facts {
            branch,
            head,
            tree,
            seal,
        }
    }

    /// Writes `section` into the document, replacing the section with the same heading or appending a
    /// new one, under the lock, and gives the document's path. The document is started with its header
    /// when this is its first section.
    pub fn append(&self, section: &NewSection<'_>, tokens: &TokenFacts, now: &Utc) -> Result<PathBuf, Error> {
        if section.body.trim().is_empty() {
            return Err(Error::EmptyBody);
        }
        let home = self.home();
        std::fs::create_dir_all(&home).map_err(io_error("creating", &home))?;
        let _lock = Lock::take(&home.join(LOCK))?;
        let document = self.document();
        let facts = self.facts();
        let mut text = match std::fs::read_to_string(&document) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => self.header(&facts, now),
            Err(error) => {
                return Err(Error::Io {
                    what: "reading",
                    path: document,
                    source: error,
                });
            }
        };
        if !text.ends_with('\n') {
            text.push('\n');
        }
        let heading = section.heading();
        let block = render(&heading, &facts, tokens, now, section.body);
        let mut blocks = split_blocks(&text);
        match blocks
            .iter()
            .position(|existing| heading_of(existing) == Some(heading.as_str()))
        {
            Some(index) => blocks[index] = block,
            None => blocks.push(block),
        }
        let text = blocks.concat();
        crate::paths::write_atomically(&document, text.as_bytes()).map_err(io_error("writing", &document))?;
        Ok(document)
    }

    /// The document's first lines.
    fn header(&self, facts: &Facts, now: &Utc) -> String {
        format!(
            "# Handover - {}\n\nWorktree: `{}`\nStarted: {}\n\nEach section below was written by the skill \
             named in its heading, as that skill's last step.\nRead with: `budd handover show --worktree {}`\n\n",
            facts.branch,
            self.worktree.display(),
            now.stamp(),
            self.worktree.display()
        )
    }

    /// Prints the document to `out`, each section marked CURRENT when its facts still match the live
    /// tree and SUPERSEDED when they do not. With `only`, just the sections whose heading is it, or whose
    /// title after the skill is it.
    pub fn show(&self, only: Option<&str>, out: &mut dyn Write) -> Result<(), Error> {
        let document = self.document();
        let text = match std::fs::read_to_string(&document) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(Error::NoDocument { home: self.home() });
            }
            Err(error) => {
                return Err(Error::Io {
                    what: "reading",
                    path: document,
                    source: error,
                });
            }
        };
        let live = self.facts();
        let write = |out: &mut dyn Write, text: &str| {
            out.write_all(text.as_bytes())
                .map_err(io_error("writing", Path::new("stdout")))
        };
        write(
            out,
            &format!(
                "Live now:  branch={}  head={}  tree={}\nDocument:  {}\n\n",
                live.branch,
                live.head,
                live.tree,
                document.display()
            ),
        )?;
        for block in split_blocks(&text) {
            let Some(heading) = heading_of(&block) else {
                if only.is_none() {
                    write(out, &block)?;
                }
                continue;
            };
            if let Some(only) = only
                && heading != only
                && !heading.ends_with(&format!(" - {only}"))
            {
                continue;
            }
            let current = fact_in(&block, "branch") == Some(live.branch.as_str())
                && fact_in(&block, "head") == Some(live.head.as_str())
                && fact_in(&block, "tree") == Some(live.tree.as_str());
            let mark = if current { "CURRENT   " } else { "SUPERSEDED" };
            let body = block.split_once('\n').map(|(_, rest)| rest).unwrap_or("");
            write(out, &format!("[{mark}] ## {heading}\n{body}"))?;
        }
        Ok(())
    }
}

/// One section as the document holds it.
fn render(heading: &str, facts: &Facts, tokens: &TokenFacts, now: &Utc, body: &str) -> String {
    format!(
        "\n## {heading}\n\n| fact | value |\n| --- | --- |\n| when | {} |\n| branch | {} |\n| head | {} |\n\
         | tree | {} |\n| seal | {} |\n| tokens in | {} |\n| tokens out | {} |\n| context | {} |\n\
         | session | {} {} |\n\n{}\n",
        now.stamp(),
        facts.branch,
        facts.head,
        facts.tree,
        facts.seal,
        tokens.input,
        tokens.output,
        tokens.context,
        tokens.short_id(),
        tokens.messages,
        body.trim_end_matches('\n')
    )
}

/// The document as blocks that concatenate back to it: the preamble (possibly empty), then one block per
/// `## ` heading, each starting at its heading line and running to just before the next. A section's
/// leading blank line therefore belongs to the block before it.
fn split_blocks(text: &str) -> Vec<String> {
    let mut blocks = vec![String::new()];
    for line in text.split_inclusive('\n') {
        if line.starts_with("## ") {
            blocks.push(String::new());
        }
        if let Some(last) = blocks.last_mut() {
            last.push_str(line);
        }
    }
    blocks
}

/// The heading text of a section block, or `None` for the preamble.
fn heading_of(block: &str) -> Option<&str> {
    let first = block.lines().next()?;
    first.strip_prefix("## ").map(str::trim_end)
}

/// The value of `| <name> | <value> |` in a section's facts table.
fn fact_in<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("| {name} | ");
    block
        .lines()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
        .and_then(|rest| rest.strip_suffix(" |"))
}

/// The writers' lock: a file created exclusively, removed on drop. A lock left behind by a writer that
/// died is taken over once it is old enough.
struct Lock {
    path: PathBuf,
}

impl Lock {
    fn take(path: &Path) -> Result<Lock, Error> {
        let started = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}", std::process::id());
                    return Ok(Lock {
                        path: path.to_path_buf(),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let age = std::fs::metadata(path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|at| SystemTime::now().duration_since(at).ok());
                    if age.is_some_and(|age| age > LOCK_STALE) {
                        let _ = std::fs::remove_file(path);
                        continue;
                    }
                    if started.elapsed() > LOCK_WAIT {
                        return Err(Error::Locked {
                            path: path.to_path_buf(),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(source) => {
                    return Err(Error::Io {
                        what: "locking",
                        path: path.to_path_buf(),
                        source,
                    });
                }
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::fingerprint::tests::repo;

    fn section<'a>(title: &'a str, body: &'a str) -> NewSection<'a> {
        NewSection {
            skill: Some("do-it"),
            title,
            body,
        }
    }

    fn shown(handover: &Handover, only: Option<&str>) -> Result<String, Error> {
        let mut out = Vec::new();
        handover.show(only, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn sections_are_appended_replaced_by_heading_and_marked_against_the_live_tree() {
        let Some((_temp, root)) = repo() else { return };
        let handover = Handover::open(&root).unwrap();
        assert_eq!(handover.home(), root.join(".git").join(FOLDER));
        assert_eq!(handover.worktree(), root.as_path());
        // Opening from a folder inside the worktree finds the same handover.
        std::fs::create_dir_all(root.join("src")).unwrap();
        assert_eq!(Handover::open(&root.join("src")).unwrap(), handover);

        assert!(matches!(shown(&handover, None).unwrap_err(), Error::NoDocument { .. }));
        assert_eq!(shown(&handover, None).unwrap_err().exit_code(), 1);
        assert!(matches!(
            handover.append(&section("cause", "  \n"), &TokenFacts::none(), &Utc::from_unix(0)),
            Err(Error::EmptyBody)
        ));

        let now = Utc::from_unix(1_793_525_405);
        let doc = handover
            .append(
                &section("cause", "Cause `a.txt:1` - wrong.\n"),
                &TokenFacts::none(),
                &now,
            )
            .unwrap();
        assert_eq!(doc, handover.document());
        let text = std::fs::read_to_string(&doc).unwrap();
        assert!(text.starts_with("# Handover - main\n\nWorktree: `"), "{text}");
        assert!(text.contains("Started: 2026-11-01 09:30 +0000\n"));
        assert!(text.contains("\n## do-it - cause\n\n| fact | value |\n| --- | --- |\n| when | 2026-11-01 09:30 +0000 |\n| branch | main |\n| head | "));
        assert!(text.contains("| seal | unsealed |\n| tokens in | not recorded (no --session passed) |\n| tokens out | not recorded |\n| context | not recorded |\n| session |  - |\n\nCause `a.txt:1` - wrong.\n"), "{text}");
        assert!(text.contains("(clean) |"));

        // The same heading again replaces the section; a new heading is appended after it.
        handover
            .append(
                &section("cause", "Cause `a.txt:2` - righter."),
                &TokenFacts::none(),
                &now,
            )
            .unwrap();
        handover
            .append(
                &NewSection {
                    skill: None,
                    title: "expectations",
                    body: "Problems:\n- x\n",
                },
                &TokenFacts::none(),
                &now,
            )
            .unwrap();
        let text = std::fs::read_to_string(&doc).unwrap();
        assert_eq!(text.matches("\n## do-it - cause\n").count(), 1, "{text}");
        assert!(!text.contains("a.txt:1"));
        assert!(text.contains("a.txt:2"));
        let cause_at = text.find("## do-it - cause").unwrap();
        let expectations_at = text.find("## expectations").unwrap();
        assert!(cause_at < expectations_at);
        assert!(text.ends_with("\n\nProblems:\n- x\n"), "{text:?}");
        let names: Vec<String> = std::fs::read_dir(handover.home())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [DOCUMENT], "the lock is gone and no temporary file is left");

        // Everything is current on an unchanged tree.
        let out = shown(&handover, None).unwrap();
        assert!(out.starts_with("Live now:  branch=main  head="), "{out}");
        assert!(out.contains("\nDocument:  "));
        assert!(out.contains("# Handover - main\n"), "the preamble is printed");
        assert!(out.contains("[CURRENT   ] ## do-it - cause\n"), "{out}");
        assert!(out.contains("[CURRENT   ] ## expectations\n"), "{out}");
        // A filter prints only that section, by title or by full heading.
        let out = shown(&handover, Some("cause")).unwrap();
        assert!(out.contains("## do-it - cause") && !out.contains("## expectations") && !out.contains("# Handover"));
        assert!(shown(&handover, Some("do-it - cause")).unwrap().contains("a.txt:2"));
        assert!(!shown(&handover, Some("nothing")).unwrap().contains("## "));

        // An edit to the tree supersedes every section written before it; a new one is current.
        std::fs::write(root.join("a.txt"), "edited\n").unwrap();
        let out = shown(&handover, None).unwrap();
        assert!(out.contains("[SUPERSEDED] ## do-it - cause\n"), "{out}");
        assert!(out.contains("[SUPERSEDED] ## expectations\n"));
        assert!(out.contains("(dirty)"));
        handover
            .append(&section("built", "done"), &TokenFacts::none(), &now)
            .unwrap();
        let out = shown(&handover, None).unwrap();
        assert!(
            out.contains("[CURRENT   ] ## built\n") || out.contains("[CURRENT   ] ## do-it - built\n"),
            "{out}"
        );
        assert!(out.contains("[SUPERSEDED] ## do-it - cause\n"));
    }

    #[test]
    fn a_linked_worktree_keeps_its_handover_under_the_main_git_dir_and_the_seal_is_read() {
        let Some((temp, root)) = repo() else { return };
        let linked = temp.path().join("linked");
        let added = git(
            &root,
            [
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/x",
                linked.to_string_lossy().as_ref(),
            ],
            &[],
        );
        assert!(added.ok, "{}", added.stderr);
        let handover = Handover::open(&linked).unwrap();
        let home = handover.home();
        assert!(
            home.starts_with(root.join(".git").join("worktrees")),
            "{}",
            home.display()
        );
        assert!(home.ends_with(FOLDER));
        assert_eq!(handover.facts().seal, "unsealed");
        let record = home.parent().unwrap().join(SEAL_RECORD);
        std::fs::create_dir_all(record.parent().unwrap()).unwrap();
        std::fs::write(&record, r#"{"fingerprint":"0123456789abcdef0123"}"#).unwrap();
        let facts = handover.facts();
        assert_eq!(facts.seal, "0123456789ab");
        assert_eq!(facts.branch, "feat/x");
        assert_eq!(facts.tree.len(), 12 + " (clean)".len());
    }

    #[test]
    fn a_folder_outside_git_is_refused_and_a_held_lock_is_waited_for() {
        let temp = tempfile::tempdir().unwrap();
        let error = Handover::open(temp.path()).unwrap_err();
        assert!(matches!(error, Error::NotARepository { .. }));
        assert_eq!(error.exit_code(), 2);
        assert!(error.to_string().starts_with("not a git repository: "));
        assert!(matches!(
            Handover::open(&temp.path().join("missing")).unwrap_err(),
            Error::NotARepository { .. }
        ));

        // A lock another writer holds is taken once it lets go.
        let lock_path = temp.path().join(LOCK);
        let held = Lock::take(&lock_path).unwrap();
        let path = lock_path.clone();
        let waiter = std::thread::spawn(move || Lock::take(&path).map(|_| ()).is_ok());
        std::thread::sleep(Duration::from_millis(100));
        assert!(lock_path.exists());
        drop(held);
        assert!(waiter.join().unwrap());
        assert!(!lock_path.exists(), "the lock is removed when its holder is done");
    }

    #[test]
    fn tokens_are_summed_once_per_message_and_the_context_is_the_last_call() {
        let temp = tempfile::tempdir().unwrap();
        let config = temp.path().join("config");
        let project = config.join("projects").join("-Users-x-repo");
        std::fs::create_dir_all(&project).unwrap();
        let transcript = project.join("abcdef12-3456-7890-abcd-ef1234567890.jsonl");
        std::fs::write(
            &transcript,
            concat!(
                "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n",
                "{\"type\":\"assistant\",\"uuid\":\"u1\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":10,\"cache_creation_input_tokens\":100,\"cache_read_input_tokens\":1000,\"output_tokens\":5,\"output_tokens_details\":{\"thinking_tokens\":2}}}}\n",
                "{\"type\":\"assistant\",\"uuid\":\"u2\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":10,\"cache_creation_input_tokens\":100,\"cache_read_input_tokens\":1000,\"output_tokens\":50,\"output_tokens_details\":{\"thinking_tokens\":20}}}}\n",
                "{\"type\":\"assistant\",\"uuid\":\"u3\",\"message\":{\"id\":\"m2\",\"usage\":{\"input_tokens\":1,\"cache_read_input_tokens\":2000,\"output_tokens\":7}}}\n",
                "{\"type\":\"assistant\",\"uuid\":\"u4\",\"message\":{\"usage\":{\"input_tokens\":3}}}\n",
                "not json with \"usage\" in it\n",
            ),
        )
        .unwrap();
        let facts = token_facts(&config, "abcdef12-3456-7890-abcd-ef1234567890", "/Users/x/repo");
        assert_eq!(facts.input, "14 fresh + 100 cache write + 3000 cache read");
        assert_eq!(facts.output, "12 (2 thinking)");
        assert_eq!(facts.messages, "3 messages, cumulative at this point");
        assert_eq!(facts.context, "3 tokens");
        assert_eq!(facts.short_id(), "abcdef12");
        assert_eq!(
            facts.lines(),
            "in:      14 fresh + 100 cache write + 3000 cache read\nout:     12 (2 thinking)\ncontext: 3 tokens\nsession: abcdef12 3 messages, cumulative at this point\n"
        );
        // The transcript by path reads the same.
        let by_path = token_facts(&config, transcript.to_string_lossy().as_ref(), "/elsewhere");
        assert_eq!(by_path.input, facts.input);
        // Unknown sessions say so; no session says so.
        let missing = token_facts(&config, "nope", "/Users/x/repo");
        assert_eq!(missing.input, "not recorded (transcript not found)");
        assert_eq!(missing.output, "not recorded");
        assert_eq!(missing.messages, "transcript not found");
        assert_eq!(token_facts(&config, "", "/"), TokenFacts::none());
    }
}
