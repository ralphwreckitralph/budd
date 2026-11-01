//! The client's end of the endpoint: connecting, and `budd watch`, which prints every line the
//! daemon sends.

use crate::server::Stream;
use budd_claude::ipc::Endpoint;
use budd_proto::{Command, PROTO, Request, Topic};
use std::io::{self, Write};
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
}
