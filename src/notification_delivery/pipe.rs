//! Inherited pipes between the confined worker and its unsandboxed parent.
//! Frames and channels are bounded; async Unix pipe handles avoid stdin
//! threads which could otherwise keep the runtime alive after cancellation.

use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
#[cfg(any(unix, test))]
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[cfg(any(unix, test))]
use super::CAPACITY;
use super::{Endpoint, Input, Output};

const MAX_FRAME: usize = crate::notifications::protocol::MAX_FRAME_BYTES;
const IO_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) fn fits_frame(value: &impl Serialize) -> bool {
    serde_json::to_vec(value).is_ok_and(|frame| frame.len() < MAX_FRAME)
}

#[cfg(unix)]
pub struct Inherited {
    input: std::os::fd::OwnedFd,
    output: std::os::fd::OwnedFd,
}

#[cfg(unix)]
impl Inherited {
    /// Called before confinement, without starting a thread or event loop.
    pub fn take() -> Result<Self> {
        Ok(Self {
            input: rustix::io::dup(std::io::stdin())?,
            output: rustix::io::dup(std::io::stdout())?,
        })
    }

    pub fn connect(self, shutdown: CancellationToken) -> Result<Endpoint> {
        // These constructors check FIFO type/access and set nonblocking mode.
        let input = tokio::net::unix::pipe::Receiver::from_owned_fd(self.input)?;
        let output = tokio::net::unix::pipe::Sender::from_owned_fd(self.output)?;
        Ok(connect(input, output, shutdown))
    }
}

#[cfg(any(unix, test))]
pub(super) fn connect<R, W>(input: R, output: W, shutdown: CancellationToken) -> Endpoint
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let stop = shutdown.child_token();
    let shutdown = stop.clone();
    let (commands, mut receiver) = mpsc::channel(CAPACITY);
    let (sender, events) = mpsc::channel(CAPACITY);
    let task = tokio::spawn(async move {
        let receive = async {
            let mut input = BufReader::new(input);
            loop {
                let Some(event) = read::<Output, _>(&mut input).await? else {
                    return Ok::<_, anyhow::Error>(());
                };
                tokio::time::timeout(IO_TIMEOUT, sender.send(event)).await??;
            }
        };
        let send = async {
            let mut output = output;
            while let Some(command) = receiver.recv().await {
                write(&mut output, &command).await?;
            }
            Ok::<_, anyhow::Error>(())
        };
        tokio::select! {
            () = shutdown.cancelled() => {},
            _ = receive => {},
            _ = send => {},
        }
    });
    Endpoint {
        commands,
        events,
        task,
        stop,
    }
}

/// One worker's private pipe lifetime. The native helper stays owned by the
/// parent between workers. The reset barrier drops every old routing record
/// and drains old outputs before another worker can reuse local generations.
pub async fn serve<R, W>(
    input: R,
    output: W,
    endpoint: &mut Endpoint,
    shutdown: CancellationToken,
) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    reset(endpoint).await?;
    let receive = async {
        let mut input = BufReader::new(input);
        loop {
            let Some(command) = read::<Input, _>(&mut input).await? else {
                return Ok::<_, anyhow::Error>(());
            };
            // Reset is parent-only, never a worker command.
            if matches!(command, Input::Reset) {
                bail!("Invalid notification pipe command");
            }
            tokio::time::timeout(IO_TIMEOUT, endpoint.commands.send(command)).await??;
        }
    };
    let send = async {
        let mut output = output;
        while let Some(event) = endpoint.events.recv().await {
            write(&mut output, &event).await?;
        }
        Ok::<_, anyhow::Error>(())
    };
    let result = tokio::select! {
        () = shutdown.cancelled() => Ok(()),
        result = receive => result,
        result = send => result,
    };
    reset(endpoint).await?;
    result
}

async fn reset(endpoint: &mut Endpoint) -> Result<()> {
    tokio::time::timeout(IO_TIMEOUT, async {
        endpoint.commands.send(Input::Reset).await?;
        while let Some(event) = endpoint.events.recv().await {
            if matches!(event, Output::Reset) {
                return Ok(());
            }
        }
        Err(anyhow!("Notification parent stopped"))
    })
    .await?
}

async fn read<T: DeserializeOwned, R: AsyncBufRead + Unpin>(input: &mut R) -> Result<Option<T>> {
    let mut frame = Vec::new();
    let mut deadline = None;
    loop {
        let bytes = match deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, input.fill_buf()).await??,
            None => input.fill_buf().await?,
        };
        if bytes.is_empty() {
            if frame.is_empty() {
                return Ok(None);
            }
            bail!("Incomplete notification pipe frame");
        }
        let newline = bytes.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(bytes.len(), |i| i + 1);
        if frame.len() + count > MAX_FRAME {
            bail!("Notification pipe frame too large");
        }
        frame.extend_from_slice(&bytes[..count]);
        input.consume(count);
        deadline.get_or_insert_with(|| tokio::time::Instant::now() + IO_TIMEOUT);
        if newline.is_some() {
            return serde_json::from_slice(&frame)
                .map(Some)
                .map_err(|_| anyhow!("Invalid notification pipe frame"));
        }
    }
}

async fn write<T: Serialize, W: AsyncWrite + Unpin>(output: &mut W, value: &T) -> Result<()> {
    let mut frame = serde_json::to_vec(value)?;
    frame.push(b'\n');
    if frame.len() > MAX_FRAME {
        bail!("Notification pipe frame too large");
    }
    tokio::time::timeout(IO_TIMEOUT, output.write_all(&frame)).await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notification_delivery::Status;

    #[tokio::test]
    async fn frames_are_bounded_and_eof_must_end_at_a_boundary() {
        let mut frame = BufReader::new(&b"{\"event\":\"reset\"}\n"[..]);
        assert!(matches!(
            read::<Output, _>(&mut frame).await.unwrap(),
            Some(Output::Reset)
        ));
        assert!(read::<Output, _>(&mut frame).await.unwrap().is_none());
        let bad = [
            vec![b'x'; MAX_FRAME + 1],
            b"{\"event\":\"reset\"}".to_vec(),
            b"{}\n".to_vec(),
        ];
        for frame in bad {
            assert!(
                read::<Output, _>(&mut BufReader::new(&frame[..]))
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn cancellation_does_not_wait_for_an_inherited_stdin_thread() {
        let (local, remote) = tokio::io::duplex(1024);
        let (input, output) = tokio::io::split(local);
        let shutdown = CancellationToken::new();
        let endpoint = connect(input, output, shutdown.clone());
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(1), endpoint.task)
            .await
            .unwrap()
            .unwrap();
        drop(remote);
    }

    #[tokio::test]
    async fn private_pipe_relays_commands_and_status_then_stops_on_eof() {
        let (local, remote) = tokio::io::duplex(8192);
        let (input, output) = tokio::io::split(local);
        let (mut remote_input, mut remote_output) = tokio::io::split(remote);
        let mut endpoint = connect(input, output, CancellationToken::new());
        endpoint
            .commands
            .send(Input::Permission { request: false })
            .await
            .unwrap();
        assert!(matches!(
            read::<Input, _>(&mut BufReader::new(&mut remote_input))
                .await
                .unwrap(),
            Some(Input::Permission { request: false })
        ));
        write(
            &mut remote_output,
            &Output::Status {
                status: Status::default(),
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            endpoint.events.recv().await.unwrap(),
            Output::Status { .. }
        ));
        drop(remote_output);
        drop(remote_input);
        tokio::time::timeout(Duration::from_secs(1), endpoint.task)
            .await
            .unwrap()
            .unwrap();
    }
}
