//! What Budd remembers and what Reset forgets; the configuration keys (from AvatarCore/BuddState.swift).
//!
//! State is what Budd learned while running: Reset Budd… forgets it, so he starts over as a new install
//! would. Configuration is what the human set by hand: it stays, as it says how they want him, not what
//! he saw. Retired keys belonged to builds before this one and are deleted at launch.
//! Any setting a module reads or writes must be in one of the three lists.

/// Keys Reset Budd… forgets: what he learned while running.
pub const STATE_KEYS: &[&str] = &[
    "claudeConfigDir",
    "autopilotAnswers",
    "crewAttention",
    "unreadCount",
    "unreadNewest",
    "sounds",
    "voice",
    "pilotMode",
    "openAtLogin",
    "officeOpen",
    // Left by builds before Open at Login kept the human's choice; nothing reads it any more.
    "loginItemConfigured",
];

/// Keys Reset Budd… keeps: what the human set by hand.
pub const CONFIG_KEYS: &[&str] = &[
    "sessionName",
    "claudePath",
    "crewScript",
    "ticketProjects",
    "crewContextLimit",
    "voiceIdentifier",
];

/// Settings of the roaming robot Budd was before he moved into the office; deleted at launch.
pub const RETIRED_KEYS: &[&str] = &["roaming", "tricks"];

/// Whether `key` is in one of the three lists.
pub fn is_known(key: &str) -> bool {
    [STATE_KEYS, CONFIG_KEYS, RETIRED_KEYS]
        .iter()
        .any(|keys| keys.contains(&key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::CONFIG_DIR_KEY;
    use std::collections::HashSet;

    /// Every key is state, configuration or retired, never two of them, and the lists are not stubs.
    /// The Swift test read every key the sources used; here the lists are the single place keys live.
    #[test]
    fn the_key_lists_are_disjoint_and_complete() {
        let state: HashSet<&str> = STATE_KEYS.iter().copied().collect();
        let config: HashSet<&str> = CONFIG_KEYS.iter().copied().collect();
        let retired: HashSet<&str> = RETIRED_KEYS.iter().copied().collect();
        assert_eq!(state.len(), STATE_KEYS.len(), "a state key is listed twice");
        assert_eq!(config.len(), CONFIG_KEYS.len(), "a configuration key is listed twice");
        assert_eq!(retired.len(), RETIRED_KEYS.len(), "a retired key is listed twice");
        assert!(state.is_disjoint(&config));
        assert!(state.is_disjoint(&retired));
        assert!(config.is_disjoint(&retired));
        assert!(STATE_KEYS.len() > 10);
        assert!(state.contains(CONFIG_DIR_KEY));
        assert!(is_known("sessionName") && is_known("roaming") && !is_known("somethingNew"));
    }
}
