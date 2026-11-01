//! What every Unix shares: liveness by `kill(pid, 0)`.

/// Whether `pid` is running: the null signal is delivered (0) or refused for lack of permission (EPERM),
/// either of which means the process exists. ESRCH means it does not.
pub fn is_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill with signal 0 only checks that the process exists; it sends nothing.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
