//! macOS: one `proc_pidinfo(PROC_PIDTBSDINFO)` call gives the parent, the start time and the name.
//!
//! This is the same record `sysctl(KERN_PROC_PID)` fills (`kinfo_proc`), through the supported libproc
//! entry point, whose struct the `libc` crate declares; `kinfo_proc` it does not.

use std::ffi::CStr;
use std::mem::{MaybeUninit, size_of};

/// The BSD process record of `pid`, or `None` when there is no such process or it may not be read.
fn info(pid: u32) -> Option<libc::proc_bsdinfo> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut record = MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = libc::c_int::try_from(size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: the buffer is a proc_bsdinfo of exactly the size passed, which is the record this flavor
    // writes; the kernel fills it and returns the bytes written, and we only read it after checking that
    // the whole record came back.
    let written = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, record.as_mut_ptr().cast(), size) };
    if written != size {
        return None;
    }
    // SAFETY: the call returned the full size, so every field of the record was written.
    Some(unsafe { record.assume_init() })
}

/// When `pid` started, in seconds since the Unix epoch.
pub fn start_time(pid: u32) -> Option<u64> {
    info(pid).map(|record| record.pbi_start_tvsec)
}

/// The parent of `pid`.
pub fn parent(pid: u32) -> Option<u32> {
    info(pid).map(|record| record.pbi_ppid)
}

/// The program's file name: the last part of its path, else the short command name the kernel keeps.
pub fn image_name(pid: u32) -> Option<String> {
    let record = info(pid)?;
    let id = libc::c_int::try_from(pid).ok()?;
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let capacity = u32::try_from(buffer.len()).ok()?;
    // SAFETY: the buffer is writable for exactly `capacity` bytes, as passed; proc_pidpath writes a
    // NUL-terminated path within it and returns its length, or 0 on failure.
    let length = unsafe { libc::proc_pidpath(id, buffer.as_mut_ptr().cast(), capacity) };
    if length > 0 {
        let path = String::from_utf8_lossy(&buffer[..length as usize]).into_owned();
        if let Some(name) = path.rsplit('/').next().filter(|name| !name.is_empty()) {
            return Some(name.to_owned());
        }
    }
    // SAFETY: pbi_name is a fixed array the kernel NUL-terminates; from_ptr reads within it as long as
    // the terminator is there, which we make sure of by copying and forcing a terminator first.
    let mut name = record.pbi_name;
    name[name.len() - 1] = 0;
    let short = unsafe { CStr::from_ptr(name.as_ptr()) }.to_string_lossy().into_owned();
    (!short.is_empty()).then_some(short)
}
