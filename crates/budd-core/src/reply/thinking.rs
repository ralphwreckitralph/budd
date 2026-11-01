//! What a running step shows over Budd's desk and on a conversation card: the kind of tool in the
//! thought cloud, the bubble line while an answer is awaited, and which message and answer the card pairs.

use crate::text::human_text;
use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;

/// The kind of tool a running step uses, for the glyph in the thought cloud over Budd's desk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolKind {
    /// Reading or searching: a magnifying glass.
    Look,
    /// Editing or writing a file: a pencil.
    Edit,
    /// Running a command: a terminal prompt.
    Shell,
    /// The web or the browser: a globe.
    Web,
    /// Handing work to a helper agent: two people.
    Helper,
    /// Anything else: a turning gear.
    Other,
}

impl ToolKind {
    /// Every kind, in order.
    pub const ALL: [ToolKind; 6] = [
        ToolKind::Look,
        ToolKind::Edit,
        ToolKind::Shell,
        ToolKind::Web,
        ToolKind::Helper,
        ToolKind::Other,
    ];

    /// A tool call this short would only flash its glyph, so the cloud keeps showing it this long after, in seconds.
    pub const HOLD: f64 = 1.2;

    /// From a Claude Code tool name, as the hook reports it.
    pub fn from_tool_name(tool_name: &str) -> ToolKind {
        match tool_name {
            "Read" | "Grep" | "Glob" | "LS" | "LSP" | "ToolSearch" => ToolKind::Look,
            "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => ToolKind::Edit,
            "Bash" | "BashOutput" | "KillShell" | "Monitor" => ToolKind::Shell,
            "WebFetch" | "WebSearch" => ToolKind::Web,
            "Agent" | "Task" | "SendMessage" | "Workflow" => ToolKind::Helper,
            name if name.starts_with("mcp__claude-in-chrome__") => ToolKind::Web,
            _ => ToolKind::Other,
        }
    }

    /// The kind's name as it is saved: "look", "edit", "shell", "web", "helper" or "other".
    pub fn name(self) -> &'static str {
        match self {
            ToolKind::Look => "look",
            ToolKind::Edit => "edit",
            ToolKind::Shell => "shell",
            ToolKind::Web => "web",
            ToolKind::Helper => "helper",
            ToolKind::Other => "other",
        }
    }
}

/// The bubble line while a sent message waits for its answer: who, what they are doing, and how long
/// it has been. Budd's thought cloud at his desk already says he is busy, so for him this only shows on
/// hover; a crew session's line stays beside its desk, which only shows that it types, not at what.
pub struct AwaitLine;

impl AwaitLine {
    /// For example "Atlas · Editing Bubble.swift · 45s". Steps lose a trailing "…", which reads oddly
    /// mid-line, and start with a capital, as crew steps such as "run swift test" do not.
    pub fn text(name: Option<&str>, step: &str, elapsed: Option<f64>) -> String {
        let step = step.strip_suffix('…').unwrap_or(step);
        let mut graphemes = step.graphemes(true);
        let step = match graphemes.next() {
            Some(first) => first.to_uppercase() + graphemes.as_str(),
            None => String::new(),
        };
        let parts: Vec<String> = [name.map(str::to_owned), Some(step), elapsed.and_then(Self::label)]
            .into_iter()
            .flatten()
            .collect();
        parts.join(" · ")
    }

    /// Coarse on purpose: the bubble resizes to fit its text, so a label that changed every second
    /// would make it twitch. Nothing for the first few seconds, then 5 s steps, then whole minutes.
    pub fn label(elapsed: f64) -> Option<String> {
        if elapsed < 5.0 {
            None
        } else if elapsed < 60.0 {
            Some(format!("{}s", elapsed as i64 / 5 * 5))
        } else {
            Some(format!("{}m", elapsed as i64 / 60))
        }
    }
}

