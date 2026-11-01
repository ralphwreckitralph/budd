//! The local endpoint between the daemon and its callers: a Unix socket or a Windows named pipe, one JSON line each way.
//!
//! One connection carries one request: the caller writes a line and reads a line back. Only the
//! human's own processes can open the endpoint: the socket file is mode 0600 in a 0700 folder and every
//! peer's uid is checked on accept; the pipe's security descriptor admits the current user and SYSTEM
//! and refuses remote clients. The wire format is the macOS app's, so on macOS `BUDD_SOCKET` can point at
//! its socket and the same lines cross.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The longest line read from a connection, well over the longest tool call Budd accepts.
pub const LONGEST_LINE: usize = 64 * 1024;
/// How long the server waits for a connection's line, and for its answer to be taken.
pub const SERVE_TIMEOUT: Duration = Duration::from_secs(5);
/// The variable that moves the endpoint, for tests and for talking to the macOS app's socket.
pub const SOCKET_VAR: &str = "BUDD_SOCKET";
/// Every Windows named pipe's name starts with this.
pub const PIPE_PREFIX: &str = r"\\.\pipe\";

/// Where the daemon listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// A Unix domain socket at this path.
    UnixSocket(PathBuf),
    /// A Windows named pipe with this full name, `\\.\pipe\...`.
    NamedPipe(String),
}

impl Endpoint {
    /// The endpoint `text` names: a pipe when it starts with `\\.\pipe\`, else a socket path.
    pub fn parse(text: &str) -> Endpoint {
        if text.starts_with(PIPE_PREFIX) {
            Endpoint::NamedPipe(text.to_owned())
        } else {
            Endpoint::UnixSocket(PathBuf::from(text))
        }
    }

    /// Budd's own endpoint: `<home>/run/budd.sock` on Unix, `\\.\pipe\budd-<user>` on Windows.
    pub fn default_for(home: &Path, user: &str) -> Endpoint {
        if cfg!(windows) {
            Endpoint::NamedPipe(format!("{PIPE_PREFIX}budd-{user}"))
        } else {
            Endpoint::UnixSocket(home.join("run").join("budd.sock"))
        }
    }

    /// The endpoint the environment names in `BUDD_SOCKET`, else Budd's own under the Budd home.
    pub fn from_env(lookup: impl Fn(&str) -> Option<String>) -> Endpoint {
        if let Some(text) = lookup(SOCKET_VAR).filter(|text| !text.trim().is_empty()) {
            return Endpoint::parse(text.trim());
        }
        let home = crate::hookrun::budd_home(&lookup);
        let user = lookup("USERNAME")
            .or_else(|| lookup("USER"))
            .filter(|user| !user.is_empty())
            .unwrap_or_else(|| "user".to_owned());
        Endpoint::default_for(&home, &user)
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Endpoint::UnixSocket(path) => write!(f, "{}", path.display()),
            Endpoint::NamedPipe(name) => f.write_str(name),
        }
    }
}

/// Sends `line` to the endpoint and returns the line that comes back, without its newline. Connecting,
/// writing and reading each give up after `timeout`. An endpoint of the other platform's kind fails
/// with `Unsupported`; one with nobody listening with `NotFound` or `ConnectionRefused`.
pub fn call(endpoint: &Endpoint, line: &str, timeout: Duration) -> io::Result<String> {
    match endpoint {
        Endpoint::UnixSocket(path) => {
            #[cfg(unix)]
            {
                unix::call(path, line, timeout)
            }
            #[cfg(not(unix))]
            {
                let _ = (path, line, timeout);
                Err(unsupported("a Unix socket"))
            }
        }
        Endpoint::NamedPipe(name) => {
            #[cfg(windows)]
            {
                windows::call(name, line, timeout)
            }
            #[cfg(not(windows))]
            {
                let _ = (name, line, timeout);
                Err(unsupported("a named pipe"))
            }
        }
    }
}

/// Whether a failed `call` means nobody is listening, as opposed to a listener that misbehaved.
pub fn is_not_running(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused)
}

fn unsupported(kind: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("{kind} is not an endpoint on this platform"),
    )
}

/// The daemon's end: bound to one endpoint, answering one line per connection.
pub struct Listener {
    endpoint: Endpoint,
    #[cfg(unix)]
    inner: unix::Listener,
    #[cfg(windows)]
    inner: windows::Listener,
}

impl fmt::Debug for Listener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Listener").field("endpoint", &self.endpoint).finish()
    }
}

