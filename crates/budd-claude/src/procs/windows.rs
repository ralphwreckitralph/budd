//! Windows: a limited-information process handle for liveness, times and the image path; a Toolhelp
//! snapshot for the parent, which is the documented way to learn it.

use std::mem::size_of;
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, STILL_ACTIVE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};

/// A process handle that closes itself.
struct Process(HANDLE);

impl Process {
    /// Opens `pid` for queries, or `None` when there is no such process or it may not be opened.
    fn open(pid: u32) -> Option<Process> {
        // SAFETY: OpenProcess takes plain values and returns a handle or null; nothing of ours is borrowed.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        Some(Process(handle))
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenProcess and is closed exactly once, here.
        unsafe { CloseHandle(self.0) };
    }
}

/// Whether `pid` is running: it can be opened and its exit code is still "active".
pub fn is_alive(pid: u32) -> bool {
    let Some(process) = Process::open(pid) else {
        return false;
    };
    let mut code: u32 = 0;
    // SAFETY: the handle is open and `code` is a valid u32 for the call to write.
    let ok = unsafe { GetExitCodeProcess(process.0, &mut code) };
    ok != 0 && code == STILL_ACTIVE as u32
}

/// A FILETIME (100 ns ticks since 1601) as whole seconds since the Unix epoch.
fn unix_seconds(time: FILETIME) -> u64 {
    let ticks = ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64;
    // Seconds between 1601-01-01 and 1970-01-01.
    const EPOCH_GAP: u64 = 11_644_473_600;
    (ticks / 10_000_000).saturating_sub(EPOCH_GAP)
}

/// When `pid` started, in seconds since the Unix epoch, from its creation time.
pub fn start_time(pid: u32) -> Option<u64> {
    let process = Process::open(pid)?;
    let empty = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut creation, mut exit, mut kernel, mut user) = (empty, empty, empty, empty);
    // SAFETY: the handle is open and the four FILETIMEs are valid for the call to write.
    let ok = unsafe { GetProcessTimes(process.0, &mut creation, &mut exit, &mut kernel, &mut user) };
    (ok != 0).then(|| unix_seconds(creation))
}

/// Whether `pid` started after `other` did, when both times are known; a parent pid that started after
/// its child was recycled.
pub fn started_after(pid: u32, other: u32) -> bool {
    match (start_time(pid), start_time(other)) {
        (Some(mine), Some(theirs)) => mine > theirs,
        _ => false,
    }
}

/// The parent of `pid`, from a snapshot of the process table.
pub fn parent(pid: u32) -> Option<u32> {
    // SAFETY: a snapshot handle or INVALID_HANDLE_VALUE comes back; nothing of ours is borrowed.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    let snapshot = Process(snapshot);
    // SAFETY: PROCESSENTRY32W is plain data; zeroed is a valid starting value once dwSize is set.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: the snapshot is open and `entry` has its size set, as the walk requires.
    let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
    while more {
        if entry.th32ProcessID == pid {
            return Some(entry.th32ParentProcessID);
        }
        // SAFETY: same handle and entry as above.
        more = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
    }
    None
}

/// The program's file name: the last part of its full image path.
pub fn image_name(pid: u32) -> Option<String> {
    let process = Process::open(pid)?;
    let mut buffer = vec![0u16; 32 * 1024];
    let mut length = buffer.len() as u32;
    // SAFETY: the buffer holds `length` u16s for the call to write; it writes the path and its length.
    let ok = unsafe { QueryFullProcessImageNameW(process.0, 0, buffer.as_mut_ptr(), &mut length) };
    if ok == 0 {
        return None;
    }
    let path = String::from_utf16_lossy(&buffer[..length as usize]);
    path.rsplit(['\\', '/'])
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}
