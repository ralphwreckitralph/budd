//! Budd's MCP tools: `ask`, `status` and `say`. The request each tool call makes once checked, the
//! board of what sessions said through them, the JSON-RPC half of the MCP server, and the wire types
//! between the server and the app. Ported from `AvatarCore/BuddTools.swift`.

use crate::reply::ReplyOptions;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};
use thiserror::Error;
use unicode_segmentation::UnicodeSegmentation;

/// A question a session put to the human through Budd's `ask` tool: the same ask as a reply ending in a
/// `Reply:` or `Decide:` line, made with a tool call instead of a convention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuddAsk {
    /// What the session needs from the human.
    pub question: String,
    /// The answers, each a button, recommended first.
    pub answers: Vec<String>,
    /// Any Claude with the session's context could make the call, the recommended answer first, so
    /// autopilot may answer it; a `Decide:` line rather than a `Reply:` line.
    pub may_decide: bool,
}

impl BuddAsk {
    /// An ask with its question, answers and who may decide it.
    pub fn new(question: impl Into<String>, answers: Vec<String>, may_decide: bool) -> Self {
        BuddAsk {
            question: question.into(),
            answers,
            may_decide,
        }
    }

    /// The ask written as the reply that would make it, so the bridge, the answer buttons, attention and
    /// autopilot treat it exactly as they treat a reply's last line.
    pub fn as_reply(&self) -> String {
        let line = self
            .answers
            .iter()
            .map(|answer| format!("`{answer}`"))
            .collect::<Vec<_>>()
            .join(" or ");
        let kind = if self.may_decide { "Decide" } else { "Reply" };
        format!("{}\n\n{kind}: {line}", self.question)
    }

    /// The session's reply so far with the question at its end, as the reply that would make the ask:
    /// whatever the session said this turn stays in view above the question.
    pub fn as_reply_after(&self, reply: Option<&str>) -> String {
        match reply.map(str::trim).filter(|reply| !reply.is_empty()) {
            Some(reply) => format!("{reply}\n\n{}", self.as_reply()),
            None => self.as_reply(),
        }
    }
}

/// Why Budd refused or could not carry out a tool call, said to the session that made it.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct BuddToolError(pub String);

impl BuddToolError {
    /// The error's words for the session.
    pub fn message(&self) -> &str {
        &self.0
    }
}

/// What a session asked Budd to do through one of his tools, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuddRequest {
    /// Say a line out loud.
    Say(String),
    /// Put a question to the human.
    Ask(BuddAsk),
    /// Set the line at the session's desk.
    Status(String),
}

/// How many characters `text` has, counting grapheme clusters as Swift's `String.count` does.
fn characters(text: &str) -> usize {
    text.graphemes(true).count()
}

impl BuddRequest {
    /// The most characters a `say` line may have.
    pub const LONGEST_LINE: usize = 200;
    /// The most characters an `ask` question may have.
    pub const LONGEST_QUESTION: usize = 1000;
    /// The most characters a `status` line may have.
    pub const LONGEST_STATUS: usize = 120;

    /// The request a tool call makes, or why it is refused, in words for the session that called it.
    pub fn make(tool: &str, arguments: &Value) -> Result<BuddRequest, BuddToolError> {
        let text = |key: &str, longest: usize| -> Result<String, BuddToolError> {
            let value = arguments.get(key).and_then(Value::as_str).map(str::trim).unwrap_or("");
            if value.is_empty() {
                return Err(BuddToolError(format!("`{key}` is required")));
            }
            if characters(value) > longest {
                return Err(BuddToolError(format!("`{key}` is longer than {longest} characters")));
            }
            Ok(value.to_owned())
        };
        match tool {
            "say" => text("text", Self::LONGEST_LINE).map(BuddRequest::Say),
            "status" => text("line", Self::LONGEST_STATUS).map(BuddRequest::Status),
            "ask" => {
                let question = text("question", Self::LONGEST_QUESTION)?;
                let given = arguments.get("answers").and_then(Value::as_array);
                let mut answers: Vec<String> = Vec::new();
                for answer in given.into_iter().flatten().filter_map(Value::as_str) {
                    let answer = answer.trim_matches([' ', '\t']);
                    if !answer.is_empty() && !answers.iter().any(|known| known == answer) {
                        answers.push(answer.to_owned());
                    }
                }
                if !(1..=ReplyOptions::MOST).contains(&answers.len()) {
                    return Err(BuddToolError(format!(
                        "`answers` needs 1 to {} different answers",
                        ReplyOptions::MOST
                    )));
                }
                if let Some(long) = answers.iter().find(|answer| characters(answer) > ReplyOptions::LONGEST) {
                    return Err(BuddToolError(format!(
                        "the answer \"{long}\" is longer than {} characters",
                        ReplyOptions::LONGEST
                    )));
                }
                let may_decide = arguments.get("mayDecide").and_then(Value::as_bool).unwrap_or(false);
                Ok(BuddRequest::Ask(BuddAsk::new(question, answers, may_decide)))
            }
            _ => Err(BuddToolError(format!("Budd has no tool called {tool}"))),
        }
    }
}

