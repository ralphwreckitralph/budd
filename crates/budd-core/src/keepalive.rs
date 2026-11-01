//! The keep-alive decision for the assistant session (from bin/keepalive.sh).
//!
//! The keep-alive runs every few minutes and keeps one fresh background session named "assistant"
//! running. It is idempotent: a session that is live and in use is left alone; one that is idle for
//! long, or quiet and full, is stopped and replaced; one that is dead, or none, is replaced. This module
//! is the decision alone; who lists the sessions, checks the process and runs `claude` is the caller.

/// What the keep-alive found about the newest background session with the assistant's name.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    /// Whether its process is running. Liveness is decided by the pid alone, never by the status:
    /// `claude agents` still lists sessions a restart killed, with a status and no process.
    pub alive: bool,
    /// Its id, for `claude stop` and the log.
    pub id: String,
    /// Its status as `claude agents` reports it, such as "idle" or "busy", when it has one.
    pub status: Option<String>,
    /// Minutes since its transcript was last written, or `None` for a session that never got a message,
    /// which has no transcript yet and is never rotated.
    pub idle_minutes: Option<f64>,
    /// Its context size in tokens, 0 when unknown.
    pub tokens: u64,
}

/// What one keep-alive run has to decide on.
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    /// The newest session with the assistant's name, or `None` when there is none.
    pub newest: Option<Session>,
    /// The context size at which a quiet session is replaced.
    pub rotate_at: u64,
    /// How many idle minutes get an idle session replaced (30 by default).
    pub idle_min: f64,
    /// How many quiet minutes a full session needs before it is replaced (2 by default).
    pub quiet_min: f64,
    /// Whether the transcript grew between the check and now: a message landed, and stopping the session
    /// would kill that turn.
    pub transcript_grew: bool,
    /// Whether Budd is sending the session a message right now.
    pub send_in_flight: bool,
    /// Whether Budd has asked the session something and is still waiting on the answer.
    pub standing_ask: bool,
}

impl Input {
    /// The script's defaults: 30 idle minutes, 2 quiet minutes.
    pub const IDLE_MIN: f64 = 30.0;
    /// See [`Input::IDLE_MIN`].
    pub const QUIET_MIN: f64 = 2.0;
}

/// What the keep-alive does this run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The session is live and in use: do nothing.
    Nothing,
    /// There is no live session: start a fresh one. The reason is for the log.
    StartFresh(String),
    /// Stop the session and start a fresh one. The reason is for the log.
    Refresh(String),
    /// The session is due for a refresh but is left for the next run, for the reason given.
    Leave(String),
}

