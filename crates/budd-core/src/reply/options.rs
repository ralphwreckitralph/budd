//! The answers a session offers when it stops for the human, as buttons.
//!
//! A session that waits on a manual step ends its reply with one line naming what it expects back:
//!
//! ```text
//! Reply: `tab is up` or `skip`
//! ```
//!
//! It reads naturally in the terminal, and Budd turns each quoted answer into a button that sends
//! it. Nothing is guessed: a reply without that line offers no buttons.
//!
//! The line says who may answer. `Reply:` is the human's: a login, a merge, an approval, anything that
//! needs their hands or their say. `Decide:` is a call any Claude with the context could make, with the
//! recommended answer first, so autopilot may answer it with that one. Answering is opt-in that way:
//! a session that only knows `Reply:` is never answered for the human.

use regex::Regex;
use std::sync::LazyLock;
use unicode_segmentation::UnicodeSegmentation;

static LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s*\**(reply|decide)\**:\**\s+(.+)$").unwrap());
static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"`([^`\n]+)`").unwrap());

/// Who may answer the line: the human (`Reply:`) or any Claude with the context (`Decide:`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    /// The human's to answer.
    #[default]
    Reply,
    /// A call autopilot may make with the first answer.
    Decide,
}

/// The answers a reply's last line offers, and the reply without that line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyOptions {
    /// The answers, in the order written.
    pub answers: Vec<String>,
    /// Who may answer.
    pub kind: Kind,
    /// The reply without its Reply line, for showing next to the buttons.
    pub body: String,
}

impl ReplyOptions {
    /// At most this many buttons, so they fit on one row.
    pub const MOST: usize = 4;
    /// Each button at most this many characters (grapheme clusters), so they fit on one row.
    pub const LONGEST: usize = 24;

    /// The options in `reply`, or `None` when its last line offers none.
    pub fn parse(reply: &str) -> Option<ReplyOptions> {
        let mut lines: Vec<&str> = reply.split('\n').collect();
        while lines
            .last()
            .is_some_and(|last| last.trim_matches([' ', '\t']).is_empty())
        {
            lines.pop();
        }
        let last = *lines.last()?;
        let captures = LINE.captures(last)?;
        let mut answers: Vec<String> = Vec::new();
        for quoted in QUOTED.captures_iter(&captures[2]) {
            let answer = quoted[1].trim_matches([' ', '\t']);
            if answer.is_empty() || answer.graphemes(true).count() > Self::LONGEST {
                continue;
            }
            if answers.iter().any(|known| known == answer) {
                continue;
            }
            answers.push(answer.to_owned());
        }
        if answers.is_empty() {
            return None;
        }
        answers.truncate(Self::MOST);
        let body = lines[..lines.len() - 1].join("\n").trim().to_owned();
        let kind = if captures[1].eq_ignore_ascii_case("decide") {
            Kind::Decide
        } else {
            Kind::Reply
        };
        Some(ReplyOptions { answers, kind, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answers(reply: &str) -> Option<Vec<String>> {
        ReplyOptions::parse(reply).map(|options| options.answers)
    }

    #[test]
    fn the_last_line_offers_the_answers() {
        let options = ReplyOptions::parse("Open the tab and log in.\n\nReply: `tab is up` or `skip`\n");
        assert_eq!(
            options,
            Some(ReplyOptions {
                answers: vec!["tab is up".into(), "skip".into()],
                kind: Kind::Reply,
                body: "Open the tab and log in.".into(),
            })
        );
        assert_eq!(
            answers("Done.\n**Reply:** `approve`, `revise` or `stop`"),
            Some(vec!["approve".into(), "revise".into(), "stop".into()])
        );
    }

    #[test]
    fn a_decide_line_is_one_claude_may_answer() {
        let decide = ReplyOptions::parse("Two layouts fit.\nDecide: `compact` or `roomy`").unwrap();
        assert_eq!(decide.kind, Kind::Decide);
        assert_eq!(decide.answers, vec!["compact", "roomy"]);
        assert_eq!(
            ReplyOptions::parse("PR #259 is green.\nReply: `marked ready` or `not yet`").map(|o| o.kind),
            Some(Kind::Reply)
        );
    }

    #[test]
    fn nothing_is_guessed() {
        assert_eq!(ReplyOptions::parse("Reply: `tab is up`\nthen I carry on."), None);
        assert_eq!(ReplyOptions::parse("Please reply when the tab is up."), None);
        assert_eq!(ReplyOptions::parse("Reply: whenever you like"), None);
    }

    #[test]
    fn keeps_the_buttons_to_one_row() {
        let options = ReplyOptions::parse("Reply: `a` `b` `a` `this answer is far too long to fit` `c` `d` `e`");
        assert_eq!(options.unwrap().answers, vec!["a", "b", "c", "d"]);
    }
}