/// One request on the app's socket: the tool call as the session made it, and the chain of processes
/// above the server, which the app matches against the sessions it knows to tell who is calling.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    /// The tool's name.
    pub tool: String,
    /// Its arguments as given.
    #[serde(default)]
    pub arguments: Value,
    /// The process ids above the server, nearest first.
    #[serde(default)]
    pub callers: Vec<u32>,
}

impl Call {
    /// The request this call makes, checked again on the app's side, or why it is refused.
    pub fn request(&self) -> Result<BuddRequest, BuddToolError> {
        BuddRequest::make(&self.tool, &self.arguments)
    }

    /// The call read off the socket, or the refusal the app answers with when it cannot be read.
    pub fn unwire(line: &[u8]) -> Result<Call, BuddToolError> {
        serde_json::from_slice(line).map_err(|_| BuddToolError("Budd could not read the request".into()))
    }
}

/// What the app answers a tool call with: a line for the session, and whether the call failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    /// The line for the session.
    pub text: String,
    /// Whether the call failed.
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

/// The tool result as it crosses the app's socket.
pub type Answer = ToolResult;

impl ToolResult {
    /// A result that succeeded.
    pub fn ok(text: impl Into<String>) -> Self {
        ToolResult {
            text: text.into(),
            is_error: false,
        }
    }

    /// A result that failed, with why.
    pub fn error(text: impl Into<String>) -> Self {
        ToolResult {
            text: text.into(),
            is_error: true,
        }
    }
}

/// Budd's MCP server, the protocol half: one JSON-RPC message in, its reply out, over stdio as Claude
/// Code runs it. Small on purpose, as Budd offers tools and nothing else (no resources, prompts or
/// sampling): `initialize`, `ping`, `tools/list` and `tools/call`, and notifications, which get no reply.
pub struct BuddMCP;

