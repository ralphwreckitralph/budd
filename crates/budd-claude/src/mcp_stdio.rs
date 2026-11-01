//! The body of `budd mcp`: Budd's MCP server on stdio, forwarding tool calls over the endpoint.
//!
//! Claude Code starts `budd mcp` for each session and talks JSON-RPC to it, one message per line. The
//! protocol is `budd_core::tools::BuddMCP`; this module reads and writes the lines, and carries each tool
//! call to the running Budd over the endpoint as one JSON line, the way the macOS app's server did. A
//! Budd that is not running answers that, so the session carries on without him.

use crate::ipc::{self, Endpoint};
use crate::procs;
use budd_core::tools::{BuddMCP, BuddRequest, Call, ToolResult};
use serde_json::Value;
use std::io::{self, BufRead, Write};
use std::time::Duration;

/// What a session is told when nobody is listening at the endpoint.
pub const NOT_RUNNING: &str = "Budd is not running, so nothing was shown. Carry on without him.";
/// What a session is told when Budd was reached but gave no usable answer.
pub const NO_ANSWER: &str = "Budd did not answer";
/// How long one tool call may take end to end, as the macOS app allowed.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// How many ancestors are sent with a call, for the app to find the session among them.
pub const CALLERS: usize = 8;

/// Carries `call` to Budd at `endpoint` and returns his answer. Nobody listening is not an error for
/// the session, as Budd is decoration; anything else that goes wrong is.
pub fn forward(endpoint: &Endpoint, call: &Call, timeout: Duration) -> ToolResult {
    let Ok(line) = serde_json::to_string(call) else {
        return ToolResult::error(NO_ANSWER);
    };
    match ipc::call(endpoint, &line, timeout) {
        Ok(answer) => serde_json::from_str::<ToolResult>(&answer).unwrap_or_else(|_| ToolResult::error(NO_ANSWER)),
        Err(error) if ipc::is_not_running(&error) => ToolResult::ok(NOT_RUNNING),
        Err(_) => ToolResult::error(NO_ANSWER),
    }
}

/// Serves MCP from `input` to `output` until `input` ends. Each tool call is checked here first, so a
/// request Budd would refuse never crosses the endpoint, then forwarded with `callers`, this server's
/// ancestors, for the app to tell which session is calling.
pub fn serve(input: &mut dyn BufRead, output: &mut dyn Write, endpoint: &Endpoint, callers: &[u32]) -> io::Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let message = line.trim();
        if message.is_empty() {
            continue;
        }
        let reply = BuddMCP::respond(message.as_bytes(), |tool: &str, arguments: &Value| {
            if let Err(refused) = BuddRequest::make(tool, arguments) {
                return ToolResult::error(refused.message());
            }
            let call = Call {
                tool: tool.to_owned(),
                arguments: arguments.clone(),
                callers: callers.to_vec(),
            };
            forward(endpoint, &call, CALL_TIMEOUT)
        });
        if let Some(reply) = reply {
            output.write_all(&reply)?;
            output.flush()?;
        }
    }
}

