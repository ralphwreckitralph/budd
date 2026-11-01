//! Who answers the crew's asks.
//!
//! In `Human` mode every ask waits for the human, as it always has. In `Autopilot` Budd answers a
//! crew session's `Decide:` line by itself with the answer the session recommends (the first), and
//! still calls the human over for everything else: a `Reply:` line, a permission prompt, or an ask with
//! no answers to choose from. Neither mode ever answers a permission prompt or a `Reply:` line.

use super::options::{Kind, ReplyOptions};
use crate::fingerprint::fingerprint;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

/// Who answers the crew's asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PilotMode {
    /// Every ask waits for the human.
    #[default]
    Human,
    /// Budd answers `Decide:` lines with the recommended answer.
    Autopilot,
}

impl PilotMode {
    /// Every mode, in order.
    pub const ALL: [PilotMode; 2] = [PilotMode::Human, PilotMode::Autopilot];

    /// The mode's name as it is saved: "human" or "autopilot".
    pub fn name(self) -> &'static str {
        match self {
            PilotMode::Human => "human",
            PilotMode::Autopilot => "autopilot",
        }
    }

    /// The mode saved under `name`, or `None` for any other name.
    pub fn from_name(name: &str) -> Option<PilotMode> {
        Self::ALL.into_iter().find(|mode| mode.name() == name)
    }
}

/// One answer autopilot gave: to which reply (its fingerprint), what, and when.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    /// The fingerprint of the reply answered.
    pub reply: String,
    /// The answer sent.
    pub answer: String,
    /// When it was sent, wall clock seconds.
    pub at: f64,
    /// It could not be confirmed as sent, so the ask is the human's again.
    pub failed: bool,
}

/// The fingerprint of a reply, with no reply fingerprinting as the empty string, as the Swift app did.
fn reply_fingerprint(reply: Option<&str>) -> String {
    fingerprint(reply.unwrap_or(""))
}

/// What autopilot has answered, kept as plain values across launches like crew attention, so a
/// restart never answers the same ask twice.
#[derive(Debug, Clone, Default)]
pub struct Autopilot {
    /// The last answer given to each session, by session id.
    answers: HashMap<String, Answer>,
    /// When each session's current ask was first seen (wall clock), and which reply it was.
    pending: HashMap<String, (String, f64)>,
}

impl PartialEq for Autopilot {
    fn eq(&self, other: &Self) -> bool {
        self.answers == other.answers
    }
}

impl Autopilot {
    /// How long an ask stands before autopilot answers it, so the human sees it coming and can step in.
    pub const GRACE: f64 = 8.0;
    /// What goes in front of an answer, so the session knows Budd gave it, not the human.
    pub const PREFIX: &'static str = "Autopilot: ";

    /// Autopilot with these answers already given.
    pub fn with_answers(answers: HashMap<String, Answer>) -> Self {
        Autopilot {
            answers,
            pending: HashMap::new(),
        }
    }

    /// The last answer given to each session, by session id.
    pub fn answers(&self) -> &HashMap<String, Answer> {
        &self.answers
    }

    /// The answer autopilot would give `reply`, or `None` when it is not autopilot's to give.
    pub fn answer(reply: Option<&str>) -> Option<String> {
        let options = ReplyOptions::parse(reply?)?;
        if options.kind != Kind::Decide {
            return None;
        }
        options.answers.into_iter().next()
    }

    /// Whether autopilot is handling this ask, so Budd does not call the human over for it: it has
    /// an answer to give, and has not failed to give it.
    pub fn handles(&self, session: &str, reply: Option<&str>) -> bool {
        if Self::answer(reply).is_none() {
            return false;
        }
        match self.answers.get(session) {
            Some(given) if given.reply == reply_fingerprint(reply) => !given.failed,
            _ => true,
        }
    }

    /// The answer to send now for a session blocked on `reply`, once the ask has stood for `GRACE`, or `None`.
    pub fn due(&mut self, session: &str, reply: Option<&str>, now: f64) -> Option<String> {
        let Some(answer) = Self::answer(reply) else {
            self.pending.remove(session);
            return None;
        };
        let print = reply_fingerprint(reply);
        if self.answers.get(session).is_some_and(|given| given.reply == print) {
            return None;
        }
        let since = match self.pending.get(session) {
            Some((pending, since)) if *pending == print => *since,
            _ => {
                self.pending.insert(session.to_owned(), (print, now));
                now
            }
        };
        (now - since >= Self::GRACE).then_some(answer)
    }