impl BuddMCP {
    /// Protocol versions this server speaks, newest first; a client asking for another gets the newest.
    pub const VERSIONS: [&'static str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
    /// The server's name, as `initialize` reports it.
    pub const SERVER_NAME: &'static str = "budd";

    /// What the server tells a client about itself.
    pub const INSTRUCTIONS: &'static str = "Budd keeps an office at the edge of the human's screen, with a desk for \
        every background Claude session, and opens it to call the human over when one needs them. Use ask to put a \
        question to the human with answer buttons, status to say where you stand, and say for a line worth hearing \
        out loud.";

    /// The tools, as `tools/list` describes them.
    pub fn tools() -> Vec<Value> {
        vec![
            json!({
                "name": "ask",
                "description": "Put a question to the human at your desk in Budd's office, with 1 to 4 short answers \
                    as buttons, and call them over. Returns at once; their answer arrives as your next message, so end \
                    your turn after asking. Set mayDecide only for a call any Claude with your context could make, your \
                    recommended answer first: in autopilot Budd then answers it himself. Never set it for logins, \
                    merges, approvals or anything destructive.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "question": {"type": "string", "description": "What you need from the human, in a sentence or two."},
                        "answers": {
                            "type": "array", "items": {"type": "string", "maxLength": ReplyOptions::LONGEST},
                            "minItems": 1, "maxItems": ReplyOptions::MOST,
                            "description": "The answers, each a button, recommended first.",
                        },
                        "mayDecide": {"type": "boolean", "description": "Autopilot may answer with the first answer."},
                    },
                    "required": ["question", "answers"],
                },
            }),
            json!({
                "name": "status",
                "description": "Set the line at your desk in Budd's office: where you stand, in a few words, such as \
                    \"writing the migration tests\".",
                "inputSchema": {
                    "type": "object",
                    "properties": {"line": {"type": "string", "maxLength": BuddRequest::LONGEST_STATUS}},
                    "required": ["line"],
                },
            }),
            json!({
                "name": "say",
                "description": "Say a short line out loud through Budd, for a moment worth hearing: a ticket shipped, \
                    a blocker found. He stays quiet while the human is away, on a call, or heard the same line within a \
                    minute.",
                "inputSchema": {
                    "type": "object",
                    "properties": {"text": {"type": "string", "maxLength": BuddRequest::LONGEST_LINE}},
                    "required": ["text"],
                },
            }),
        ]
    }

    /// The reply to one message as a line of JSON, or `None` for a notification or a reply from the client.
    /// `call` carries out a tool call with its name and arguments as given.
    pub fn respond(message: &[u8], mut call: impl FnMut(&str, &Value) -> ToolResult) -> Option<Vec<u8>> {
        let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(message) else {
            return Some(Self::error(Value::Null, -32700, "Parse error"));
        };
        // A notification has no id and gets no reply; neither does a reply to a request of ours.
        let id = object.get("id")?.clone();
        let method = object.get("method").and_then(Value::as_str)?;
        let empty = Map::new();
        let params = object.get("params").and_then(Value::as_object).unwrap_or(&empty);
        let reply = match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
                let version = if Self::VERSIONS.contains(&asked) {
                    asked
                } else {
                    Self::VERSIONS[0]
                };
                Self::result(
                    id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": Self::SERVER_NAME, "version": "1.0.0"},
                        "instructions": Self::INSTRUCTIONS,
                    }),
                )
            }
            "ping" => Self::result(id, json!({})),
            "tools/list" => Self::result(id, json!({"tools": Self::tools()})),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str);
                let known = name.is_some_and(|name| Self::tools().iter().any(|tool| tool["name"] == name));
                let Some(name) = name.filter(|_| known) else {
                    return Some(Self::error(
                        id,
                        -32602,
                        &format!("Unknown tool: {}", name.unwrap_or("none")),
                    ));
                };
                let arguments = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
                let result = call(name, &arguments);
                Self::result(
                    id,
                    json!({"content": [{"type": "text", "text": result.text}], "isError": result.is_error}),
                )
            }
            _ => Self::error(id, -32601, &format!("Method not found: {method}")),
        };
        Some(reply)
    }

    fn result(id: Value, result: Value) -> Vec<u8> {
        Self::encode(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }

    fn error(id: Value, code: i64, message: &str) -> Vec<u8> {
        Self::encode(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}))
    }

    /// One line of JSON: stdio messages are delimited by newlines and must not contain one.
    fn encode(object: Value) -> Vec<u8> {
        let mut line = object.to_string().into_bytes();
        line.push(b'\n');
        line
    }
}

/// What a session said in one turn through Budd's tools.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Said {
    ask: Option<BuddAsk>,
    status: Option<String>,
    /// The message the turn answered when it was said.
    turn: Option<String>,
}

/// What sessions told Budd through his tools: each one's standing ask and its status line.
///
/// Both belong to the turn they were made in. An ask stands until the session's next turn starts, as a
/// reply's last line does: the human's answer is that next turn. A status line goes with its turn too,
/// so a session that finished and moved on never shows where it stood two tasks ago.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuddBoard {
    said: HashMap<String, Said>,
}

impl BuddBoard {
    /// A session asked, in the turn answering `turn`.
    pub fn ask(&mut self, ask: BuddAsk, session: &str, turn: Option<&str>) {
        self.update(session, turn).ask = Some(ask);
    }

    /// A session set its status line, in the turn answering `turn`.
    pub fn status(&mut self, line: &str, session: &str, turn: Option<&str>) {
        self.update(session, turn).status = Some(line.to_owned());
    }

    /// The session's ask, while the turn it was made in is still the current one.
    pub fn ask_of(&self, session: &str, turn: Option<&str>) -> Option<&BuddAsk> {
        self.current(session, turn)?.ask.as_ref()
    }

    /// The session's status line, while the turn it was set in is still the current one.
    pub fn status_of(&self, session: &str, turn: Option<&str>) -> Option<&str> {
        self.current(session, turn)?.status.as_deref()
    }

    /// Drops what sessions that have gone said.
    pub fn keep(&mut self, sessions: &HashSet<String>) {
        self.said.retain(|session, _| sessions.contains(session));
    }

    fn current(&self, session: &str, turn: Option<&str>) -> Option<&Said> {
        self.said.get(session).filter(|said| said.turn.as_deref() == turn)
    }

