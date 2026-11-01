//! The session's replies the human has not read yet, and how Budd waves about them at his desk.
//!
//! A reply is known by when it was written (its file's modification time). The first one seen is the
//! baseline, not news, so starting the app never announces an old reply; every newer one counts.
//! Reading clears the count. The state is plain values so it can be saved between launches.

/// How many replies are unread, when the newest was written, and when the last one arrived.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UnreadReplies {
    count: usize,
    newest: Option<f64>,
    arrived_at: Option<f64>,
}

impl UnreadReplies {
    /// The big wave when a reply arrives, in seconds.
    pub const GREETING: f64 = 3.0;
    /// After that, a shorter wave closing every `PERIOD` seconds until it is read, so there is a pause
    /// between the greeting and the first one.
    pub const PERIOD: f64 = 8.0;
    /// How long each later wave lasts.
    pub const BURST: f64 = 1.6;
    /// How long the pointer has to rest on Budd's desk or his card for the reply to count as read.
    pub const READ_AFTER_HOVER: f64 = 1.5;

    /// The state restored from a save: how many were unread, and when the newest reply seen was written.
    pub fn restored(count: usize, newest: Option<f64>) -> Self {
        UnreadReplies {
            count,
            newest,
            arrived_at: None,
        }
    }

    /// How many replies are unread.
    pub fn count(&self) -> usize {
        self.count
    }

    /// When the newest reply seen was written, read or not.
    pub fn newest(&self) -> Option<f64> {
        self.newest
    }

    /// When the last unread reply arrived, on the caller's animation clock; the waving is timed from it.
    pub fn arrived_at(&self) -> Option<f64> {
        self.arrived_at
    }

    /// The session's last reply is the one written at `written_at`; `now` is the animation clock.
    pub fn see(&mut self, written_at: f64, now: f64) {
        let Some(newest) = self.newest else {
            self.newest = Some(written_at);
            return;
        };
        if written_at <= newest {
            return;
        }
        self.newest = Some(written_at);
        self.count += 1;
        self.arrived_at = Some(now);
    }

    /// The human read the replies: nothing is unread, and the waving stops.
    pub fn read(&mut self) {
        self.count = 0;
        self.arrived_at = None;
    }

    /// How strongly to wave at `now`: 0 is not at all, 1 a full wave. Ramps in and out so the arm
    /// never jumps. Unread replies from before a restart have no arrival time and wave on the period.
    pub fn wave(&self, now: f64) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let since = now - self.arrived_at.unwrap_or(-Self::GREETING);
        if since < Self::GREETING {
            return ramp(since, Self::GREETING);
        }
        let phase = (since - Self::GREETING) % Self::PERIOD - (Self::PERIOD - Self::BURST);
        if phase >= 0.0 {
            0.8 * ramp(phase, Self::BURST)
        } else {
            0.0
        }
    }
}

/// 0 at both ends of `length`, 1 in between, eased over a quarter second at each end.
fn ramp(time: f64, length: f64) -> f64 {
    let edge = 0.25;
    let rise = time.min(length - time) / edge;
    let clamped = rise.clamp(0.0, 1.0);
    clamped * clamped * (3.0 - 2.0 * clamped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_reply_seen_is_not_news() {
        let mut unread = UnreadReplies::default();
        unread.see(100.0, 0.0);
        assert_eq!(unread.count(), 0);
        assert_eq!(unread.wave(1.0), 0.0);
    }

    #[test]
    fn each_newer_reply_counts_and_older_ones_do_not() {
        let mut unread = UnreadReplies::restored(0, Some(100.0));
        unread.see(200.0, 10.0);
        unread.see(200.0, 11.0);
        unread.see(150.0, 12.0);
        assert_eq!(unread.count(), 1);
        unread.see(300.0, 20.0);
        assert_eq!(unread.count(), 2);
        assert_eq!(unread.newest(), Some(300.0));
    }

    #[test]
    fn reading_clears_the_count_but_not_what_was_seen() {
        let mut unread = UnreadReplies::restored(0, Some(100.0));
        unread.see(200.0, 10.0);
        unread.read();
        assert_eq!(unread.count(), 0);
        unread.see(200.0, 11.0);
        assert_eq!(unread.count(), 0);
    }

    #[test]
    fn greets_with_a_big_wave_then_waves_in_bursts() {
        let mut unread = UnreadReplies::restored(0, Some(100.0));
        unread.see(200.0, 10.0);
        assert_eq!(unread.wave(10.0), 0.0);
        assert_eq!(unread.wave(11.5), 1.0);
        // A pause after the greeting, then a burst closing each period.
        let first_burst = 13.0 + UnreadReplies::PERIOD - UnreadReplies::BURST;
        assert_eq!(unread.wave(13.1), 0.0);
        assert_eq!(unread.wave(first_burst - 0.1), 0.0);
        assert_eq!(unread.wave(first_burst + 0.8), 0.8);
        assert_eq!(unread.wave(first_burst + UnreadReplies::BURST + 0.1), 0.0);
        assert_eq!(unread.wave(first_burst + UnreadReplies::PERIOD + 0.8), 0.8);
    }

    #[test]
    fn a_count_restored_from_before_a_restart_still_waves() {
        let unread = UnreadReplies::restored(2, Some(300.0));
        let most = (0..80)
            .map(|tenth| unread.wave(f64::from(tenth) / 10.0))
            .fold(0.0_f64, f64::max);
        assert_eq!(most, 0.8);
    }
}
