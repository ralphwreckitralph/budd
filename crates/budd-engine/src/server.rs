//! The daemon's end of the endpoint: a tokio listener, one task per connection, and the bookkeeping
//! that routes replies and events to the right clients.
//!
//! A connection may be long-lived (the app: `hello`, then requests and events for as long as it stays)
//! or one-shot (`budd mcp`: one bare tool call, one answer). Both speak JSON lines. On Unix the
//! endpoint is a socket file, mode 0600 in a 0700 folder, and a peer of another uid is dropped; on
//! Windows a named pipe whose security descriptor admits the current user and SYSTEM alone, closed to
//! remote clients, the same descriptor `budd_claude::ipc` builds for the blocking pipe.

use crate::engine::{ClientId, Outgoing};
use budd_claude::ipc::Endpoint;
use budd_core::tools::ToolResult;
use budd_proto::{ClientLine, Command, Request, Topic};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// The longest line a client may send: well over the longest tool call Budd accepts.
pub const LONGEST_LINE: usize = 1024 * 1024;

/// A connection's bytes, whichever kind of endpoint it came through.
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// What a connection task tells the engine's task.
#[derive(Debug)]
pub enum Inbound {
    /// A client connected; its lines go out through `lines`.
    Connected {
        /// Which client.
        client: ClientId,
        /// Where to send it lines.
        lines: mpsc::UnboundedSender<String>,
    },
    /// A client sent a line that parsed.
    Line {
        /// Which client.
        client: ClientId,
        /// What it sent.
        line: ClientLine,
    },
    /// A client went away.
    Disconnected {
        /// Which client.
        client: ClientId,
    },
}

/// The listener on the endpoint.
pub struct Server {
    inner: platform::Listener,
}

impl Server {
    /// Binds `endpoint`, making it reachable by this user alone.
    pub fn bind(endpoint: &Endpoint) -> io::Result<Server> {
        Ok(Server {
            inner: platform::Listener::bind(endpoint)?,
        })
    }

    /// Waits for the next connection from this user. One from anyone else is dropped and the wait goes on.
    pub async fn accept(&mut self) -> io::Result<Box<dyn Stream>> {
        loop {
            match self.inner.accept().await? {
                Some(stream) => return Ok(stream),
                None => tracing::warn!("a connection from another user was refused"),
            }
        }
    }
}

/// Serves one connection until it closes: lines in go to the engine through `to_engine`, lines out
/// come from the channel handed over in `Inbound::Connected`. A line that does not parse is answered
/// on the spot with `{"ok":false,"error":...}`.
pub async fn serve_connection(stream: Box<dyn Stream>, client: ClientId, to_engine: mpsc::UnboundedSender<Inbound>) {
    let (reader, mut writer) = tokio::io::split(stream);
    let (lines, mut outgoing) = mpsc::unbounded_channel::<String>();
    if to_engine
        .send(Inbound::Connected {
            client,
            lines: lines.clone(),
        })
        .is_err()
    {
        return;
    }
    let writing = tokio::spawn(async move {
        while let Some(line) = outgoing.recv().await {
            if writer.write_all(line.as_bytes()).await.is_err() || writer.write_all(b"\n").await.is_err() {
                break;
            }
            if writer.flush().await.is_err() {
                break;
            }
        }
        let _ = writer.shutdown().await;
    });
    let mut reader = BufReader::new(reader);
    let mut buffer = String::new();
    loop {
        buffer.clear();
        let read = (&mut reader).take(LONGEST_LINE as u64).read_line(&mut buffer).await;
        match read {
            Ok(0) | Err(_) => break,
            Ok(_) if !buffer.ends_with('\n') && buffer.len() >= LONGEST_LINE => {
                let _ = lines.send(json!({"ok": false, "error": "the line is too long"}).to_string());
                break;
            }
            Ok(_) => {}
        }
        let text = buffer.trim();
        if text.is_empty() {
            continue;
        }
        match ClientLine::parse(text) {
            Ok(line) => {
                if to_engine.send(Inbound::Line { client, line }).is_err() {
                    break;
                }
            }
            Err(error) => {
                let _ = lines.send(json!({"ok": false, "error": error}).to_string());
            }
        }
    }
    let _ = to_engine.send(Inbound::Disconnected { client });
    drop(lines);
    // A one-shot caller (`budd mcp`) closes its writing side before reading the answer, so the writer
    // stays until the router lets go of the channel; a router that never answers is cut off in time.
    if tokio::time::timeout(LAST_ANSWER, writing).await.is_err() {
        tracing::warn!(?client, "a connection was closed before its answer came");
    }
}

