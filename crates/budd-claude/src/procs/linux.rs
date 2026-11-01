//! Linux: `/proc/<pid>/stat` for the parent and the start time, `/proc/<pid>/comm` for the name.

use std::fs;
use std::sync::OnceLock;

/// The fields of `/proc/<pid>/stat` after the command name, which is in parentheses and may hold spaces.
fn stat_fields(pid: u32) -> Option<Vec<String>> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = text.rsplit_once(')')?.1;
    Some(after_name.split_whitespace().map(str::to_owned).collect())
}

/// When the system booted, in seconds since the Unix epoch, from the `btime` line of `/proc/stat`.
fn boot_time() -> Option<u64> {
    static BOOT: OnceLock<Option<u64>> = OnceLock::new();
    *BOOT.get_or_init(|| {
        let text = fs::read_to_string("/proc/stat").ok()?;
        text.lines()
            .find_map(|line| line.strip_prefix("btime "))
            .and_then(|value| value.trim().parse().ok())
    })
}

/// Clock ticks per second, which `starttime` counts in.
fn ticks_per_second() -> Option<u64> {
    // SAFETY: sysconf takes a name and returns a value; it touches no memory of ours.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    u64::try_from(ticks).ok().filter(|&ticks| ticks > 0)
}

/// When `pid` started, in seconds since the Unix epoch: boot time plus its `starttime` in ticks.
pub fn start_time(pid: u32) -> Option<u64> {
    let fields = stat_fields(pid)?;
    // Field 22 of the whole line is `starttime`; after "(comm)" come fields 3 onward, so it is index 19.
    let ticks: u64 = fields.get(19)?.parse().ok()?;
    Some(boot_time()? + ticks / ticks_per_second()?)
}

/// The parent of `pid`: field 4 of the stat line, index 1 after the command name.
pub fn parent(pid: u32) -> Option<u32> {
    stat_fields(pid)?.get(1)?.parse().ok()
}

/// The program's file name: the last part of `/proc/<pid>/exe`, else the short `comm` name.
pub fn image_name(pid: u32) -> Option<String> {
    if let Ok(exe) = fs::read_link(format!("/proc/{pid}/exe"))
        && let Some(name) = exe.file_name()
    {
        return Some(name.to_string_lossy().into_owned());
    }
    let comm = fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let comm = comm.trim();
    (!comm.is_empty()).then(|| comm.to_owned())
}