    /// Records an answer as sent (or failed), so it is never sent twice.
    pub fn record(&mut self, session: &str, reply: Option<&str>, answer: &str, at: f64, failed: bool) {
        let given = Answer {
            reply: reply_fingerprint(reply),
            answer: answer.to_owned(),
            at,
            failed,
        };
        self.answers.insert(session.to_owned(), given);
        self.pending.remove(session);
    }

    /// The answer autopilot gave the reply a session is on now, for its line and card.
    pub fn given(&self, session: &str, reply: Option<&str>) -> Option<&Answer> {
        self.answers
            .get(session)
            .filter(|given| given.reply == reply_fingerprint(reply))
    }

    /// Drops what is known about sessions that have gone.
    pub fn keep(&mut self, sessions: &HashSet<String>) {
        self.answers.retain(|session, _| sessions.contains(session));
        self.pending.retain(|session, _| sessions.contains(session));
    }

    /// The answers as they are kept in the app's settings: `{session: {reply, answer, at, failed}}`.
    pub fn plist(&self) -> Value {
        let entries: Map<String, Value> = self
            .answers
            .iter()
            .map(|(session, given)| {
                let entry = json!({
                    "reply": given.reply,
                    "answer": given.answer,
                    "at": given.at,
                    "failed": given.failed,
                });
                (session.clone(), entry)
            })
            .collect();
        Value::Object(entries)
    }

    /// The answers read back from the app's settings. An entry missing its reply, answer or time is
    /// skipped; a missing `failed` is false.
    pub fn from_plist(plist: &Value) -> Self {
        let mut answers = HashMap::new();
        if let Some(entries) = plist.as_object() {
            for (session, value) in entries {
                let (Some(reply), Some(answer), Some(at)) = (
                    value.get("reply").and_then(Value::as_str),
                    value.get("answer").and_then(Value::as_str),
                    value.get("at").and_then(Value::as_f64),
                ) else {
                    continue;
                };
                let failed = value.get("failed").and_then(Value::as_bool).unwrap_or(false);
                let given = Answer {
                    reply: reply.to_owned(),
                    answer: answer.to_owned(),
                    at,
                    failed,
                };
                answers.insert(session.clone(), given);
            }
        }
        Self::with_answers(answers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASK: &str = "Two layouts fit.\nDecide: `compact` or `roomy`";

    #[test]
    fn answers_a_decide_line_with_the_recommended_answer() {
        assert_eq!(Autopilot::answer(Some(ASK)).as_deref(), Some("compact"));
        // A Reply line is the human's, so a session that only knows Reply is never answered for them.
        assert_eq!(Autopilot::answer(Some("Log in please.\nReply: `logged in`")), None);
        assert_eq!(Autopilot::answer(Some("Which one do you want, and why?")), None);
        assert_eq!(Autopilot::answer(None), None);
    }

    #[test]
    fn waits_out_the_grace_then_answers_once() {
        let mut pilot = Autopilot::default();
        assert_eq!(pilot.due("s1", Some(ASK), 100.0), None);
        assert_eq!(pilot.due("s1", Some(ASK), 100.0 + Autopilot::GRACE - 1.0), None);
        assert_eq!(
            pilot.due("s1", Some(ASK), 100.0 + Autopilot::GRACE).as_deref(),
            Some("compact")
        );
        pilot.record("s1", Some(ASK), "compact", 109.0, false);
        assert_eq!(pilot.due("s1", Some(ASK), 200.0), None);
        assert_eq!(pilot.given("s1", Some(ASK)).map(|a| a.answer.as_str()), Some("compact"));
        // The next ask is a new one.
        let next = "Done.\nDecide: `ship it` or `hold`";
        assert_eq!(pilot.given("s1", Some(next)), None);
        assert_eq!(pilot.due("s1", Some(next), 300.0), None);
        assert_eq!(
            pilot.due("s1", Some(next), 300.0 + Autopilot::GRACE).as_deref(),
            Some("ship it")
        );
    }

    #[test]
    fn a_failed_answer_goes_back_to_the_human() {
        let mut pilot = Autopilot::default();
        assert!(pilot.handles("s1", Some(ASK)));
        pilot.record("s1", Some(ASK), "compact", 1.0, true);
        assert!(!pilot.handles("s1", Some(ASK)));
        assert_eq!(pilot.due("s1", Some(ASK), 1000.0), None);
        assert!(!pilot.handles("s1", Some("Log in.\nReply: `logged in`")));
    }

    #[test]
    fn survives_a_restart() {
        let mut pilot = Autopilot::default();
        pilot.record("s1", Some(ASK), "compact", 5.0, false);
        let restored = Autopilot::from_plist(&pilot.plist());
        assert_eq!(restored, pilot);
        assert_eq!(
            restored.given("s1", Some(ASK)).map(|a| a.answer.as_str()),
            Some("compact")
        );
    }
}
