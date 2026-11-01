//! `budd expect`: the shape of the one document `/budd:do-it` writes before it builds, from the
//! pipeline's `expectation-shape.mjs`, rule for rule and message for message.
//!
//! The document is exactly this, and nothing else on the page:
//!
//! ```text
//! Problems:
//! - one sentence, 300 characters at most
//! Expected:
//! - one sentence, 300 characters at most: what is true when the work is done
//! Potential risk:
//! - one sentence
//! ```
//!
//! The three headings come in that order, once each. Every bullet is ONE line, so a bullet that wraps
//! onto a second line is refused rather than guessed at. Problems and Expected take as many bullets as
//! the ticket needs, at least one; Potential risk takes exactly one. The 300-character cap applies to
//! the risk line too. A sentence is counted as `style-check.mjs` counted it, so "e.g." and a
//! `path/with.dots` never split one. An em dash is refused.
//!
//! `sidecar` also writes the acceptance file `budd finalize answers` and `budd evidence init` read,
//! `{ ticket, source, acceptance: [{ id, text }] }`, one item per Expected bullet as E1..En. Nothing is
//! written for a failing document.

use fancy_regex::Regex as FancyRegex;
use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;

/// The most characters a bullet may have.
pub const MAX_BULLET_CHARS: usize = 300;

/// Where the sidecar says it came from.
pub const SIDECAR_SOURCE: &str = "budd:do-it expectation document";

/// The three headings, in order.
const SECTIONS: [Section; 3] = [
    Section {
        heading: "Problems:",
        single: false,
    },
    Section {
        heading: "Expected:",
        single: false,
    },
    Section {
        heading: "Potential risk:",
        single: true,
    },
];

#[derive(Debug, Clone, Copy)]
struct Section {
    heading: &'static str,
    single: bool,
}

/// What the check found: an empty `errors` is the only pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Validation {
    /// One line per fault, in the document's order, then the per-section counts.
    pub errors: Vec<String>,
    /// The Problems bullets.
    pub problems: Vec<String>,
    /// The Expected bullets.
    pub expected: Vec<String>,
    /// The Potential risk bullets.
    pub risk: Vec<String>,
}

impl Validation {
    /// Whether the document passes.
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// The `ok:` line printed for a passing document.
    pub fn summary(&self) -> String {
        format!(
            "ok: {} problems, {} expected, 1 risk",
            self.problems.len(),
            self.expected.len()
        )
    }

    /// The finalize sidecar: every Expected bullet becomes an acceptance item, numbered in order.
    pub fn sidecar(&self, ticket: Option<&str>) -> Value {
        json!({
            "ticket": ticket,
            "source": SIDECAR_SOURCE,
            "acceptance": self
                .expected
                .iter()
                .enumerate()
                .map(|(i, text)| json!({"id": format!("E{}", i + 1), "text": text}))
                .collect::<Vec<_>>(),
        })
    }
}

fn plain(pattern: &'static str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|error| panic!("expect pattern {pattern:?} does not compile: {error}"))
}

static BULLET: LazyLock<Regex> = LazyLock::new(|| plain(r"^-(\s|$)"));
static CODE_SPAN: LazyLock<Regex> = LazyLock::new(|| plain(r"`[^`]*`"));
static EMPHASIS: LazyLock<Regex> = LazyLock::new(|| plain(r"\*\*|__"));
/// "e.g.", "i.e.", "vs." and "etc." carry a full stop that ends nothing.
static ABBREVIATION: LazyLock<Regex> = LazyLock::new(|| plain(r"(?i)\b(e\.g|i\.e|vs|etc|approx|incl)\."));
static LEADING_MARKS: LazyLock<Regex> = LazyLock::new(|| plain(r"^[\s>*_#`-]+"));
/// A sentence ends at a full stop, a question mark or an exclamation mark followed by space, or at
/// a line break.
static SENTENCE_END: LazyLock<FancyRegex> = LazyLock::new(|| {
    FancyRegex::new(r"(?<=[.!?])\s+|\n+").unwrap_or_else(|error| panic!("expect pattern does not compile: {error}"))
});

/// A code span is one token to a reader, however long it is, and emphasis marks hide the full stop
/// before them: both come out before anything is counted.
fn countable(text: &str) -> String {
    let no_spans = CODE_SPAN.replace_all(text, "X");
    EMPHASIS.replace_all(&no_spans, "").into_owned()
}