/// Serves MCP on stdin and stdout until stdin closes, at the endpoint `BUDD_SOCKET` names or Budd's own.
pub fn main() {
    let endpoint = Endpoint::from_env(|key| std::env::var(key).ok());
    let callers = procs::ancestors(std::process::id(), CALLERS);
    let stdin = io::stdin();
    let stdout = io::stdout();
    // A closed stdout means Claude Code is gone; there is nobody left to tell.
    let _ = serve(&mut stdin.lock(), &mut stdout.lock(), &endpoint, &callers);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_call_nobody_answers_is_not_an_error_for_the_session() {
        let temp = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::UnixSocket(temp.path().join("none.sock"));
        let call = Call {
            tool: "say".to_owned(),
            arguments: json!({"text": "hi"}),
            callers: vec![1],
        };
        let result = forward(&endpoint, &call, Duration::from_secs(1));
        assert_eq!(result, ToolResult::ok(NOT_RUNNING));
        // The wrong kind of endpoint for this platform is Budd not answering.
        let other = if cfg!(windows) {
            Endpoint::UnixSocket(temp.path().join("x.sock"))
        } else {
            Endpoint::NamedPipe(r"\\.\pipe\budd-test".to_owned())
        };
        assert_eq!(
            forward(&other, &call, Duration::from_secs(1)),
            ToolResult::error(NO_ANSWER)
        );
    }

    #[cfg(unix)]
    mod on_unix {
        use super::*;
        use crate::ipc::Listener;
        use std::io::Cursor;
        use std::thread;

        /// A stand-in for the app: answers each call with what it would show, or garbage when asked to.
        fn fake_budd(endpoint: &Endpoint, answers: usize) -> thread::JoinHandle<Vec<Call>> {
            let listener = Listener::bind(endpoint).unwrap();
            thread::spawn(move || {
                let mut seen = Vec::new();
                for _ in 0..answers {
                    listener
                        .accept_line(|line| {
                            let call = Call::unwire(line.as_bytes()).unwrap();
                            let answer = match call.tool.as_str() {
                                "say" => "garbage".to_owned(),
                                tool => serde_json::to_string(&ToolResult::ok(format!(
                                    "{tool} shown, {} callers, first {:?}",
                                    call.callers.len(),
                                    call.callers.first()
                                )))
                                .unwrap(),
                            };
                            seen.push(call);
                            answer
                        })
                        .unwrap();
                }
                seen
            })
        }

        fn replies(input: &str, endpoint: &Endpoint, callers: &[u32]) -> Vec<Value> {
            let mut out = Vec::new();
            serve(&mut Cursor::new(input.as_bytes()), &mut out, endpoint, callers).unwrap();
            let text = String::from_utf8(out).unwrap();
            text.lines().map(|line| serde_json::from_str(line).unwrap()).collect()
        }

        #[test]
        fn initialize_list_and_call_cross_stdio_and_the_endpoint() {
            let temp = tempfile::tempdir().unwrap();
            let endpoint = Endpoint::UnixSocket(temp.path().join("budd.sock"));
            let budd = fake_budd(&endpoint, 2);
            let input = concat!(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
                "\n",
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                "\n\n",
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
                "\n",
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"ask","arguments":{"question":"Merge it?","answers":["Yes","No"]}}}"#,
                "\n",
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"status","arguments":{"line":""}}}"#,
                "\n",
                r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"say","arguments":{"text":"shipped"}}}"#,
                "\n",
            );
            let replies = replies(input, &endpoint, &[4242, 7]);
            assert_eq!(replies.len(), 5, "{replies:?}");
            assert_eq!(replies[0]["id"], 1);
            assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
            assert_eq!(replies[0]["result"]["serverInfo"]["name"], BuddMCP::SERVER_NAME);
            assert_eq!(replies[1]["id"], 2);
            let tools: Vec<&str> = replies[1]["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["name"].as_str().unwrap())
                .collect();
            assert_eq!(tools, vec!["ask", "status", "say"]);
            assert_eq!(replies[2]["id"], 3);
            assert_eq!(replies[2]["result"]["isError"], false);
            assert_eq!(
                replies[2]["result"]["content"][0]["text"],
                "ask shown, 2 callers, first Some(4242)"
            );
            // An empty status line is refused here, before the endpoint.
            assert_eq!(replies[3]["id"], 4);
            assert_eq!(replies[3]["result"]["isError"], true);
            assert_ne!(replies[3]["result"]["content"][0]["text"], NO_ANSWER);
            // Garbage from the app is "did not answer".
            assert_eq!(replies[4]["id"], 5);
            assert_eq!(replies[4]["result"]["isError"], true);
            assert_eq!(replies[4]["result"]["content"][0]["text"], NO_ANSWER);
            let seen = budd.join().unwrap();
            assert_eq!(seen.len(), 2);
            assert_eq!(seen[0].tool, "ask");
            assert_eq!(seen[0].arguments["answers"], json!(["Yes", "No"]));
            assert_eq!(seen[0].callers, vec![4242, 7]);
            assert_eq!(seen[1].tool, "say");
        }

        #[test]
        fn without_budd_the_session_is_told_to_carry_on() {
            let temp = tempfile::tempdir().unwrap();
            let endpoint = Endpoint::UnixSocket(temp.path().join("none.sock"));
            let input = concat!(
                r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"say","arguments":{"text":"hello"}}}"#,
                "\n",
                "not json\n",
                r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"dance","arguments":{}}}"#,
                "\n",
            );
            let replies = replies(input, &endpoint, &[]);
            assert_eq!(replies.len(), 3);
            assert_eq!(replies[0]["result"]["isError"], false);
            assert_eq!(replies[0]["result"]["content"][0]["text"], NOT_RUNNING);
            assert_eq!(replies[1]["error"]["code"], -32700);
            assert_eq!(replies[2]["error"]["code"], -32602);
        }
    }
}
