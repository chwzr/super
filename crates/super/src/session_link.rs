//! What the attached terminal talks to: a session in this device's daemon,
//! over its socket, or a session on another device, through the platform.
//!
//! Both speak the same RPC: session commands go out and come back as
//! responses, session events arrive as `--mode rpc` emits them. The attached
//! front end ([`crate::attach`]) renders them the same way whichever side
//! they come from.

use serde_json::Value;
use std::sync::Arc;
use super_daemon::client::{ClientError, ClientEvent, DaemonClient};
use super_remote::controller::{HostCommands, Sender, SessionController};
use super_remote::s2::S2Client;
use super_sdk::daemon::DaemonCommand;
use super_sdk::protocol::{Command, Event, Response};
use tokio::sync::mpsc::UnboundedReceiver;

/// The platform, as this terminal reaches it.
pub(crate) struct Platform {
    pub client: Arc<S2Client>,
    /// The headers this terminal's commands carry.
    pub sender: Sender,
    /// This device's host id.
    pub host_id: String,
}

impl Platform {
    /// One `GET /api/remote/config` with the stored sign-in.
    pub async fn connect() -> Result<Arc<Self>, String> {
        let cwd = std::env::current_dir().unwrap_or_default();
        let settings = super_coding::Settings::load(&cwd, false);
        let host = match super_remote::HostIdentity::default_path() {
            Some(path) => super_remote::HostIdentity::load_or_create(&path)
                .map_err(|error| format!("host identity: {error:#}"))?,
            None => return Err("no home directory".into()),
        };
        let credentials = super_remote::PlatformCredentials::new(
            crate::remote::platform_url(&settings),
            host.name.clone(),
        );
        let remote = super_remote::Remote::start(Arc::new(credentials), host)
            .await
            .map_err(|error| format!("{error:#}"))?;
        Ok(Arc::new(Self {
            client: remote.client,
            sender: Sender {
                device_id: remote.config.device_id,
                name: format!("Terminal on {}", remote.host.name),
            },
            host_id: remote.host.host_id,
        }))
    }

    pub fn host_commands(&self, host_id: &str) -> HostCommands {
        HostCommands::new(self.client.clone(), host_id, self.sender.clone())
    }
}

/// A session on another device, followed through its streams.
pub(crate) struct RemoteSession {
    pub host_id: String,
    pub host_name: String,
    controller: SessionController,
    events: UnboundedReceiver<Value>,
    platform: Arc<Platform>,
}

impl RemoteSession {
    pub async fn open(
        platform: Arc<Platform>,
        host_id: &str,
        host_name: &str,
        session_id: &str,
    ) -> Result<Self, String> {
        let (controller, events) = SessionController::open(
            platform.client.clone(),
            host_id,
            session_id,
            platform.sender.clone(),
        )
        .await
        .map_err(|error| format!("{error:#}"))?;
        Ok(Self {
            host_id: host_id.to_string(),
            host_name: host_name.to_string(),
            controller,
            events,
            platform,
        })
    }
}

/// The attached terminal's connection: the local daemon (attached to a
/// session on this device, or idle while a remote session is shown), a
/// remote session, and the platform once it was needed.
#[derive(Default)]
pub(crate) struct Link {
    daemon: Option<(DaemonClient, UnboundedReceiver<ClientEvent>)>,
    remote: Option<RemoteSession>,
    platform: Option<Arc<Platform>>,
}

enum Arrival {
    Remote(Option<Value>),
    Daemon(Option<ClientEvent>),
}

impl Link {
    pub fn new(
        daemon: Option<(DaemonClient, UnboundedReceiver<ClientEvent>)>,
        platform: Option<Arc<Platform>>,
    ) -> Self {
        Self {
            daemon,
            remote: None,
            platform,
        }
    }

    /// A connection to this device's daemon, its events taken.
    pub fn local(client: DaemonClient, events: UnboundedReceiver<ClientEvent>) -> Self {
        Self::new(Some((client, events)), None)
    }

    pub fn daemon(&self) -> Option<&DaemonClient> {
        self.daemon.as_ref().map(|(client, _)| client)
    }

    pub fn remote(&self) -> Option<&RemoteSession> {
        self.remote.as_ref()
    }

    pub fn platform(&self) -> Option<&Arc<Platform>> {
        self.platform.as_ref()
    }

    /// The platform, connecting on first use.
    pub async fn platform_or_connect(&mut self) -> Result<Arc<Platform>, String> {
        if let Some(platform) = &self.platform {
            return Ok(platform.clone());
        }
        let platform = Platform::connect().await?;
        self.platform = Some(platform.clone());
        Ok(platform)
    }

