//! Process facts the engine and the hook need: is a pid alive, when it started, who its ancestors are. One file per platform.
//!
//! Every function here asks the OS about one process and never scans the whole process table, except
//! `parent` on Windows, where the Toolhelp snapshot is the documented way to learn a parent. The hook,
//! which runs on every tool call, only asks for a start time when the pid changes.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(windows)]
use windows as platform;

/// Whether the process `pid` is running. A process this user may not signal still counts as alive.
pub fn is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unix::is_alive(pid)
    }
    #[cfg(windows)]
    {
        windows::is_alive(pid)
    }
}

/// When the process `pid` started, in whole seconds since the Unix epoch, or `None` when the OS will not say.
pub fn start_time(pid: u32) -> Option<u64> {
    platform::start_time(pid)
}

/// The parent of the process `pid`, or `None` when it has none the OS will name.
pub fn parent(pid: u32) -> Option<u32> {
    platform::parent(pid)
}

/// The file name of the program the process `pid` runs, such as "claude" or "node.exe", or `None`.
pub fn image_name(pid: u32) -> Option<String> {
    platform::image_name(pid)
}

/// The ancestors of the process `pid`, nearest first, at most `max` of them. The chain stops at the
/// first process without a parent, at init (pid 1) or the Windows idle and system processes, and at a
/// parent already seen, since a recycled pid can point the chain back on itself.
pub fn ancestors(pid: u32, max: usize) -> Vec<u32> {
    let mut chain = Vec::new();
    let mut current = pid;
    while chain.len() < max {
        let Some(next) = parent(current) else { break };
        if next <= 1 || next == current || chain.contains(&next) {
            break;
        }
        #[cfg(windows)]
        if windows::started_after(next, current) {
            // The recorded parent pid was reused by a process younger than the child: not a real parent.
            break;
        }
        chain.push(next);
        current = next;
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_current_process_is_alive_and_has_a_past() {
        let me = std::process::id();
        assert!(is_alive(me));
        let started = start_time(me).expect("the OS knows when this process started");
        // Started no later than now, and well after the epoch.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        assert!(started <= now + 1, "started {started} after now {now}");
        assert!(started > 315_360_000);
        let parent = parent(me).expect("the test runner has a parent");
        assert!(is_alive(parent));
        let chain = ancestors(me, 8);
        assert!(!chain.is_empty());
        assert_eq!(chain[0], parent);
        assert!(chain.len() <= 8);
        assert_eq!(ancestors(me, 1).len(), 1);
        assert!(ancestors(me, 0).is_empty());
        let name = image_name(me).expect("the test binary has a name");
        assert!(!name.is_empty());
        assert!(!name.contains('/') && !name.contains('\\'));
    }

    #[test]
    fn a_pid_nobody_has_is_dead_and_nameless() {
        // Above every pid limit: macOS stops at 99998, Windows pids fit in a DWORD but stay far
        // lower, Linux allows up to 4194304.
        let nobody = 4_194_304 + 7;
        assert!(!is_alive(nobody));
        assert_eq!(start_time(nobody), None);
        assert_eq!(parent(nobody), None);
        assert_eq!(image_name(nobody), None);
        assert!(ancestors(nobody, 8).is_empty());
    }
}