/// How many sentences `text` holds, as `style-check.mjs` counts them.
pub fn sentence_count(text: &str) -> usize {
    let text = countable(text);
    let text = ABBREVIATION.replace_all(&text, "$1");
    let mut pieces = Vec::new();
    let mut last = 0;
    for found in SENTENCE_END.find_iter(&text).flatten() {
        pieces.push(&text[last..found.start()]);
        last = found.end();
    }
    pieces.push(&text[last..]);
    pieces
        .into_iter()
        .map(|piece| LEADING_MARKS.replace(piece, "").trim().to_owned())
        .filter(|piece| !piece.is_empty())
        .count()
}

/// Parses the document and checks every rule.
pub fn validate(text: &str) -> Validation {
    let mut found = Validation::default();
    let mut at: Option<usize> = None;
    let heading_list = SECTIONS
        .iter()
        .map(|s| format!("\"{}\"", s.heading))
        .collect::<Vec<_>>()
        .join(", ");

    for (i, raw) in text.split('\n').enumerate() {
        let line = raw.trim();
        let where_ = format!("line {}", i + 1);
        if line.is_empty() {
            continue;
        }
        if let Some(next) = SECTIONS.iter().position(|s| s.heading == line) {
            let expected = at.map_or(0, |at| at + 1);
            if next != expected {
                found.errors.push(format!(
                    "{where_}: \"{line}\" is out of order; the headings are {heading_list}, once each, in that order"
                ));
            }
            at = Some(next);
            continue;
        }
        if !BULLET.is_match(line) {
            let head: String = line.chars().take(40).collect();
            found.errors.push(format!(
                "{where_}: \"{head}\" is neither a heading nor a \"- \" bullet; one bullet is one line"
            ));
            continue;
        }
        let Some(section) = at else {
            found
                .errors
                .push(format!("{where_}: a bullet before the \"Problems:\" heading"));
            continue;
        };
        let heading = SECTIONS[section].heading;
        let body = line[1..].trim();
        let label = format!("{where_} ({})", heading.replace(':', ""));
        match section {
            0 => found.problems.push(body.to_owned()),
            1 => found.expected.push(body.to_owned()),
            _ => found.risk.push(body.to_owned()),
        }
        if body.is_empty() {
            found.errors.push(format!("{label}: the bullet is empty"));
        }
        let chars = body.chars().count();
        if chars > MAX_BULLET_CHARS {
            found
                .errors
                .push(format!("{label}: {chars} characters; the cap is {MAX_BULLET_CHARS}"));
        }
        let sentences = sentence_count(body);
        if sentences > 1 {
            found
                .errors
                .push(format!("{label}: {sentences} sentences; a bullet is ONE sentence"));
        }
        if body.contains('\u{2014}') {
            found.errors.push(format!("{label}: an em dash; use a plain \"-\""));
        }
    }

    for (index, section) in SECTIONS.iter().enumerate() {
        let n = match index {
            0 => found.problems.len(),
            1 => found.expected.len(),
            _ => found.risk.len(),
        };
        if n == 0 {
            found
                .errors
                .push(format!("\"{}\" needs at least one bullet", section.heading));
        } else if section.single && n > 1 {
            found.errors.push(format!(
                "\"{}\" has {n} bullets; it is exactly one sentence",
                section.heading
            ));
        }
    }
    found
}

