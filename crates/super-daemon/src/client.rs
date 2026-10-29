//! A client of the daemon socket: connect, say hello, call commands. `super
//! daemon status|stop` use it; the terminal client of slice 3 grows from it.

use anyhow::{Context as _, bail};
use std::path::Path;
use super_sdk::daemon::{ClientKind, DaemonCommand, DaemonRequest, HelloReply, PROTOCOL_VERSIONS};
use super_sdk::protocol::{Incoming, Response, decode_line};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

pub struct DaemonClient {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    next_id: u64,
}

impl DaemonClient {
    pub async fn connect(socket: &Path) -> std::io::Result<Self> {
        let stream = UnixStream::connect(socket).await?;
        let (read, write) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(read),
            writer: write,
            next_id: 0,
        })
    }

    /// The handshake. Fails with the daemon's own words on a protocol
    /// mismatch.
    pub async fn hello(
        &mut self,
        client: ClientKind,
        name: &str,
        version: &str,
    ) -> anyhow::Result<HelloReply> {
        let response = self
            .call(DaemonCommand::Hello {
                client,
                name: Some(name.to_string()),
                version: version.to_string(),
                protocol_versions: PROTOCOL_VERSIONS,
            })
            .await?;
        if !response.success {
            bail!(
                "{}",
                response.error.unwrap_or_else(|| "hello refused".into())
            );
        }
        let data = response.data.context("hello without data")?;
        Ok(serde_json::from_value(data)?)
    }

    /// Send one daemon-level command and wait for its response, discarding
    /// events on the way.
    pub async fn call(&mut self, command: DaemonCommand) -> anyhow::Result<Response> {
        self.next_id += 1;
        let id = format!("c{}", self.next_id);
        let request = DaemonRequest {
            id: Some(id.clone()),
            command,
        };
        self.send_line(&serde_json::to_string(&request)?).await?;
        loop {
            let Some(line) = self.next_line().await? else {
                bail!("the daemon closed the connection");
            };
            if let Ok(Incoming::Response(response)) = decode_line(&line)
                && response.id.as_deref() == Some(&id)
            {
                return Ok(response);
            }
        }
    }

    pub async fn send_line(&mut self, line: &str) -> std::io::Result<()> {
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await
    }

    /// The next line without its terminator, or `None` when the daemon
    /// closed the connection.
    pub async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        let mut line = String::new();
        loop {
            line.clear();
            if self.reader.read_line(&mut line).await? == 0 {
                return Ok(None);
            }
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            if !trimmed.trim().is_empty() {
                return Ok(Some(trimmed.to_string()));
            }
        }
    }
}
