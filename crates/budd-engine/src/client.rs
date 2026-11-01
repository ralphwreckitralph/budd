//! The client's end of the endpoint: connecting, one request and its reply, and `budd watch`, which
//! prints every line the daemon sends.

use crate::server::Stream;
use budd_claude::ipc::Endpoint;
use budd_proto::{Command, PROTO, Reply, Request, Topic};
use std::io::{self, Write};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Connects to the daemon at `endpoint`. Nobody listening is `NotFound` or `ConnectionRefused`.
pub async fn connect(endpoint: &Endpoint) -> io::Result<Box<dyn Stream>> {
    match endpoint {
        Endpoint::UnixSocket(path) => {
            #[cfg(unix)]
            {
                let stream = tokio::net::UnixStream::connect(path).await?;
                Ok(Box::new(stream))
            }
            #[cfg(not(unix))]
            {
                let _ = path;
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "a Unix socket is not an endpoint on this platform",
                ))
            }
        }
        Endpoint::NamedPipe(name) => {
            #[cfg(windows)]
            {
                connect_pipe(name).await
            }
            #[cfg(not(windows))]
            {
                let _ = name;
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "a named pipe is not an endpoint on this platform",
                ))
            }
        }
    }
}

/// Opens the pipe, waiting out a busy moment between two instances for up to a few seconds.
#[cfg(windows)]
async fn connect_pipe(name: &str) -> io::Result<Box<dyn Stream>> {
    use tokio::net::windows::named_pipe::ClientOptions;
    /// `ERROR_PIPE_BUSY`: every instance is taken for the moment.
    const PIPE_BUSY: i32 = 231;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match ClientOptions::new().open(name) {
            Ok(client) => return Ok(Box::new(client)),
            Err(error) if error.raw_os_error() == Some(PIPE_BUSY) && tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Connects, sends `request` and waits for its reply, for up to `timeout`. The connection stays open
/// until the reply comes, as a command like `reset` or `doctor.run` answers only once its work is
/// done; no `hello` goes first, so no event is in the way. A request without an id gets id 1.
pub async fn request(endpoint: &Endpoint, request: &Request, timeout: Duration) -> io::Result<Reply> {
    let id = request.id.unwrap_or(1);
    let request = Request {
        id: Some(id),
        command: request.command.clone(),
    };
    let exchange = async {
        let stream = connect(endpoint).await?;
        let (reader, mut writer) = tokio::io::split(stream);
        let mut line = serde_json::to_string(&request).map_err(io::Error::other)?;
        line.push('\n');
        writer.write_all(line.as_bytes()).await?;
        writer.flush().await?;
        let mut lines = BufReader::new(reader).lines();
        while let Some(line) = lines.next_line().await? {
            match serde_json::from_str::<Reply>(&line) {
                Ok(reply) if reply.id == id => return Ok(reply),
                Ok(_) => {}
                Err(_) => {
                    // A line the daemon refused before it reached the engine has no id.
                    let refused: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                    if refused.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
                        let error = refused
                            .get("error")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("refused");
                        return Err(io::Error::new(io::ErrorKind::InvalidData, error.to_owned()));
                    }
                }
            }
        }
        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the daemon closed the connection before it answered",
        ))
    };
    tokio::time::timeout(timeout, exchange).await.unwrap_or_else(|_| {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "the daemon did not answer in time",
        ))
    })
}