/// The sidecar as a file: two-space indent and a final newline.
pub fn encode_sidecar(sidecar: &Value) -> String {
    let mut text = serde_json::to_string_pretty(sidecar).unwrap_or_else(|_| sidecar.to_string());
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "Problems:
- The report header reads `ReqDateTime` where the ticket asks for Bet Date/Time.
- The CSV export carries the same header.
Expected:
- The header reads Bet Date/Time, e.g. on the SPP-STG report.
- The CSV header reads Bet Date/Time too.
Potential risk:
- The companion app reads the same column name (see `src/report.js`).
";

    #[test]
    fn the_documented_shape_passes_and_makes_the_sidecar() {
        let found = validate(GOOD);
        assert_eq!(found.errors, Vec::<String>::new());
        assert_eq!(found.summary(), "ok: 2 problems, 2 expected, 1 risk");
        let sidecar = found.sidecar(Some("CASBO-1"));
        assert_eq!(sidecar["ticket"], "CASBO-1");
        assert_eq!(sidecar["source"], SIDECAR_SOURCE);
        assert_eq!(sidecar["acceptance"][0]["id"], "E1");
        assert_eq!(sidecar["acceptance"][1]["id"], "E2");
        assert_eq!(
            sidecar["acceptance"][1]["text"],
            "The CSV header reads Bet Date/Time too."
        );
        assert_eq!(sidecar["acceptance"].as_array().unwrap().len(), 2);
        assert!(encode_sidecar(&sidecar).ends_with("}\n"));
        assert_eq!(found.sidecar(None)["ticket"], Value::Null);
    }

    #[test]
    fn every_rule_speaks_in_the_scripts_words() {
        let text = "Intro line\nExpected:\n- fine\nProblems:\n-\n- Two sentences. Here they are.\n- Em \u{2014} dash\nPotential risk:\n- a\n- b\n";
        let found = validate(text);
        assert_eq!(
            found.errors,
            vec![
                "line 1: \"Intro line\" is neither a heading nor a \"- \" bullet; one bullet is one line",
                "line 2: \"Expected:\" is out of order; the headings are \"Problems:\", \"Expected:\", \"Potential risk:\", once each, in that order",
                "line 4: \"Problems:\" is out of order; the headings are \"Problems:\", \"Expected:\", \"Potential risk:\", once each, in that order",
                "line 5 (Problems): the bullet is empty",
                "line 6 (Problems): 2 sentences; a bullet is ONE sentence",
                "line 7 (Problems): an em dash; use a plain \"-\"",
                "line 8: \"Potential risk:\" is out of order; the headings are \"Problems:\", \"Expected:\", \"Potential risk:\", once each, in that order",
                "\"Potential risk:\" has 2 bullets; it is exactly one sentence",
            ]
        );
        let found = validate("- early\n");
        assert_eq!(
            found.errors,
            vec![
                "line 1: a bullet before the \"Problems:\" heading",
                "\"Problems:\" needs at least one bullet",
                "\"Expected:\" needs at least one bullet",
                "\"Potential risk:\" needs at least one bullet",
            ]
        );
        let long = format!(
            "Problems:\n- {}\nExpected:\n- x\nPotential risk:\n- y\n",
            "a".repeat(301)
        );
        assert_eq!(
            validate(&long).errors,
            vec!["line 2 (Problems): 301 characters; the cap is 300"]
        );
        // A long line of non-ASCII counts code points, as the script's spread did.
        let long = format!(
            "Problems:\n- {}\nExpected:\n- x\nPotential risk:\n- y\n",
            "é".repeat(300)
        );
        assert!(validate(&long).ok());
        // A line that is not a bullet is cut to 40 characters in the message.
        let found = validate(&format!("Problems:\n{}\n", "x".repeat(60)));
        assert!(found.errors[0].starts_with(&format!("line 2: \"{}\" is neither", "x".repeat(40))));
        // A bullet with nothing after the dash is still a bullet; a dash glued to a word is not.
        assert!(
            validate("Problems:\n-\n")
                .errors
                .contains(&"line 2 (Problems): the bullet is empty".to_owned())
        );
        assert!(
            validate("Problems:\n-x\n")
                .errors
                .iter()
                .any(|e| e.contains("neither a heading nor"))
        );
        assert!(validate("").errors.len() == 3);
    }

    #[test]
    fn sentences_are_counted_as_style_check_counts_them() {
        assert_eq!(sentence_count("One sentence."), 1);
        assert_eq!(sentence_count("One. Two."), 2);
        assert_eq!(sentence_count("One! Two? Three"), 3);
        assert_eq!(sentence_count("Set the filter, e.g. SPP-STG, then search."), 1);
        assert_eq!(sentence_count("See vs. the other one, etc. and i.e. this."), 1);
        assert_eq!(sentence_count("The file `src/a.b.c` is read. Then it is parsed."), 2);
        assert_eq!(sentence_count("The file `src/a.b.c` is read here."), 1);
        assert_eq!(sentence_count("**Bold one.** **Bold two.**"), 2);
        assert_eq!(sentence_count("Problem: the total is wrong on the page."), 1);
        assert_eq!(sentence_count("line one\nline two"), 2);
        assert_eq!(sentence_count("Ends with a dot.  "), 1);
        assert_eq!(sentence_count("v1.2.3 is the version"), 1);
        assert_eq!(sentence_count(""), 0);
        assert_eq!(sentence_count("- just marks - "), 1);
    }
}
