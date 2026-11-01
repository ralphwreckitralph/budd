//! Unix: a stream socket file, mode 0600 in a 0700 folder, with the peer's uid checked on accept.

use super::{read_line, write_line};
use std::fs;
use std::io;
use std::net::Shutdown;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// This process's effective user id.
fn my_uid() -> u32 {
    // SAFETY: geteuid takes nothing and cannot fail.
    unsafe { libc::geteuid() }
}

/// Connects to the socket at `path`, sends `line`, and reads the answer.
pub fn call(path: &Path, line: &str, timeout: Duration) -> io::Result<String> {
    let mut stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    write_line(&mut stream, line)?;
    let _ = stream.shutdown(Shutdown::Write);
    read_line(&mut stream)
}

/// A bound socket that removes its file when dropped.
pub struct Listener {
    inner: UnixListener,
    path: PathBuf,
    timeout: Duration,
}

impl Listener {
    /// Binds `path`: the folder above it is created and made 0700, a stale socket of this user's is
    /// removed, anything else at the path is refused, and the new socket is made 0600.
    pub fn bind(path: &Path) -> io::Result<Listener> {
        let parent = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        fs::create_dir_all(parent)?;
        if fs::metadata(parent)?.uid() == my_uid() {
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        match fs::symlink_metadata(path) {
            Ok(existing) => {
                if !existing.file_type().is_socket() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("{} exists and is not a socket", path.display()),
                    ));
                }
                if existing.uid() != my_uid() {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("the socket {} belongs to another user", path.display()),
                    ));
                }
                fs::remove_file(path)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let inner = UnixListener::bind(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(Listener {
            inner,
            path: path.to_path_buf(),
            timeout: super::SERVE_TIMEOUT,
        })
    }

    /// Changes how long a connection may take.
    pub fn set_serve_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Accepts one connection and serves one line, as the public `Listener::accept_line` says.
    pub fn accept_line(&self, mut handler: impl FnMut(&str) -> String) -> io::Result<()> {
        let (mut stream, _) = self.inner.accept()?;
        let peer = peer_uid(&stream)?;
        if peer != my_uid() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("a connection from uid {peer} was refused"),
            ));
        }
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        let line = read_line(&mut stream)?;
        let answer = handler(&line);
        write_line(&mut stream, &answer)?;
        let _ = stream.shutdown(Shutdown::Both);
        Ok(())
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// The effective uid of the process at the other end of `stream`.
#[cfg(target_os = "macos")]
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    // SAFETY: xucred is plain data, so zeroed is a valid value for the kernel to overwrite.
    let mut cred: libc::xucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::xucred>() as libc::socklen_t;
    // SAFETY: the buffer is an xucred of exactly `length` bytes, which LOCAL_PEERCRED fills.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERCRED,
            (&raw mut cred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred.cr_uid)
}

/// The effective uid of the process at the other end of `stream`.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    // SAFETY: ucred is plain data, so zeroed is a valid value for the kernel to overwrite.
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the buffer is a ucred of exactly `length` bytes, which SO_PEERCRED fills.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred.uid)
}

/// The effective uid of the process at the other end of `stream`, on the other BSDs.
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    unsafe extern "C" {
        fn getpeereid(socket: libc::c_int, euid: *mut libc::uid_t, egid: *mut libc::gid_t) -> libc::c_int;
    }
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: getpeereid writes one uid and one gid to the two valid pointers given.
    let result = unsafe { getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}