/// How long a closed connection waits for the answer to its last line.
const LAST_ANSWER: std::time::Duration = std::time::Duration::from_secs(15);

/// One connected client as the router knows it.
struct Client {
    lines: mpsc::UnboundedSender<String>,
    /// What it subscribed to; `None` until its `hello` is answered.
    topics: Option<HashSet<Topic>>,
    /// The `hello` sent and not yet answered: its id and the topics it asked for.
    pending_hello: Option<(u64, HashSet<Topic>)>,
    /// Requests that came as bare tool calls, answered as bare tool results.
    bare: HashSet<u64>,
    /// The connection closed its reading side; it is kept only until its bare calls are answered.
    gone: bool,
}

/// Routes the engine's lines to the clients, and turns the clients' lines into requests.
#[derive(Default)]
pub struct Clients {
    clients: HashMap<ClientId, Client>,
    next_bare: u64,
}

/// Bare tool calls carry no id; the router lends them one from here up, out of any client's way.
const BARE_IDS: u64 = 1 << 40;

impl Clients {
    /// A router with nobody connected.
    pub fn new() -> Clients {
        Clients {
            clients: HashMap::new(),
            next_bare: BARE_IDS,
        }
    }

    /// How many clients are connected.
    pub fn len(&self) -> usize {
        self.clients.len()
    }

