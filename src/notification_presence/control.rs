//! One-shot presence requests with an overall deadline, including connect and
//! write. The usual blocking control client deliberately supports long calls
//! and subscriptions; its Windows pipe reads have no timeout. Presence must
//! recover independently without leaving blocked worker threads behind.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::runtime::{Builder, Runtime};

use crate::control::ControlRequest;

const MAX_FRAME: usize = 16 * 1024;

pub(super) struct Sender {
    socket: PathBuf,
    deadline: Duration,
    // Created lazily on the existing presence worker, then reused for every
    // attempt. All I/O is async; no blocking pool or per-retry thread exists.
    runtime: Option<Runtime>,
}

impl Sender {
    pub(super) fn new(socket: PathBuf, deadline: Duration) -> Self {
        Self {
            socket,
            deadline,
            runtime: None,
        }
    }

    pub(super) fn send(&mut self, request: &ControlRequest) -> Result<()> {
        if self.runtime.is_none() {
            self.runtime = Some(
                Builder::new_current_thread()
                    .enable_io()
                    .enable_time()
                    .build()?,
            );
        }
        self.runtime.as_ref().unwrap().block_on(request_with(
            connect(&self.socket),
            request,
            self.deadline,
        ))
    }
}

async fn request_with<S>(
    connecting: impl Future<Output = Result<S>>,
    request: &ControlRequest,
    deadline: Duration,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // Dropping the future also drops its socket/overlapped named-pipe handle.
    // An accepted request whose reply is lost remains uncertain to Delivery,
    // which clears it before activating a different account or chat.
    tokio::time::timeout(deadline, async {
        let mut stream = connecting.await?;
        let mut frame = serde_json::to_vec(request)?;
        frame.push(b'\n');
        if frame.len() > MAX_FRAME {
            bail!("presence request is too large");
        }
        stream.write_all(&frame).await?;
        stream.flush().await?;
        let mut reader = BufReader::new(stream.take((MAX_FRAME + 1) as u64));
        let mut response = Vec::new();
        reader.read_until(b'\n', &mut response).await?;
        if response.len() > MAX_FRAME {
            bail!("presence response is too large");
        }
        if response.last() != Some(&b'\n') {
            bail!("the daemon closed the presence connection");
        }
        let response: serde_json::Value = serde_json::from_slice(&response)
            .context("invalid presence response from the daemon")?;
        if response["ok"] != true {
            bail!("the daemon did not accept presence");
        }
        Ok(())
    })
    .await
    .context("presence control request timed out")?
}

#[cfg(unix)]
async fn connect(path: &Path) -> Result<tokio::net::UnixStream> {
    Ok(tokio::net::UnixStream::connect(path).await?)
}