/// Connects, says `hello` as the client `name` subscribed to every topic, and writes every line the
/// daemon sends to `out`, the reply to `hello` first, until the daemon closes the connection.
pub async fn watch(endpoint: &Endpoint, name: &str, out: &mut (dyn Write + Send)) -> io::Result<()> {
    let stream = connect(endpoint).await?;
    let (reader, mut writer) = tokio::io::split(stream);
    let hello = Request {
        id: Some(1),
        command: Command::Hello {
            client: name.to_owned(),
            proto: PROTO,
            subscribe: Some(Topic::ALL.to_vec()),
        },
    };
    let mut line = serde_json::to_string(&hello).map_err(io::Error::other)?;
    line.push('\n');
    writer.write_all(line.as_bytes()).await?;
    writer.flush().await?;
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        writeln!(out, "{line}")?;
        out.flush()?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tokio::net::UnixListener;

    #[tokio::test]
    async fn watch_says_hello_and_prints_what_comes_back() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("budd.sock");
        let listener = UnixListener::bind(&path).expect("binds");
        let endpoint = Endpoint::UnixSocket(path.clone());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accepts");
            let (reader, mut writer) = tokio::io::split(stream);
            let mut lines = BufReader::new(reader).lines();
            let hello = lines.next_line().await.expect("reads").expect("a line");
            writer
                .write_all(b"{\"id\":1,\"ok\":true,\"proto\":1}\n{\"ev\":\"unread\",\"count\":0}\n")
                .await
                .expect("writes");
            writer.shutdown().await.expect("closes");
            hello
        });
        let mut out: Vec<u8> = Vec::new();
        watch(&endpoint, "watch", &mut out).await.expect("watches");
        let hello = server.await.expect("the task");
        let sent: Request = serde_json::from_str(&hello).expect("a request");
        assert!(matches!(
            sent.command,
            Command::Hello { proto: PROTO, subscribe: Some(ref topics), .. } if topics.len() == Topic::ALL.len()
        ));
        assert_eq!(
            String::from_utf8(out).expect("utf8"),
            "{\"id\":1,\"ok\":true,\"proto\":1}\n{\"ev\":\"unread\",\"count\":0}\n"
        );
        let missing = Endpoint::UnixSocket(temp.path().join("none.sock"));
        let error = connect(&missing).await.err().expect("nobody listening");
        assert!(budd_claude::ipc::is_not_running(&error), "{error}");
        let pipe = Endpoint::NamedPipe(r"\\.\pipe\budd".to_owned());
        let wrong = connect(&pipe).await.err().expect("wrong platform");
        assert_eq!(wrong.kind(), io::ErrorKind::Unsupported);
    }

    #[tokio::test]
    async fn request_waits_for_the_reply_with_its_id() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("budd.sock");
        let listener = UnixListener::bind(&path).expect("binds");
        let endpoint = Endpoint::UnixSocket(path.clone());
        let server = tokio::spawn(async move {
            // The first caller gets an event-like line, a stranger's reply, then its own.
            let (stream, _) = listener.accept().await.expect("accepts");
            let (reader, mut writer) = tokio::io::split(stream);
            let mut lines = BufReader::new(reader).lines();
            let first = lines.next_line().await.expect("reads").expect("a line");
            writer
                .write_all(b"{\"ev\":\"log\",\"level\":\"info\",\"message\":\"hi\"}\n{\"id\":9,\"ok\":true}\n{\"id\":1,\"ok\":true,\"checks\":[]}\n")
                .await
                .expect("writes");
            // The second caller is refused before the engine sees it.
            let (stream, _) = listener.accept().await.expect("accepts");
            let (_, mut writer) = tokio::io::split(stream);
            writer
                .write_all(b"{\"ok\":false,\"error\":\"the line is too long\"}\n")
                .await
                .expect("writes");
            writer.shutdown().await.expect("closes");
            // The third caller gets nothing.
            let (stream, _) = listener.accept().await.expect("accepts");
            tokio::time::sleep(Duration::from_secs(5)).await;
            drop(stream);
            first
        });
        let sent = Request {
            id: None,
            command: Command::DoctorRun,
        };
        let reply = request(&endpoint, &sent, Duration::from_secs(5))
            .await
            .expect("a reply");
        assert!(reply.ok && reply.id == 1 && reply.data.contains_key("checks"));
        let refused = request(&endpoint, &sent, Duration::from_secs(5))
            .await
            .expect_err("refused");
        assert_eq!(refused.kind(), io::ErrorKind::InvalidData);
        assert_eq!(refused.to_string(), "the line is too long");
        let late = request(&endpoint, &sent, Duration::from_millis(100))
            .await
            .expect_err("timed out");
        assert_eq!(late.kind(), io::ErrorKind::TimedOut);
        server.abort();
    }
}