    /// Show a remote session, or (`None`) go back to the daemon's.
    pub fn set_remote(&mut self, remote: Option<RemoteSession>) {
        if let Some(previous) = self.remote.take() {
            previous.controller.close();
        }
        self.remote = remote;
    }

    /// Drop events that queued up for the session shown before.
    pub fn drain(&mut self) {
        if let Some((_, events)) = &mut self.daemon {
            while events.try_recv().is_ok() {}
        }
    }

    /// A session command for the session shown, with its raw response.
    pub async fn execute(&self, command: Command) -> Result<Response, ClientError> {
        if let Some(remote) = &self.remote {
            let name = command.name();
            return remote.controller.execute(command).await.map_err(|error| {
                ClientError::Command {
                    command: name.to_string(),
                    message: format!("{error:#}"),
                }
            });
        }
        match &self.daemon {
            Some((client, _)) => client.execute(command).await,
            None => Err(ClientError::Closed),
        }
    }

    /// [`execute`](Self::execute), turning `success: false` into an error.
    pub async fn execute_ok(&self, command: Command) -> Result<Option<Value>, ClientError> {
        let response = self.execute(command).await?;
        if response.success {
            Ok(response.data)
        } else {
            Err(ClientError::Command {
                command: response.command,
                message: response.error.unwrap_or_else(|| "failed".into()),
            })
        }
    }

    /// The next event of the session shown, or a daemon-level event. While
    /// a remote session is shown, the local daemon's news is about other
    /// sessions and is dropped.
    pub async fn recv(&mut self) -> Option<ClientEvent> {
        loop {
            let arrival = match (&mut self.remote, &mut self.daemon) {
                (Some(remote), Some((_, events))) => tokio::select! {
                    event = remote.events.recv() => Arrival::Remote(event),
                    event = events.recv() => Arrival::Daemon(event),
                },
                (Some(remote), None) => Arrival::Remote(remote.events.recv().await),
                (None, Some((_, events))) => return events.recv().await,
                (None, None) => return None,
            };
            match arrival {
                Arrival::Remote(event) => {
                    return event.map(|value| ClientEvent::Session(Event(value)));
                }
                // The daemon went away; the remote session carries on.
                Arrival::Daemon(None) => self.daemon = None,
                Arrival::Daemon(Some(_)) => {}
            }
        }
    }

    /// Stop the session shown: through its daemon, here or on its device.
    pub async fn stop_session(&self, session_id: &str) -> Result<(), ClientError> {
        if let Some(remote) = &self.remote {
            let response = remote
                .platform
                .host_commands(&remote.host_id)
                .call(&DaemonCommand::StopSession {
                    session_id: session_id.to_string(),
                })
                .await
                .map_err(|error| ClientError::Unavailable(format!("{error:#}")))?;
            return if response.success {
                Ok(())
            } else {
                Err(ClientError::Command {
                    command: response.command,
                    message: response.error.unwrap_or_else(|| "failed".into()),
                })
            };
        }
        match &self.daemon {
            Some((client, _)) => client.stop_session(session_id).await,
            None => Err(ClientError::Closed),
        }
    }

    /// Leave the session shown running and stop following it.
    pub async fn detach(&mut self) {
        if self.remote.is_some() {
            self.set_remote(None);
        } else if let Some((client, _)) = &self.daemon
            && !client.is_closed()
        {
            let _ = client.detach().await;
        }
    }

    /// Nothing is left to talk to.
    pub fn is_closed(&self) -> bool {
        self.remote.is_none() && self.daemon().is_none_or(DaemonClient::is_closed)
    }

    /// "SUPER daemon 0.0.41 (pid 4242)", or "linuxbox through the SUPER
    /// platform", for the line that introduces a session.
    pub fn describe(&self) -> String {
        match (&self.remote, self.daemon()) {
            (Some(remote), _) => format!("{} through the SUPER platform", remote.host_name),
            (None, Some(client)) => {
                let hello = client.hello();
                format!("SUPER daemon {} (pid {})", hello.daemon_version, hello.pid)
            }
            (None, None) => "no connection".into(),
        }
    }

    /// What `/remote` says in the attached terminal.
    pub fn status(&self, session_id: &str) -> String {
        match (&self.remote, self.daemon()) {
            (Some(remote), _) => format!(
                "♦ controlling session {} on {} through the SUPER platform",
                crate::sessions::short_id(session_id),
                remote.host_name
            ),
            (None, Some(client)) => crate::handover::attached_status(client, session_id),
            (None, None) => "not connected".into(),
        }
    }
}
