//! Readers of Claude Code's surfaces that may change between versions, with the versions they were verified against.
//!
//! Claude Code's version gate ([`version`]), the rows of `claude agents --json` ([`agents_json`]), and
//! the per-process records under `<config>/sessions/` ([`registry`]) are the surfaces Budd leans on
//! without a contract. Each reader is lenient: a field Claude Code drops or renames costs a detail,
//! never the whole listing. `core/compat.toml` says which versions the readers were checked against.

pub mod agents_json;
pub mod registry;
pub mod version;

use budd_core::crew::{AgentRow, LiveSession};

/// The background sessions on the bridge, from the rows of `claude agents --json`: every one whose
/// process `alive` confirms, and every one asleep while it waits on the human, oldest first. `alive`
/// is the process check; a pid the caller may not signal (EPERM) should count as alive there. This is
/// [`budd_core::crew::live_sessions`], reached through the adapter that owns the parsing.
pub fn live_sessions(rows: &[AgentRow], assistant: &str, alive: impl Fn(u32) -> bool) -> Vec<LiveSession> {
    budd_core::crew::live_sessions(rows, assistant, alive)
}

#[cfg(test)]
mod tests {
    use super::*;
    use budd_core::crew::{AgentState, Status};

    #[test]
    fn the_fixture_listing_becomes_the_bridge() {
        let rows = agents_json::parse(include_str!("../../fixtures/synthetic/agents.json"));
        assert_eq!(rows.len(), 5);
        let sessions = live_sessions(&rows, "assistant", |pid| pid != 40005);
        assert_eq!(
            sessions.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["assistant", "toy-1", "toy-2"]
        );
        assert_eq!(sessions[0].pid, Some(40001));
        assert_eq!(sessions[0].status, Some(Status::Idle));
        assert_eq!(sessions[1].status, Some(Status::Busy));
        assert!(sessions[2].asleep());
        assert_eq!(sessions[2].state, Some(AgentState::Blocked));
        assert_eq!(
            sessions[2].waiting_for.as_deref(),
            Some("Reply: approve the Jira comment?")
        );
        assert_eq!(sessions[2].session_id, "33333333-3333-4333-8333-333333333333");
        // The done row's process is gone, so it is gone; the interactive one never counted.
        assert!(sessions.iter().all(|s| s.id != "e5f6a7b8" && s.id != "d4e5f6a7"));
    }
}