#[cfg(windows)]
async fn connect(path: &Path) -> Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use tokio::net::windows::named_pipe::ClientOptions;
    use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;
    use windows_sys::Win32::Storage::FileSystem::SECURITY_IDENTIFICATION;

    let name = path.to_str().context("pipe name is not UTF-8")?;
    if !name.starts_with(r"\\.\pipe\") {
        bail!("presence requires a local named pipe");
    }
    let own = crate::win::current_user_sid()?;
    loop {
        // Unlike a synchronous pipe read, this opens an OVERLAPPED handle.
        // CreateFile returns PIPE_BUSY immediately; waiting is cancellable.
        match ClientOptions::new()
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(name)
        {
            Ok(pipe) => {
                if crate::win::object_owner_sid(&pipe)? != own {
                    bail!("the control pipe belongs to another user");
                }
                return Ok(pipe);
            }
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notification_presence::{Presence, View};
    use std::sync::mpsc;

    const TEST_DEADLINE: Duration = Duration::from_millis(100);

    fn request() -> ControlRequest {
        ControlRequest::NotificationView {
            session: "test".into(),
            account: "123456789".into(),
            chat: 42,
            visible: true,
        }
    }

    #[tokio::test]
    async fn deadline_cancels_connect_write_and_partial_response() {
        let error = request_with(
            std::future::pending::<Result<tokio::io::DuplexStream>>(),
            &request(),
            TEST_DEADLINE,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));

        // A peer that never reads prevents the request from fitting in its
        // buffer. Timing out closes the stream rather than retaining a writer.
        let (client, mut peer) = tokio::io::duplex(1);
        assert!(
            request_with(async { Ok(client) }, &request(), TEST_DEADLINE)
                .await
                .is_err()
        );
        let mut remaining = Vec::new();
        tokio::time::timeout(TEST_DEADLINE, peer.read_to_end(&mut remaining))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(remaining.len(), 1);

        let (client, peer) = tokio::io::duplex(MAX_FRAME);
        let server = tokio::spawn(async move {
            let mut peer = BufReader::new(peer);
            peer.read_until(b'\n', &mut Vec::new()).await.unwrap();
            peer.get_mut().write_all(b"{\"ok\":").await.unwrap();
            // The client must hang up even though the server sent part of a line.
            assert_eq!(peer.read(&mut [0]).await.unwrap(), 0);
        });
        assert!(
            request_with(async { Ok(client) }, &request(), TEST_DEADLINE)
                .await
                .is_err()
        );
        tokio::time::timeout(TEST_DEADLINE, server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn oversized_and_negative_responses_are_not_accepted() {
        for response in [
            vec![b' '; MAX_FRAME + 1],
            b"{\"ok\":false}\n".to_vec(),
            b"{\"ok\":true}".to_vec(),
        ] {
            let (client, peer) = tokio::io::duplex(MAX_FRAME * 2);
            let server = tokio::spawn(async move {
                let mut peer = BufReader::new(peer);
                peer.read_until(b'\n', &mut Vec::new()).await.unwrap();
                peer.get_mut().write_all(&response).await.unwrap();
            });
            assert!(
                request_with(async { Ok(client) }, &request(), TEST_DEADLINE)
                    .await
                    .is_err()
            );
            server.await.unwrap();
        }
    }

    #[test]
    fn stalled_accepted_request_recovers_on_the_same_presence_worker() {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let path = dir.path().join("presence.sock");
        #[cfg(windows)]
        let path = PathBuf::from(format!(
            r"\\.\pipe\cww-presence-{}",
            super::super::session()
        ));
        let server_path = path.clone();
        let (ready, wait_ready) = mpsc::channel();
        let (events, received) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let runtime = Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
                .unwrap();
            runtime.block_on(async {
                let mut listener = crate::control::bind(&server_path).unwrap();
                ready.send(()).unwrap();
                // First request is accepted and read, but its reply stalls.
                let mut first = BufReader::new(listener.accept().await.unwrap());
                let mut line = String::new();
                first.read_line(&mut line).await.unwrap();
                let old: serde_json::Value = serde_json::from_str(&line).unwrap();
                events.send(old).unwrap();
                assert_eq!(
                    tokio::time::timeout(Duration::from_secs(5), first.read(&mut [0]))
                        .await
                        .unwrap()
                        .unwrap(),
                    0
                );
                // The same worker reconnects: uncertain old view cleared,
                // latest view set, then Drop clears that latest view.
                for _ in 0..3 {
                    let next = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                    let mut next = BufReader::new(next);
                    let mut line = String::new();
                    next.read_line(&mut line).await.unwrap();
                    let value: serde_json::Value = serde_json::from_str(&line).unwrap();
                    next.get_mut().write_all(b"{\"ok\":true}\n").await.unwrap();
                    events.send(value).unwrap();
                }
            });
        });
        wait_ready.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut sender = Sender::new(path, TEST_DEADLINE);
        let presence =
            Presence::with_sender(move |request| sender.send(&request).map_err(|e| e.to_string()));
        presence.set(Some(View {
            account: "123456789".into(),
            chat: 42,
        }));
        let first = received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(first["visible"], true);
        presence.set(Some(View {
            account: "987654321".into(),
            chat: 99,
        }));
        for (account, chat, visible) in [("123456789", 42, false), ("987654321", 99, true)] {
            let next = received.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(next["session"], first["session"]);
            assert_eq!(next["account"], account);
            assert_eq!(next["chat"], chat);
            assert_eq!(next["visible"], visible);
        }
        drop(presence);
        let last = received.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(last["account"], "987654321");
        assert_eq!(last["chat"], 99);
        assert_eq!(last["visible"], false);
        server.join().unwrap();
        drop(dir);
    }
}
