//! Windows: a named pipe whose security descriptor admits the current user and SYSTEM only, created as
//! the first instance so a second server is refused, and closed to remote clients.
//!
//! Pipe reads and writes here are synchronous without a deadline: a timeout on them needs overlapped
//! I/O or `CancelSynchronousIo`, which this crate does not link yet. The client side still gives up
//! waiting for a busy pipe after its timeout.

use super::{read_line, write_line};
use std::cell::Cell;
use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::ptr;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

// windows-sys gates ConnectNamedPipe behind its Win32_System_IO feature, for the OVERLAPPED type of its
// second argument, which this crate does not enable. Only a null OVERLAPPED is ever passed here, so the
// pointer type can be opaque; the ABI is the same.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn ConnectNamedPipe(hnamedpipe: HANDLE, lpoverlapped: *mut c_void) -> i32;
}

/// `text` as a NUL-terminated UTF-16 string.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Opens the pipe `name`, sends `line`, and reads the answer. A busy pipe is waited for up to `timeout`.
pub fn call(name: &str, line: &str, timeout: Duration) -> io::Result<String> {
    let path = wide(name);
    let deadline = Instant::now() + timeout;
    let handle = loop {
        // SAFETY: the name is a valid NUL-terminated wide string and every other argument is a plain value
        // or null, as CreateFileW allows.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            break handle;
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_PIPE_BUSY as i32) {
            return Err(error);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "the pipe stayed busy"));
        }
        // SAFETY: same valid name; the timeout is a plain value.
        unsafe { WaitNamedPipeW(path.as_ptr(), left.as_millis().min(u32::MAX as u128) as u32) };
    };
    // SAFETY: the handle is open and owned by nobody else; the File closes it.
    let mut file = unsafe { File::from_raw_handle(handle as _) };
    write_line(&mut file, line)?;
    read_line(&mut file)
}

/// A security descriptor from SDDL, freed when dropped.
struct Descriptor(*mut c_void);

impl Descriptor {
    /// A descriptor admitting the current user and SYSTEM, with full access, and nobody else.
    fn for_current_user() -> io::Result<Descriptor> {
        let sid = current_user_sid()?;
        let sddl = wide(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})"));
        let mut descriptor: *mut c_void = ptr::null_mut();
        // SAFETY: the SDDL is a valid NUL-terminated wide string; the out pointer is valid; the size
        // pointer may be null, as documented.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if ok == 0 || descriptor.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Descriptor(descriptor))
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: the descriptor was allocated by ConvertStringSecurityDescriptor..., which documents LocalFree.
        unsafe { LocalFree(self.0) };
    }
}

/// The current user's SID as text, such as "S-1-5-21-...".
fn current_user_sid() -> io::Result<String> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: the process pseudo-handle needs no closing; the out pointer is valid.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    struct Token(HANDLE);
    impl Drop for Token {
        fn drop(&mut self) {
            // SAFETY: the token came from OpenProcessToken and is closed once, here.
            unsafe { CloseHandle(self.0) };
        }
    }
    let token = Token(token);
    let mut needed: u32 = 0;
    // SAFETY: a null buffer of length 0 asks for the size, which is written to `needed`.
    unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    // SAFETY: the buffer holds `needed` bytes, 8-byte aligned, as TOKEN_USER requires.
    let ok = unsafe { GetTokenInformation(token.0, TokenUser, buffer.as_mut_ptr().cast(), needed, &mut needed) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call filled the buffer with a TOKEN_USER whose Sid points inside the same buffer.
    let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
    let mut text: *mut u16 = ptr::null_mut();
    // SAFETY: the SID pointer is the one the token gave; the out pointer is valid.
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 || text.is_null() {
        return Err(io::Error::last_os_error());
    }
    let mut length = 0;
    // SAFETY: the string is NUL-terminated by the call; we walk to the terminator.
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: `length` u16s are readable before the terminator.
    let sid = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
    // SAFETY: the string was allocated by ConvertSidToStringSidW, which documents LocalFree.
    unsafe { LocalFree(text.cast()) };
    Ok(sid)
}

/// A named pipe server: the next instance waiting for a client, created ahead so the name is never free.
pub struct Listener {
    name: Vec<u16>,
    descriptor: Descriptor,
    waiting: Cell<HANDLE>,
}

// SAFETY: a pipe handle and a security descriptor are process-wide kernel objects, usable from any
// thread; the Cell is only touched by whichever thread calls accept_line, one at a time, as &self plus
// the lack of Sync guarantees.
unsafe impl Send for Listener {}

impl Listener {
    /// Creates the pipe's first instance, refusing a name another server already holds.
    pub fn bind(name: &str) -> io::Result<Listener> {
        let listener = Listener {
            name: wide(name),
            descriptor: Descriptor::for_current_user()?,
            waiting: Cell::new(INVALID_HANDLE_VALUE),
        };
        let first = listener.instance(true)?;
        listener.waiting.set(first);
        Ok(listener)
    }

    /// Synchronous pipes have no read deadline; the timeout is kept for the client's wait only.
    pub fn set_serve_timeout(&mut self, _timeout: Duration) {}

    /// One new pipe instance under the listener's security descriptor.
    fn instance(&self, first: bool) -> io::Result<HANDLE> {
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.0,
            bInheritHandle: 0,
        };
        let mut open_mode = PIPE_ACCESS_DUPLEX;
        if first {
            open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
        }
        // SAFETY: the name is a valid NUL-terminated wide string, the attributes point at a descriptor
        // that outlives the call, and the rest are plain values.
        let handle = unsafe {
            CreateNamedPipeW(
                self.name.as_ptr(),
                open_mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                super::LONGEST_LINE as u32,
                super::LONGEST_LINE as u32,
                0,
                &mut attributes,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(handle)
    }

    /// Waits for one client on the waiting instance, serves its line, and leaves a fresh instance waiting.
    pub fn accept_line(&self, mut handler: impl FnMut(&str) -> String) -> io::Result<()> {
        let handle = self.waiting.get();
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "the pipe is closed"));
        }
        // SAFETY: the handle is an open pipe instance; a null OVERLAPPED means a blocking wait.
        let connected = unsafe { ConnectNamedPipe(handle, ptr::null_mut()) } != 0
            || io::Error::last_os_error().raw_os_error() == Some(ERROR_PIPE_CONNECTED as i32);
        if !connected {
            return Err(io::Error::last_os_error());
        }
        // The next client must find an instance even while this one is served.
        match self.instance(false) {
            Ok(next) => self.waiting.set(next),
            Err(_) => self.waiting.set(INVALID_HANDLE_VALUE),
        }
        // SAFETY: the handle is open and owned by nobody else; the File closes it after the disconnect.
        let mut file = unsafe { File::from_raw_handle(handle as _) };
        let served = (|| {
            let line = read_line(&mut file)?;
            let answer = handler(&line);
            write_line(&mut file, &answer)?;
            // SAFETY: the file's handle is the open pipe instance.
            unsafe { FlushFileBuffers(file.as_raw_handle() as _) };
            Ok(())
        })();
        // SAFETY: same open pipe instance; disconnecting before the close lets the client read the answer.
        unsafe { DisconnectNamedPipe(file.as_raw_handle() as _) };
        served
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let handle = self.waiting.get();
        if handle != INVALID_HANDLE_VALUE {
            // SAFETY: the waiting instance is open and closed exactly once, here.
            unsafe { CloseHandle(handle) };
        }
    }
}