/// The message Budd sent, while its answer is awaited: its text, how the card shows it (its pictures
/// noted), and when it went, in wall-clock seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Sent {
    /// The text alone, as it went.
    pub text: String,
    /// How the card shows it.
    pub label: String,
    /// When it went, wall-clock seconds.
    pub at: f64,
}

/// What a conversation card shows of a session's latest turn.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Shown {
    /// The message, as the human wrote it.
    pub prompt: Option<String>,
    /// Its answer, or what the session has said so far.
    pub reply: Option<String>,
    /// While the turn is under way, what it is doing, such as "Reading Brand.php · 15s".
    pub working: Option<String>,
}

impl Shown {
    fn new(prompt: Option<&str>, reply: Option<&str>, working: Option<String>) -> Self {
        Shown {
            prompt: prompt.map(str::to_owned),
            reply: reply.map(str::to_owned),
            working,
        }
    }
}

/// What a conversation card shows of a session's latest turn: the message, and either its answer or a line
/// saying it is being worked on. A message and an answer only ever show together when the answer is to that
/// message; mid-turn, the session's last answer belongs to the message before, and showing it under the new
/// one reads as a wrong answer. A message taken into a turn already running is answered while that turn
/// goes on, so what the session said since it shows above the line.
pub struct CardTurn;

impl CardTurn {
    /// What the card shows.
    ///
    /// - `sent`: the message Budd sent, while its answer is awaited.
    /// - `busy`: the session is in the middle of a turn.
    /// - `turn`: the message its current or last turn answers, as far as it is known.
    /// - `reply`: its last answer.
    /// - `so_far`: what it has said in the current turn since taking `turn` in mid-turn, else `None`.
    /// - `step`: what it is doing right now, such as "Reading Brand.php".
    /// - `now`: wall-clock seconds.
    pub fn resolve(
        sent: Option<&Sent>,
        busy: bool,
        turn: Option<&str>,
        reply: Option<&str>,
        so_far: Option<&str>,
        step: Option<&str>,
        now: f64,
    ) -> Shown {
        let step = step.filter(|step| !step.is_empty()).unwrap_or("Working");
        let Some(sent) = sent else {
            let prompt = turn.and_then(Self::human);
            return if busy {
                Shown {
                    prompt,
                    reply: so_far.map(str::to_owned),
                    working: Some(AwaitLine::text(None, step, None)),
                }
            } else {
                Shown {
                    prompt,
                    reply: reply.map(str::to_owned),
                    working: None,
                }
            };
        };
        let started = turn.is_some_and(|turn| turn_is_for(turn, &sent.text));
        let elapsed = now - sent.at;
        let label = Some(sent.label.as_str());
        if started && !busy {
            // Answered: the answer is on its way to being pinned, and it is this message's.
            return Shown::new(label, reply, None);
        }
        if started {
            return Shown::new(label, so_far, Some(AwaitLine::text(None, step, Some(elapsed))));
        }
        // Still running whatever was going when it was sent, or not begun yet.
        if busy {
            return Shown::new(label, None, Some(AwaitLine::text(Some("Queued"), step, None)));
        }
        Shown::new(
            label,
            None,
            Some(AwaitLine::text(None, "Sent, waiting for it to start", Some(elapsed))),
        )
    }

    /// The turn's message as the human wrote it, or `None` when the human did not write it: a background task
    /// finishing, another session's message, a system reminder. A long message Claude Code wrapped in a
    /// `pasted_content` tag comes out unwrapped, and a slash command as "/name args".
    pub fn human(turn: &str) -> Option<String> {
        let text = turn.trim();
        if text.starts_with("Another Claude session sent a message") {
            return None;
        }
        human_text(&Value::String(text.to_owned()))
    }
}