    /// Whether nobody is connected.
    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }

    /// Takes what a connection task said: a new client, a line, or a goodbye. A line becomes the
    /// request the engine takes, or `None` when there is nothing for the engine in it.
    pub fn inbound(&mut self, inbound: Inbound) -> Option<(ClientId, Request)> {
        match inbound {
            Inbound::Connected { client, lines } => {
                self.clients.insert(
                    client,
                    Client {
                        lines,
                        topics: None,
                        pending_hello: None,
                        bare: HashSet::new(),
                        gone: false,
                    },
                );
                None
            }
            Inbound::Disconnected { client } => {
                // A one-shot caller closes its writing side before reading the answer: keep it until then.
                match self.clients.get_mut(&client) {
                    Some(state) if !state.bare.is_empty() => state.gone = true,
                    _ => {
                        self.clients.remove(&client);
                    }
                }
                None
            }
            Inbound::Line { client, line } => {
                let state = self.clients.get_mut(&client)?;
                match line {
                    ClientLine::Request(request) => {
                        if let (Some(id), Command::Hello { subscribe, .. }) = (request.id, &request.command) {
                            let topics = subscribe
                                .clone()
                                .map(|topics| topics.into_iter().collect())
                                .unwrap_or_else(|| Topic::ALL.into_iter().collect());
                            state.pending_hello = Some((id, topics));
                        }
                        Some((client, request))
                    }
                    ClientLine::BareTool(call) => {
                        let id = self.next_bare;
                        self.next_bare += 1;
                        state.bare.insert(id);
                        Some((
                            client,
                            Request {
                                id: Some(id),
                                command: Command::ToolCall { call },
                            },
                        ))
                    }
                }
            }
        }
    }

    /// Sends one of the engine's lines where it goes: a reply to its client, an event to every client
    /// subscribed to its topic.
    pub fn dispatch(&mut self, outgoing: Outgoing) {
        match outgoing {
            Outgoing::Reply { client, reply } => {
                let Some(state) = self.clients.get_mut(&client) else {
                    return;
                };
                if state.pending_hello.as_ref().is_some_and(|(id, _)| *id == reply.id) {
                    let (_, topics) = state.pending_hello.take().unwrap_or_default();
                    if reply.ok {
                        state.topics = Some(topics);
                    }
                }
                let bare = state.bare.remove(&reply.id);
                let finished = state.gone && state.bare.is_empty();
                let line = if bare {
                    // The MCP wire: the tool result alone.
                    let result = if reply.ok {
                        Value::Object(reply.data)
                    } else {
                        serde_json::to_value(ToolResult::error(reply.error.unwrap_or_default())).unwrap_or(Value::Null)
                    };
                    result.to_string()
                } else {
                    serde_json::to_string(&reply).unwrap_or_default()
                };
                if state.lines.send(line).is_err() || finished {
                    self.clients.remove(&client);
                }
            }
            Outgoing::Event(event) => {
                let topic = event.topic();
                let Ok(line) = serde_json::to_string(&event) else {
                    return;
                };
                let mut gone = Vec::new();
                for (client, state) in &self.clients {
                    if state.topics.as_ref().is_some_and(|topics| topics.contains(&topic))
                        && state.lines.send(line.clone()).is_err()
                    {
                        gone.push(*client);
                    }
                }
                for client in gone {
                    self.clients.remove(&client);
                }
            }
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::Stream;
    use budd_claude::ipc::Endpoint;
    use std::fs;
    use std::io;
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use tokio::net::UnixListener;

    /// A bound socket file, removed when dropped.
    pub struct Listener {
        inner: UnixListener,
        path: PathBuf,
        owner: u32,
    }

    impl Listener {
        /// Binds the socket: the folder above it is created and made 0700, a stale socket is removed,
        /// anything else at the path is refused, and the new socket is made 0600.
        pub fn bind(endpoint: &Endpoint) -> io::Result<Listener> {
            let Endpoint::UnixSocket(path) = endpoint else {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "a named pipe is not an endpoint on this platform",
                ));
            };
            let parent = match path.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => parent,
                _ => Path::new("."),
            };
            budd_claude::paths::ensure_private_dir(parent)?;
            match fs::symlink_metadata(path) {
                Ok(existing) if existing.file_type().is_socket() => fs::remove_file(path)?,
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("{} exists and is not a socket", path.display()),
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            let inner = UnixListener::bind(path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
            let owner = fs::metadata(path)?.uid();
            Ok(Listener {
                inner,
                path: path.clone(),
                owner,
            })
        }

        /// The next connection, or `None` for one from another user.
        pub async fn accept(&mut self) -> io::Result<Option<Box<dyn Stream>>> {
            let (stream, _) = self.inner.accept().await?;
            let peer = stream.peer_cred()?.uid();
            if peer != self.owner {
                return Ok(None);
            }
            Ok(Some(Box::new(stream)))
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::Stream;
    use budd_claude::ipc::Endpoint;
    use std::io;
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

    /// A named pipe server: the next instance waiting for a client, created ahead so the name is never free.
    pub struct Listener {
        name: String,
        waiting: Option<NamedPipeServer>,
    }

    impl Listener {
        /// Creates the pipe's first instance, refusing a name another server already holds.
        pub fn bind(endpoint: &Endpoint) -> io::Result<Listener> {
            let Endpoint::NamedPipe(name) = endpoint else {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "a Unix socket is not an endpoint on this platform",
                ));
            };
            let first = Self::instance(name, true)?;
            Ok(Listener {
                name: name.clone(),
                waiting: Some(first),
            })
        }

        /// One new pipe instance under the user's security descriptor.
        fn instance(name: &str, first: bool) -> io::Result<NamedPipeServer> {
            let descriptor = sddl::Descriptor::for_current_user()?;
            let mut attributes = sddl::SecurityAttributes::for_descriptor(&descriptor);
            let mut options = ServerOptions::new();
            options.first_pipe_instance(first).reject_remote_clients(true);
            // SAFETY: the attributes point at a descriptor that lives until the end of this function,
            // after the pipe has been created, which is all CreateNamedPipeW needs of it.
            unsafe { options.create_with_security_attributes_raw(name, (&raw mut attributes).cast()) }
        }

        /// The next connection; every client is this user's by the descriptor, so never `None`.
        pub async fn accept(&mut self) -> io::Result<Option<Box<dyn Stream>>> {
            let server = match self.waiting.take() {
                Some(server) => server,
                None => Self::instance(&self.name, false)?,
            };
            server.connect().await?;
            // The next client must find an instance even while this one is served.
            self.waiting = Self::instance(&self.name, false).ok();
            Ok(Some(Box::new(server)))
        }
    }

    /// The pipe's security descriptor: the current user and SYSTEM, nobody else.
    ///
    /// This duplicates the SDDL the blocking `budd_claude::ipc::windows` builds, as that helper is not
    /// exposed; the two must stay the same, so a client of either reaches the other's pipe. It calls
    /// the Win32 functions directly rather than through `windows-sys`, which this crate does not depend
    /// on.
    mod sddl {
        use std::ffi::c_void;
        use std::io;
        use std::ptr;

        #[repr(C)]
        struct SidAndAttributes {
            sid: *mut c_void,
            attributes: u32,
        }

        /// `TOKEN_USER`: the SID of the token's user.
        #[repr(C)]
        struct TokenUser {
            user: SidAndAttributes,
        }

        /// `SECURITY_ATTRIBUTES`, as `CreateNamedPipeW` takes it.
        #[repr(C)]
        pub struct SecurityAttributes {
            length: u32,
            descriptor: *mut c_void,
            inherit: i32,
        }

        impl SecurityAttributes {
            /// Attributes that apply `descriptor`, not inherited by children.
            pub fn for_descriptor(descriptor: &Descriptor) -> SecurityAttributes {
                SecurityAttributes {
                    length: std::mem::size_of::<SecurityAttributes>() as u32,
                    descriptor: descriptor.0,
                    inherit: 0,
                }
            }
        }

        const TOKEN_QUERY: u32 = 0x0008;
        const TOKEN_USER_CLASS: i32 = 1;
        const SDDL_REVISION_1: u32 = 1;

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> *mut c_void;
            fn CloseHandle(handle: *mut c_void) -> i32;
            fn LocalFree(memory: *mut c_void) -> *mut c_void;
        }

        #[link(name = "advapi32")]
        unsafe extern "system" {
            fn OpenProcessToken(process: *mut c_void, access: u32, token: *mut *mut c_void) -> i32;
            fn GetTokenInformation(
                token: *mut c_void,
                class: i32,
                information: *mut c_void,
                length: u32,
                needed: *mut u32,
            ) -> i32;
            fn ConvertSidToStringSidW(sid: *mut c_void, text: *mut *mut u16) -> i32;
            fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text: *const u16,
                revision: u32,
                descriptor: *mut *mut c_void,
                size: *mut u32,
            ) -> i32;
        }

        /// `text` as a NUL-terminated UTF-16 string.
        fn wide(text: &str) -> Vec<u16> {
            text.encode_utf16().chain(std::iter::once(0)).collect()
        }

        /// A security descriptor from SDDL, freed when dropped.
        pub struct Descriptor(pub *mut c_void);

        impl Descriptor {
            /// A descriptor admitting the current user and SYSTEM, with full access, and nobody else.
            pub fn for_current_user() -> io::Result<Descriptor> {
                let sid = current_user_sid()?;
                let sddl = wide(&format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})"));
                let mut descriptor: *mut c_void = ptr::null_mut();
                // SAFETY: the SDDL is a valid NUL-terminated wide string; the out pointer is valid; the
                // size pointer may be null, as documented.
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
            let mut token: *mut c_void = ptr::null_mut();
            // SAFETY: the process pseudo-handle needs no closing; the out pointer is valid.
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
                return Err(io::Error::last_os_error());
            }
            struct Token(*mut c_void);
            impl Drop for Token {
                fn drop(&mut self) {
                    // SAFETY: the token came from OpenProcessToken and is closed once, here.
                    unsafe { CloseHandle(self.0) };
                }
            }
            let token = Token(token);
            let mut needed: u32 = 0;
            // SAFETY: a null buffer of length 0 asks for the size, which is written to `needed`.
            unsafe { GetTokenInformation(token.0, TOKEN_USER_CLASS, ptr::null_mut(), 0, &mut needed) };
            if needed == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
            // SAFETY: the buffer holds `needed` bytes, 8-byte aligned, as TOKEN_USER requires.
            let ok = unsafe {
                GetTokenInformation(
                    token.0,
                    TOKEN_USER_CLASS,
                    buffer.as_mut_ptr().cast(),
                    needed,
                    &mut needed,
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: the call filled the buffer with a TOKEN_USER whose Sid points inside the same buffer.
            let user = unsafe { &*(buffer.as_ptr() as *const TokenUser) };
            let mut text: *mut u16 = ptr::null_mut();
            // SAFETY: the SID pointer is the one the token gave; the out pointer is valid.
            if unsafe { ConvertSidToStringSidW(user.user.sid, &mut text) } == 0 || text.is_null() {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use budd_proto::{Event, Reply, UnreadEvent};
    use serde_json::json;

    fn connected(clients: &mut Clients, id: u64) -> mpsc::UnboundedReceiver<String> {
        let (lines, rx) = mpsc::unbounded_channel();
        clients.inbound(Inbound::Connected {
            client: ClientId(id),
            lines,
        });
        rx
    }

    #[test]
    fn hello_subscribes_and_events_go_to_subscribers_only() {
        let mut clients = Clients::new();
        let mut ui = connected(&mut clients, 1);
        let mut watcher = connected(&mut clients, 2);
        let mut quiet = connected(&mut clients, 3);
        assert_eq!(clients.len(), 3);
        let hello = ClientLine::parse(r#"{"id":1,"cmd":"hello","client":"ui","proto":1,"subscribe":["crew"]}"#)
            .expect("parses");
        let request = clients.inbound(Inbound::Line {
            client: ClientId(1),
            line: hello,
        });
        assert!(matches!(request, Some((ClientId(1), Request { id: Some(1), .. }))));
        let all = ClientLine::parse(r#"{"id":7,"cmd":"hello","client":"watch","proto":1}"#).expect("parses");
        clients.inbound(Inbound::Line {
            client: ClientId(2),
            line: all,
        });
        // Not subscribed until the hello is answered well.
        clients.dispatch(Outgoing::Event(Event::Unread(UnreadEvent::default())));
        assert!(ui.try_recv().is_err());
        clients.dispatch(Outgoing::Reply {
            client: ClientId(1),
            reply: Reply::done(1),
        });
        clients.dispatch(Outgoing::Reply {
            client: ClientId(2),
            reply: Reply::done(7),
        });
        assert_eq!(ui.try_recv().expect("the reply"), r#"{"id":1,"ok":true}"#);
        watcher.try_recv().expect("the reply");
        clients.dispatch(Outgoing::Event(Event::Unread(UnreadEvent {
            count: 1,
            newest: None,
            arrived_at: None,
        })));
        clients.dispatch(Outgoing::Event(Event::Crew(Default::default())));
        assert_eq!(ui.try_recv().expect("crew"), r#"{"ev":"crew","members":[]}"#);
        assert!(ui.try_recv().is_err(), "unread was not subscribed");
        assert_eq!(watcher.try_recv().expect("unread"), r#"{"ev":"unread","count":1}"#);
        assert_eq!(watcher.try_recv().expect("crew"), r#"{"ev":"crew","members":[]}"#);
        assert!(quiet.try_recv().is_err(), "never said hello");
        // A refused hello leaves the client unsubscribed.
        let late = ClientLine::parse(r#"{"id":9,"cmd":"hello","client":"old","proto":9}"#).expect("parses");
        clients.inbound(Inbound::Line {
            client: ClientId(3),
            line: late,
        });
        clients.dispatch(Outgoing::Reply {
            client: ClientId(3),
            reply: Reply::error(9, "no"),
        });
        quiet.try_recv().expect("the refusal");
        clients.dispatch(Outgoing::Event(Event::Crew(Default::default())));
        assert!(quiet.try_recv().is_err());
        // A client that went is forgotten.
        drop(quiet);
        clients.inbound(Inbound::Disconnected { client: ClientId(3) });
        assert_eq!(clients.len(), 2);
        drop(ui);
        clients.dispatch(Outgoing::Event(Event::Crew(Default::default())));
        assert_eq!(clients.len(), 1, "a closed channel drops the client");
    }

    #[test]
    fn a_bare_tool_call_is_answered_with_a_bare_tool_result() {
        let mut clients = Clients::new();
        let mut mcp = connected(&mut clients, 4);
        let bare = ClientLine::parse(r#"{"tool":"say","arguments":{"text":"hi"},"callers":[1]}"#).expect("parses");
        let (client, request) = clients
            .inbound(Inbound::Line {
                client: ClientId(4),
                line: bare,
            })
            .expect("a request");
        assert_eq!(client, ClientId(4));
        let id = request.id.expect("an id was lent");
        assert!(id >= BARE_IDS);
        assert!(matches!(request.command, Command::ToolCall { call } if call.tool == "say"));
        clients.dispatch(Outgoing::Reply {
            client,
            reply: Reply::ok(id, &ToolResult::ok("Said.")),
        });
        let line: Value = serde_json::from_str(&mcp.try_recv().expect("the result")).expect("json");
        assert_eq!(line, json!({"text": "Said.", "isError": false}));
        // An engine error on a bare call still reads as a tool result.
        let bare = ClientLine::parse(r#"{"tool":"say","arguments":{"text":"hi"}}"#).expect("parses");
        let (client, request) = clients
            .inbound(Inbound::Line {
                client: ClientId(4),
                line: bare,
            })
            .expect("a request");
        clients.dispatch(Outgoing::Reply {
            client,
            reply: Reply::error(request.id.unwrap_or(0), "broken"),
        });
        let line: Value = serde_json::from_str(&mcp.try_recv().expect("the result")).expect("json");
        assert_eq!(line, json!({"text": "broken", "isError": true}));
        assert!(!clients.is_empty());
        // A one-shot caller that closed before its answer still gets it, then goes.
        let bare = ClientLine::parse(r#"{"tool":"status","arguments":{"line":"x"}}"#).expect("parses");
        let (client, request) = clients
            .inbound(Inbound::Line {
                client: ClientId(4),
                line: bare,
            })
            .expect("a request");
        clients.inbound(Inbound::Disconnected { client });
        assert_eq!(clients.len(), 1, "kept for its answer");
        clients.dispatch(Outgoing::Reply {
            client,
            reply: Reply::ok(request.id.unwrap_or(0), &ToolResult::ok("Shown.")),
        });
        assert!(mcp.try_recv().expect("the result").contains("Shown."));
        assert!(clients.is_empty());
    }

    #[cfg(unix)]
    mod on_unix {
        use super::*;
        use std::os::unix::fs::PermissionsExt;
        use tokio::io::AsyncReadExt;
        use tokio::net::UnixStream;

        #[tokio::test]
        async fn connections_are_served_as_lines_and_the_socket_is_private() {
            let temp = tempfile::tempdir().expect("tempdir");
            let path = temp.path().join("run").join("budd.sock");
            let endpoint = Endpoint::UnixSocket(path.clone());
            let mut server = Server::bind(&endpoint).expect("binds");
            let mode = std::fs::metadata(&path).expect("socket").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            let dir_mode = std::fs::metadata(path.parent().expect("parent"))
                .expect("dir")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700);
            let (to_engine, mut inbound) = mpsc::unbounded_channel();
            let accepting = tokio::spawn(async move {
                let stream = server.accept().await.expect("accepts");
                serve_connection(stream, ClientId(1), to_engine).await;
                server
            });
            let mut client = UnixStream::connect(&path).await.expect("connects");
            client
                .write_all(b"{\"id\":1,\"cmd\":\"unread.read\"}\nnot json\n")
                .await
                .expect("writes");
            let connected = inbound.recv().await.expect("connected");
            let Inbound::Connected { lines, .. } = connected else {
                panic!("{connected:?}");
            };
            let line = inbound.recv().await.expect("a line");
            assert!(matches!(
                line,
                Inbound::Line {
                    client: ClientId(1),
                    line: ClientLine::Request(Request {
                        id: Some(1),
                        command: Command::UnreadRead
                    })
                }
            ));
            lines.send(r#"{"id":1,"ok":true}"#.to_owned()).expect("sends");
            drop(lines);
            client.shutdown().await.expect("closes writing");
            let mut answer = String::new();
            client.read_to_string(&mut answer).await.expect("reads");
            // The refusal of the bad line and the reply, in the order they were made.
            assert!(answer.contains("not JSON"), "{answer}");
            assert!(answer.ends_with("{\"id\":1,\"ok\":true}\n"), "{answer}");
            assert!(matches!(inbound.recv().await, Some(Inbound::Disconnected { .. })));
            let server = accepting.await.expect("the task");
            drop(server);
            assert!(!path.exists(), "the socket goes with the server");
            // A plain file at the path is refused.
            std::fs::write(&path, "x").expect("writes");
            let refused = Server::bind(&endpoint).err().expect("refused");
            assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
        }
    }
}