    fn update(&mut self, session: &str, turn: Option<&str>) -> &mut Said {
        let entry = self.said.entry(session.to_owned()).or_default();
        if entry.turn.as_deref() != turn {
            *entry = Said {
                turn: turn.map(str::to_owned),
                ..Said::default()
            };
        }
        entry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reply::{Autopilot, Kind};

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    mod budd_request {
        use super::*;

        #[test]
        fn makes_each_tool() {
            assert_eq!(
                BuddRequest::make("say", &json!({"text": " Shipped it. "})),
                Ok(BuddRequest::Say("Shipped it.".into()))
            );
            assert_eq!(
                BuddRequest::make("status", &json!({"line": "writing tests"})),
                Ok(BuddRequest::Status("writing tests".into()))
            );
            assert_eq!(
                BuddRequest::make(
                    "ask",
                    &json!({"question": "Which layout?", "answers": ["compact", "roomy", "compact"], "mayDecide": true})
                ),
                Ok(BuddRequest::Ask(BuddAsk::new(
                    "Which layout?",
                    strings(&["compact", "roomy"]),
                    true
                )))
            );
        }

        #[test]
        fn refuses_what_does_not_fit() {
            let refusal = |tool: &str, arguments: Value| BuddRequest::make(tool, &arguments).err().map(|e| e.0);
            assert_eq!(refusal("say", json!({})).as_deref(), Some("`text` is required"));
            assert_eq!(
                refusal("say", json!({"text": "a".repeat(201)})).as_deref(),
                Some("`text` is longer than 200 characters")
            );
            assert_eq!(
                refusal("ask", json!({"question": "Go?", "answers": []})).as_deref(),
                Some("`answers` needs 1 to 4 different answers")
            );
            assert_eq!(
                refusal("ask", json!({"question": "Go?", "answers": ["a", "b", "c", "d", "e"]})).as_deref(),
                Some("`answers` needs 1 to 4 different answers")
            );
            assert!(
                refusal("ask", json!({"question": "Go?", "answers": ["x".repeat(25)]}))
                    .is_some_and(|message| message.starts_with("the answer"))
            );
            assert_eq!(
                refusal("dance", json!({})).as_deref(),
                Some("Budd has no tool called dance")
            );
        }

        #[test]
        fn crosses_the_socket_and_is_checked_again() {
            let call = Call {
                tool: "status".into(),
                arguments: json!({"line": "on it"}),
                callers: vec![4242, 1],
            };
            let wire = serde_json::to_vec(&call).unwrap();
            let read = Call::unwire(&wire).unwrap();
            assert_eq!(read.request(), Ok(BuddRequest::Status("on it".into())));
            assert_eq!(read.callers, [4242, 1]);
            assert!(Call::unwire(b"{not json").is_err());
            let result = ToolResult::ok("Shown.");
            let wire = serde_json::to_vec(&result).unwrap();
            assert!(wire.windows(7).any(|w| w == b"isError"));
            assert_eq!(serde_json::from_slice::<Answer>(&wire).unwrap(), result);
        }

        #[test]
        fn an_ask_reads_as_the_reply_that_would_make_it() {
            let ask = BuddAsk::new("Which layout?", strings(&["compact", "roomy"]), true);
            let options = ReplyOptions::parse(&ask.as_reply_after(Some("I tried both."))).unwrap();
            assert_eq!(options.answers, ["compact", "roomy"]);
            assert_eq!(options.kind, Kind::Decide);
            assert_eq!(options.body, "I tried both.\n\nWhich layout?");
            let human = BuddAsk::new("Merge it?", strings(&["merged"]), false);
            assert_eq!(
                ReplyOptions::parse(&human.as_reply_after(None)).map(|o| o.kind),
                Some(Kind::Reply)
            );
            assert_eq!(Autopilot::answer(Some(&human.as_reply())), None);
        }
    }

    mod budd_board {
        use super::*;

        fn ask() -> BuddAsk {
            BuddAsk::new("Go?", strings(&["go"]), false)
        }

        #[test]
        fn an_ask_stands_until_the_next_turn() {
            let mut board = BuddBoard::default();
            board.ask(ask(), "s1", Some("t1"));
            board.status("testing", "s1", Some("t1"));
            assert_eq!(board.ask_of("s1", Some("t1")), Some(&ask()));
            assert_eq!(board.status_of("s1", Some("t1")), Some("testing"));
            assert_eq!(board.ask_of("s1", Some("t2")), None);
            assert_eq!(board.status_of("s1", Some("t2")), None);
            assert_eq!(board.ask_of("s2", Some("t1")), None);
        }

        #[test]
        fn a_new_turn_starts_clean() {
            let mut board = BuddBoard::default();
            board.ask(ask(), "s1", Some("t1"));
            board.status("writing", "s1", Some("t2"));
            assert_eq!(board.ask_of("s1", Some("t2")), None);
            assert_eq!(board.status_of("s1", Some("t2")), Some("writing"));
        }

        #[test]
        fn forgets_sessions_that_have_gone() {
            let mut board = BuddBoard::default();
            board.ask(ask(), "s1", Some("t1"));
            board.keep(&HashSet::from(["s2".to_owned()]));
            assert_eq!(board.ask_of("s1", Some("t1")), None);
        }

        /// The Swift test reads two transcripts through `TranscriptSummary.read`, which belongs to the
        /// transcript port; what the board relies on is that a turn is its entry's uuid, not its words,
        /// so the same words under a new uuid are a new turn.
        #[test]
        fn the_same_words_twice_are_two_turns() {
            let mut board = BuddBoard::default();
            board.ask(ask(), "s1", Some("u1"));
            assert_eq!(board.ask_of("s1", Some("u1")), Some(&ask()));
            assert_eq!(board.ask_of("s1", Some("u2")), None);
        }
    }

    mod budd_mcp {
        use super::*;

        fn send_with(message: &str, call: impl FnMut(&str, &Value) -> ToolResult) -> Option<Value> {
            let data = BuddMCP::respond(message.as_bytes(), call)?;
            assert_eq!(data.last(), Some(&b'\n'));
            assert!(!data[..data.len() - 1].contains(&b'\n'));
            assert!(
                !String::from_utf8(data.clone()).unwrap().contains("\\/"),
                "slashes are not escaped"
            );
            Some(serde_json::from_slice(&data).unwrap())
        }

        fn send(message: &str) -> Option<Value> {
            send_with(message, |_, _| ToolResult::ok("ok"))
        }

        #[test]
        fn agrees_on_a_version() {
            let known =
                send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#)
                    .unwrap();
            let result = &known["result"];
            assert_eq!(result["protocolVersion"], "2025-06-18");
            assert_eq!(result["serverInfo"]["name"], "budd");
            assert!(result["capabilities"]["tools"].is_object());
            let unknown =
                send(r#"{"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"2099-01-01"}}"#)
                    .unwrap();
            assert_eq!(unknown["result"]["protocolVersion"], BuddMCP::VERSIONS[0]);
            assert_eq!(unknown["id"], "a");
        }

        #[test]
        fn lists_its_tools() {
            let reply = send(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
            let tools = reply["result"]["tools"].as_array().unwrap();
            assert_eq!(
                tools.iter().map(|t| t["name"].as_str().unwrap()).collect::<Vec<_>>(),
                ["ask", "status", "say"]
            );
            assert!(tools.iter().all(|t| t["inputSchema"]["type"] == "object"));
        }

        #[test]
        fn calls_a_tool() {
            let mut called: Option<(String, Option<String>)> = None;
            let reply = send_with(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"status","arguments":{"line":"on it"}}}"#,
                |tool, arguments| {
                    called = Some((tool.to_owned(), arguments["line"].as_str().map(str::to_owned)));
                    ToolResult::ok("Shown.")
                },
            )
            .unwrap();
            assert_eq!(called, Some(("status".to_owned(), Some("on it".to_owned()))));
            let result = &reply["result"];
            assert_eq!(result["isError"], false);
            assert_eq!(result["content"][0]["text"], "Shown.");
        }

        #[test]
        fn a_failed_call_is_a_tool_error() {
            let reply = send_with(
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"say","arguments":{}}}"#,
                |_, _| ToolResult::error("Budd is not running"),
            )
            .unwrap();
            assert_eq!(reply["result"]["isError"], true);
        }

        #[test]
        fn refuses_what_it_does_not_know() {
            let tool = send(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"dance"}}"#).unwrap();
            assert_eq!(tool["error"]["code"], -32602);
            let method = send(r#"{"jsonrpc":"2.0","id":6,"method":"resources/list"}"#).unwrap();
            assert_eq!(method["error"]["code"], -32601);
            let garbage = send("{not json").unwrap();
            assert_eq!(garbage["error"]["code"], -32700);
            assert_eq!(garbage["id"], Value::Null);
        }

        #[test]
        fn notifications_get_no_reply() {
            assert_eq!(send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#), None);
            assert_eq!(send(r#"{"jsonrpc":"2.0","id":7,"result":{}}"#), None);
            assert!(send(r#"{"jsonrpc":"2.0","id":8,"method":"ping"}"#).unwrap()["result"].is_object());
        }
    }
}