/// Whether `turn`, the prompt as the hook records it, is `message`: the text alone, without the
/// pictures' "[Image #1]" markers, and cut short as the hook cuts it. It is `SessionReport::turn_is_for`.
fn turn_is_for(turn: &str, message: &str) -> bool {
    crate::presence::SessionReport::turn_is_for(turn, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tools_map_to_their_glyph() {
        assert_eq!(ToolKind::from_tool_name("Read"), ToolKind::Look);
        assert_eq!(ToolKind::from_tool_name("Grep"), ToolKind::Look);
        assert_eq!(ToolKind::from_tool_name("Edit"), ToolKind::Edit);
        assert_eq!(ToolKind::from_tool_name("Write"), ToolKind::Edit);
        assert_eq!(ToolKind::from_tool_name("Bash"), ToolKind::Shell);
        assert_eq!(ToolKind::from_tool_name("WebSearch"), ToolKind::Web);
        assert_eq!(
            ToolKind::from_tool_name("mcp__claude-in-chrome__navigate"),
            ToolKind::Web
        );
        assert_eq!(ToolKind::from_tool_name("Agent"), ToolKind::Helper);
        assert_eq!(ToolKind::from_tool_name("mcp__slack__send"), ToolKind::Other);
        assert_eq!(ToolKind::from_tool_name(""), ToolKind::Other);
    }

    /// The Swift test decodes a `SessionReport`; that type belongs to the presence port, so this reads
    /// the report's `tool` field as JSON and maps it, with the same literals.
    #[test]
    fn report_carries_the_tool() {
        let report: Value = json!({"state": "working", "detail": "Reading a.swift", "tool": "Read", "at": 1});
        assert_eq!(
            report["tool"].as_str().map(ToolKind::from_tool_name),
            Some(ToolKind::Look)
        );
        // Written by an older hook: no tool, still a report.
        let older: Value = json!({"state": "thinking", "at": 1});
        assert_eq!(older["tool"].as_str(), None);
    }

    #[test]
    fn await_line_says_who_what_and_how_long() {
        assert_eq!(AwaitLine::text(None, "Thinking…", None), "Thinking");
        assert_eq!(
            AwaitLine::text(None, "Editing Bubble.swift", Some(47.0)),
            "Editing Bubble.swift · 45s"
        );
        assert_eq!(
            AwaitLine::text(Some("Atlas"), "run swift test", None),
            "Atlas · Run swift test"
        );
        assert_eq!(
            AwaitLine::text(Some("Atlas"), "Working", Some(130.0)),
            "Atlas · Working · 2m"
        );
    }

    #[test]
    fn elapsed_is_coarse_so_the_bubble_does_not_twitch() {
        assert_eq!(AwaitLine::label(0.0), None);
        assert_eq!(AwaitLine::label(4.9), None);
        assert_eq!(AwaitLine::label(5.0).as_deref(), Some("5s"));
        assert_eq!(AwaitLine::label(59.9).as_deref(), Some("55s"));
        assert_eq!(AwaitLine::label(60.0).as_deref(), Some("1m"));
        assert_eq!(AwaitLine::label(3599.0).as_deref(), Some("59m"));
    }

    fn sent() -> Sent {
        Sent {
            text: "API Setting tab: lets talk about this".into(),
            label: "API Setting tab: lets talk about this".into(),
            at: 1000.0,
        }
    }

    fn shown(prompt: Option<&str>, reply: Option<&str>, working: Option<&str>) -> Shown {
        Shown::new(prompt, reply, working.map(str::to_owned))
    }

    #[test]
    fn idle_pairs_the_turn_with_its_answer() {
        let result = CardTurn::resolve(None, false, Some("oh tapos"), Some("Fair, human."), None, None, 2000.0);
        assert_eq!(result, shown(Some("oh tapos"), Some("Fair, human."), None));
    }

    #[test]
    fn mid_turn_never_shows_the_answer_to_the_message_before() {
        // The hook has the new message; the last answer is still the one to "oh tapos".
        let result = CardTurn::resolve(
            None,
            true,
            Some("API Setting tab: lets talk about this"),
            Some("Fair, human."),
            None,
            Some("Reading Brand.php"),
            2000.0,
        );
        assert_eq!(result.prompt.as_deref(), Some("API Setting tab: lets talk about this"));
        assert_eq!(result.reply, None);
        assert_eq!(result.working.as_deref(), Some("Reading Brand.php"));
    }

    #[test]
    fn just_sent_before_any_hook() {
        let result = CardTurn::resolve(
            Some(&sent()),
            false,
            Some("oh tapos"),
            Some("Fair, human."),
            None,
            None,
            1002.0,
        );
        assert_eq!(result.prompt, Some(sent().label));
        assert_eq!(result.reply, None);
        assert_eq!(result.working.as_deref(), Some("Sent, waiting for it to start"));
    }

    #[test]
    fn working_on_the_message_sent() {
        let result = CardTurn::resolve(
            Some(&sent()),
            true,
            Some("API Setting tab: lets talk about this"),
            Some("Fair, human."),
            None,
            Some("Reading Brand.php…"),
            1017.0,
        );
        assert_eq!(
            result,
            shown(Some(&sent().label), None, Some("Reading Brand.php · 15s"))
        );
    }

    #[test]
    fn queued_behind_another_turn() {
        let result = CardTurn::resolve(
            Some(&sent()),
            true,
            Some("oh tapos"),
            None,
            None,
            Some("Running swift test"),
            1030.0,
        );
        assert_eq!(
            result,
            shown(Some(&sent().label), None, Some("Queued · Running swift test"))
        );
    }

    #[test]
    fn answered_before_the_answer_is_pinned() {
        let result = CardTurn::resolve(
            Some(&sent()),
            false,
            Some("API Setting tab: lets talk about this"),
            Some("Human, first a correction."),
            None,
            None,
            1032.0,
        );
        assert_eq!(
            result,
            shown(Some(&sent().label), Some("Human, first a correction."), None)
        );
    }

    #[test]
    fn a_message_taken_in_mid_turn_shows_its_answer_while_the_turn_goes_on() {
        let result = CardTurn::resolve(
            Some(&sent()),
            true,
            Some("API Setting tab: lets talk about this"),
            Some("Older answer."),
            Some("Not finalized yet.\n\nBack to the top-bar fix."),
            Some("Running swift test"),
            1030.0,
        );
        assert_eq!(
            result,
            shown(
                Some(&sent().label),
                Some("Not finalized yet.\n\nBack to the top-bar fix."),
                Some("Running swift test · 30s")
            )
        );
        let later = CardTurn::resolve(
            None,
            true,
            Some("API Setting tab: lets talk about this"),
            Some("Older answer."),
            Some("Not finalized yet."),
            Some("Running swift test"),
            1030.0,
        );
        assert_eq!(
            later,
            shown(
                Some(&sent().label),
                Some("Not finalized yet."),
                Some("Running swift test")
            )
        );
    }

    #[test]
    fn a_step_with_nothing_to_say_says_working() {
        let result = CardTurn::resolve(None, true, Some("x"), None, None, Some(""), 0.0);
        assert_eq!(result.working.as_deref(), Some("Working"));
    }

    #[test]
    fn only_what_the_human_wrote_is_theirs() {
        let notification =
            "<task-notification> <task-id>bc6irfqhk</task-id> <tool-use-id>toolu_016</tool-use-id></task-notification>";
        assert_eq!(
            CardTurn::resolve(None, false, Some(notification), Some("done"), None, None, 0.0).prompt,
            None
        );
        assert_eq!(
            CardTurn::human("Another Claude session sent a message: <agent-message>hi</agent-message>"),
            None
        );
        assert_eq!(
            CardTurn::human("<pasted_content id=\"1\">a long message\nover lines</pasted_content>").as_deref(),
            Some("a long message\nover lines")
        );
        assert_eq!(
            CardTurn::human("<command-name>/clear</command-name>").as_deref(),
            Some("/clear")
        );
        assert_eq!(CardTurn::human("  oh tapos ").as_deref(), Some("oh tapos"));
    }
}