impl Listener {
    /// Binds `endpoint`, making it reachable by this user alone. On Unix the parent folder is created
    /// mode 0700, a stale socket file of this user's is replaced, and anything else at the path is
    /// refused; on Windows the pipe is created as the first instance, so a second Budd is refused.
    pub fn bind(endpoint: &Endpoint) -> io::Result<Listener> {
        let inner = match endpoint {
            Endpoint::UnixSocket(path) => {
                #[cfg(unix)]
                {
                    unix::Listener::bind(path)?
                }
                #[cfg(not(unix))]
                {
                    let _ = path;
                    return Err(unsupported("a Unix socket"));
                }
            }
            Endpoint::NamedPipe(name) => {
                #[cfg(windows)]
                {
                    windows::Listener::bind(name)?
                }
                #[cfg(not(windows))]
                {
                    let _ = name;
                    return Err(unsupported("a named pipe"));
                }
            }
        };
        Ok(Listener {
            endpoint: endpoint.clone(),
            inner,
        })
    }

    /// Where this listener is bound.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Changes how long a connection may take to send its line and take its answer; [`SERVE_TIMEOUT`] by default.
    pub fn set_serve_timeout(&mut self, timeout: Duration) {
        self.inner.set_serve_timeout(timeout);
    }

    /// Waits for one connection, reads its line, answers with what `handler` returns, and closes it.
    /// A connection from another user is closed unanswered with `PermissionDenied`; one that sends
    /// nothing in time fails with a timeout. Either way the listener is ready for the next call.
    pub fn accept_line(&self, handler: impl FnMut(&str) -> String) -> io::Result<()> {
        self.inner.accept_line(handler)
    }
}

/// Reads up to the first newline, or the end, whichever comes first, capped at [`LONGEST_LINE`]; the
/// newline is dropped. Nothing at all is an `UnexpectedEof`.
pub(crate) fn read_line(reader: &mut dyn Read) -> io::Result<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    while bytes.len() < LONGEST_LINE {
        let count = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(newline) = bytes.iter().position(|&b| b == b'\n') {
            bytes.truncate(newline);
            return Ok(String::from_utf8_lossy(&bytes).into_owned());
        }
    }
    if bytes.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the connection sent nothing",
        ));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Writes `line` and a newline, unless it ends with one already.