/// The keep-alive's decision for `input`.
pub fn decide(input: &Input) -> Decision {
    let Some(session) = &input.newest else {
        return Decision::StartFresh("no previous session".to_owned());
    };
    if !session.alive {
        return Decision::StartFresh(format!("session {} is not running", session.id));
    }
    if session.status.as_deref() != Some("idle") {
        return Decision::Nothing;
    }
    let Some(idle) = session.idle_minutes else {
        return Decision::Nothing;
    };
    let idle_whole = idle.floor() as u64;
    let reason = if idle >= input.idle_min {
        format!(
            "session {} idle for {idle_whole} min (limit {})",
            session.id, input.idle_min as u64
        )
    } else if idle >= input.quiet_min && session.tokens >= input.rotate_at {
        format!(
            "session {} at {} tokens (limit {}), quiet for {idle_whole} min",
            session.id, session.tokens, input.rotate_at
        )
    } else {
        return Decision::Nothing;
    };
    if input.send_in_flight {
        return Decision::Leave(format!("{reason}, but a message is on its way to it, leaving it"));
    }
    if input.standing_ask {
        return Decision::Leave(format!("{reason}, but Budd is still waiting on its answer, leaving it"));
    }
    if input.transcript_grew {
        return Decision::Leave(format!("{reason}, but it just got a message, leaving it"));
    }
    Decision::Refresh(reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hook::ROTATE_AT;

    fn session(alive: bool, status: Option<&str>, idle_minutes: Option<f64>, tokens: u64) -> Session {
        Session {
            alive,
            id: "s1".to_owned(),
            status: status.map(str::to_owned),
            idle_minutes,
            tokens,
        }
    }

    fn input(newest: Option<Session>) -> Input {
        Input {
            newest,
            rotate_at: ROTATE_AT,
            idle_min: Input::IDLE_MIN,
            quiet_min: Input::QUIET_MIN,
            transcript_grew: false,
            send_in_flight: false,
            standing_ask: false,
        }
    }

    #[test]
    fn no_session_or_a_dead_one_starts_fresh() {
        assert_eq!(
            decide(&input(None)),
            Decision::StartFresh("no previous session".to_owned())
        );
        // The status says nothing about liveness: a killed session still lists as blocked.
        assert_eq!(
            decide(&input(Some(session(false, Some("blocked"), Some(1.0), 0)))),
            Decision::StartFresh("session s1 is not running".to_owned())
        );
        assert_eq!(
            decide(&input(Some(session(false, Some("idle"), Some(99.0), 0)))),
            Decision::StartFresh("session s1 is not running".to_owned())
        );
    }

    #[test]
    fn a_session_in_use_is_left_alone() {
        assert_eq!(
            decide(&input(Some(session(true, Some("busy"), Some(99.0), ROTATE_AT)))),
            Decision::Nothing
        );
        assert_eq!(
            decide(&input(Some(session(true, Some("blocked"), Some(99.0), 0)))),
            Decision::Nothing
        );
        assert_eq!(
            decide(&input(Some(session(true, None, Some(99.0), 0)))),
            Decision::Nothing
        );
        // Idle but not for long, and not full.
        assert_eq!(
            decide(&input(Some(session(true, Some("idle"), Some(29.0), ROTATE_AT - 1)))),
            Decision::Nothing
        );
        // Never got a message: no transcript, never rotated.
        assert_eq!(
            decide(&input(Some(session(true, Some("idle"), None, ROTATE_AT)))),
            Decision::Nothing
        );
    }

    #[test]
    fn a_long_idle_session_is_refreshed() {
        assert_eq!(
            decide(&input(Some(session(true, Some("idle"), Some(30.0), 0)))),
            Decision::Refresh("session s1 idle for 30 min (limit 30)".to_owned())
        );
        assert_eq!(
            decide(&input(Some(session(true, Some("idle"), Some(45.7), 0)))),
            Decision::Refresh("session s1 idle for 45 min (limit 30)".to_owned())
        );
    }

    #[test]
    fn a_full_session_is_refreshed_once_quiet() {
        assert_eq!(
            decide(&input(Some(session(true, Some("idle"), Some(2.0), ROTATE_AT)))),
            Decision::Refresh("session s1 at 400000 tokens (limit 400000), quiet for 2 min".to_owned())
        );
        assert_eq!(
            decide(&input(Some(session(true, Some("idle"), Some(5.0), ROTATE_AT + 10)))),
            Decision::Refresh("session s1 at 400010 tokens (limit 400000), quiet for 5 min".to_owned())
        );
        // Full but not yet quiet; quiet but not full.
        assert_eq!(
            decide(&input(Some(session(true, Some("idle"), Some(1.5), ROTATE_AT)))),
            Decision::Nothing
        );
        assert_eq!(
            decide(&input(Some(session(true, Some("idle"), Some(5.0), ROTATE_AT - 1)))),
            Decision::Nothing
        );
        // The limits are the input's.
        let mut custom = input(Some(session(true, Some("idle"), Some(3.0), 1000)));
        custom.rotate_at = 1000;
        custom.quiet_min = 3.0;
        assert!(matches!(decide(&custom), Decision::Refresh(_)));
        custom.quiet_min = 4.0;
        assert_eq!(decide(&custom), Decision::Nothing);
        custom.idle_min = 3.0;
        assert_eq!(
            decide(&custom),
            Decision::Refresh("session s1 idle for 3 min (limit 3)".to_owned())
        );
    }

    #[test]
    fn a_due_session_is_left_while_a_message_is_in_play() {
        let mut grew = input(Some(session(true, Some("idle"), Some(40.0), 0)));
        grew.transcript_grew = true;
        assert_eq!(
            decide(&grew),
            Decision::Leave("session s1 idle for 40 min (limit 30), but it just got a message, leaving it".to_owned())
        );
        let mut sending = input(Some(session(true, Some("idle"), Some(2.0), ROTATE_AT)));
        sending.send_in_flight = true;
        assert!(
            matches!(decide(&sending), Decision::Leave(reason) if reason.ends_with("on its way to it, leaving it"))
        );
        let mut asked = input(Some(session(true, Some("idle"), Some(40.0), 0)));
        asked.standing_ask = true;
        assert!(matches!(decide(&asked), Decision::Leave(reason) if reason.contains("waiting on its answer")));
        // The holds only matter for a session that is due; a dead one is replaced regardless.
        let mut dead = input(Some(session(false, Some("idle"), Some(40.0), 0)));
        dead.send_in_flight = true;
        dead.standing_ask = true;
        dead.transcript_grew = true;
        assert!(matches!(decide(&dead), Decision::StartFresh(_)));
        let mut busy = input(Some(session(true, Some("busy"), Some(40.0), 0)));
        busy.transcript_grew = true;
        assert_eq!(decide(&busy), Decision::Nothing);
    }
}
