//! The rows of `claude agents --json` (verified against 2.1.316).
//!
//! The listing is a JSON array of objects with `id`, `sessionId`, `name`, `cwd`, `pid`, `status`
//! (idle, busy, waiting), `state` (working, blocked, done, failed, stopped), `waitingFor`, `startedAt`
//! and `kind` ("background" for `--bg` sessions). [`AgentRow`] takes every field as optional, so a
//! field Claude Code drops costs a detail and not the listing; `budd_core::crew::live_sessions` decides
//! what each row is worth.

pub use budd_core::crew::AgentRow;
use serde_json::Value;

/// The rows in `text`, the output of `claude agents --json`. Output that is not a JSON array reads as
/// no rows, and a row that is not an object is skipped, since the listing is polled and a bad read
/// must not take the bridge down.
pub fn parse(text: &str) -> Vec<AgentRow> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(rows)) => from_values(rows),
        _ => Vec::new(),
    }
}

/// The rows among `values`, already parsed as JSON by the caller; a value that is not an object is
/// skipped.
pub fn from_values(values: Vec<Value>) -> Vec<AgentRow> {
    values
        .into_iter()
        .filter(|value| value.is_object())
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_row_of_the_fixture_leniently() {
        let rows = parse(include_str!("../../fixtures/synthetic/agents.json"));
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].name.as_deref(), Some("assistant"));
        assert_eq!(rows[0].pid, Some(40001));
        assert_eq!(rows[0].waiting_for, None);
        assert_eq!(rows[2].pid, None);
        assert_eq!(rows[2].state.as_deref(), Some("blocked"));
        assert_eq!(rows[2].waiting_for.as_deref(), Some("Reply: approve the Jira comment?"));
        assert_eq!(rows[3].kind.as_deref(), Some("interactive"));
        assert_eq!(rows[3].state, None);
        assert_eq!(rows[4].started_at, Some(1761900240000.0));
    }

    #[test]
    fn bad_output_is_no_rows_and_a_bad_row_is_skipped() {
        assert!(parse("").is_empty());
        assert!(parse("Error: not logged in").is_empty());
        assert!(parse("{}").is_empty());
        let rows = parse(r#"[{"id": "x", "kind": "background"}, 42, "y", {"pid": "not a number"}]"#);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id.as_deref(), Some("x"));
    }
}