pub(crate) fn write_line(writer: &mut dyn Write, line: &str) -> io::Result<()> {
    writer.write_all(line.as_bytes())?;
    if !line.ends_with('\n') {
        writer.write_all(b"\n")?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_are_named_by_kind() {
        assert_eq!(
            Endpoint::parse(r"\\.\pipe\budd-alice"),
            Endpoint::NamedPipe(r"\\.\pipe\budd-alice".to_owned())
        );
        assert_eq!(
            Endpoint::parse("/tmp/budd.sock"),
            Endpoint::UnixSocket(PathBuf::from("/tmp/budd.sock"))
        );
        let own = Endpoint::default_for(Path::new("/home/alice/.budd"), "alice");
        if cfg!(windows) {
            assert_eq!(own, Endpoint::NamedPipe(r"\\.\pipe\budd-alice".to_owned()));
        } else {
            assert_eq!(
                own,
                Endpoint::UnixSocket(PathBuf::from("/home/alice/.budd/run/budd.sock"))
            );
        }
        assert!(!own.to_string().is_empty());
        let from = Endpoint::from_env(|key| (key == SOCKET_VAR).then(|| " /var/run/b.sock ".to_owned()));
        assert_eq!(from, Endpoint::UnixSocket(PathBuf::from("/var/run/b.sock")));
        let lookup = |key: &str| match key {
            "BUDD_HOME" => Some("/opt/budd".to_owned()),
            "USER" | "USERNAME" => Some("bob".to_owned()),
            _ => None,
        };
        assert_eq!(
            Endpoint::from_env(lookup),
            Endpoint::default_for(Path::new("/opt/budd"), "bob")
        );
    }

    #[test]
    fn lines_are_read_to_the_newline_or_the_end() {
        let mut short: &[u8] = b"hello\nworld\n";
        assert_eq!(read_line(&mut short).unwrap(), "hello");
        let mut bare: &[u8] = b"no newline";
        assert_eq!(read_line(&mut bare).unwrap(), "no newline");
        let mut empty: &[u8] = b"";
        assert_eq!(read_line(&mut empty).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
        let long = vec![b'x'; LONGEST_LINE + 10];
        let mut long_reader: &[u8] = &long;
        assert!(read_line(&mut long_reader).unwrap().len() <= LONGEST_LINE + 4096);
        let mut out = Vec::new();
        write_line(&mut out, "a").unwrap();
        write_line(&mut out, "b\n").unwrap();
        assert_eq!(out, b"a\nb\n");
    }

    #[cfg(unix)]
    mod on_unix {
        use super::*;
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::{UnixListener, UnixStream};
        use std::thread;

        #[test]
        fn a_bound_socket_answers_each_call_on_its_own_connection() {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("run").join("budd.sock");
            let endpoint = Endpoint::UnixSocket(path.clone());
            let listener = Listener::bind(&endpoint).unwrap();
            assert_eq!(listener.endpoint(), &endpoint);
            let dir_mode = std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777;
            assert_eq!(dir_mode, 0o700);
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            let server = thread::spawn(move || {
                let mut count = 0;
                for _ in 0..2 {
                    listener
                        .accept_line(|line| {
                            count += 1;
                            format!("{count}:{line}")
                        })
                        .unwrap();
                }
                listener
            });
            assert_eq!(call(&endpoint, "ping", Duration::from_secs(2)).unwrap(), "1:ping");
            assert_eq!(
                call(&endpoint, "{\"tool\":\"say\"}\n", Duration::from_secs(2)).unwrap(),
                "2:{\"tool\":\"say\"}"
            );
            let listener = server.join().unwrap();
            drop(listener);
            // The socket file goes with the listener.
            assert!(!path.exists());
        }

        #[test]
        fn nobody_listening_reads_as_not_running() {
            let temp = tempfile::tempdir().unwrap();
            let missing = Endpoint::UnixSocket(temp.path().join("none.sock"));
            let error = call(&missing, "x", Duration::from_secs(1)).unwrap_err();
            assert!(is_not_running(&error), "{error}");
            // A socket file whose server is gone refuses the connection.
            let stale = temp.path().join("stale.sock");
            drop(UnixListener::bind(&stale).unwrap());
            let error = call(&Endpoint::UnixSocket(stale), "x", Duration::from_secs(1)).unwrap_err();
            assert!(is_not_running(&error), "{error}");
            assert!(!is_not_running(&io::Error::new(io::ErrorKind::TimedOut, "slow")));
            let pipe = Endpoint::NamedPipe(r"\\.\pipe\budd".to_owned());
            assert_eq!(
                call(&pipe, "x", Duration::from_secs(1)).unwrap_err().kind(),
                io::ErrorKind::Unsupported
            );
            assert_eq!(Listener::bind(&pipe).unwrap_err().kind(), io::ErrorKind::Unsupported);
        }

        #[test]
        fn a_stale_socket_of_ours_is_replaced_but_anything_else_is_refused() {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("budd.sock");
            drop(UnixListener::bind(&path).unwrap());
            assert!(path.exists());
            let listener = Listener::bind(&Endpoint::UnixSocket(path.clone())).unwrap();
            drop(listener);
            // A plain file at the path is not ours to remove.
            std::fs::write(&path, "not a socket").unwrap();
            let error = Listener::bind(&Endpoint::UnixSocket(path.clone())).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "not a socket");
            // Nor is a folder.
            let folder = temp.path().join("dir.sock");
            std::fs::create_dir(&folder).unwrap();
            assert_eq!(
                Listener::bind(&Endpoint::UnixSocket(folder)).unwrap_err().kind(),
                io::ErrorKind::AlreadyExists
            );
        }

        #[test]
        fn a_silent_connection_times_out_and_the_next_is_served() {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("budd.sock");
            let endpoint = Endpoint::UnixSocket(path.clone());
            let mut listener = Listener::bind(&endpoint).unwrap();
            listener.set_serve_timeout(Duration::from_millis(200));
            let server = thread::spawn(move || {
                let first = listener.accept_line(|_| "never".to_owned());
                let second = listener.accept_line(|line| format!("ok:{line}"));
                (first, second)
            });
            let silent = UnixStream::connect(&path).unwrap();
            thread::sleep(Duration::from_millis(400));
            assert_eq!(call(&endpoint, "hello", Duration::from_secs(2)).unwrap(), "ok:hello");
            drop(silent);
            let (first, second) = server.join().unwrap();
            let kind = first.unwrap_err().kind();
            assert!(
                matches!(kind, io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut),
                "{kind:?}"
            );
            second.unwrap();
        }
    }
}
