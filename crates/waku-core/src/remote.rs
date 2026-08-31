//! iroh P2P transport for the Waku daemon.
//!
//! The daemon process is synchronous: it binds a [`TcpListener`] and serves
//! blocking [`WebSocket`] connections on std threads. iroh is an async QUIC
//! stack, so this module runs a dedicated multi-thread tokio runtime on its
//! own std thread. Every accepted iroh connection is bridged into a
//! synchronous byte stream via a channel pair; the existing, transport-neutral
//! [`crate::server::run_message_loop`] then handles framing, authentication,
//! request dispatch, and subscriber fan-out exactly as it does for TCP.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use iroh::endpoint::presets;
use iroh::{Endpoint, RelayMode};
use tungstenite::protocol::{Role, WebSocketConfig};
use tungstenite::WebSocket;
use waku_protocol::{IROH_ALPN, IrohBridge, RemoteTicket};

use crate::server::{Hub, RequestDispatcher, ServerOptions};

/// A bound iroh endpoint whose ticket can be published before any connection
/// is accepted.
pub struct IrohEndpoint {
    runtime: tokio::runtime::Runtime,
    endpoint: Endpoint,
    ticket: RemoteTicket,
}

impl IrohEndpoint {
    /// Bind an iroh endpoint for the daemon's remote transport.
    ///
    /// `secret_key` is the daemon's stable iroh identity; the same key must be
    /// reused across restarts so a previously shared ticket keeps working.
    /// `token` is embedded in the ticket: holding the ticket is the full
    /// capability to dial and authenticate against this daemon.
    pub fn bind(secret_key: iroh::SecretKey, label: String, token: String) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .thread_name("waku-daemon-iroh")
            .enable_all()
            .build()
            .context("could not start iroh runtime")?;
        let endpoint = runtime
            .block_on(bind_endpoint(secret_key))
            .context("failed to bind iroh endpoint")?;
        let ticket = RemoteTicket::new(label, endpoint.addr(), token);
        Ok(Self {
            runtime,
            endpoint,
            ticket,
        })
    }

    pub fn ticket(&self) -> &RemoteTicket {
        &self.ticket
    }

    /// Start the accept loop on the daemon's hub and dispatcher.
    pub fn serve(
        self,
        token: String,
        dispatcher: Arc<RequestDispatcher>,
        hub: Arc<Hub>,
        shutdown: Arc<std::sync::atomic::AtomicBool>,
        options: Arc<ServerOptions>,
    ) -> IrohTransport {
        IrohTransport::start(self, token, dispatcher, hub, shutdown, options)
    }
}

/// Handle to a running iroh transport. Dropping it signals the accept thread
/// to stop; the thread owns the tokio runtime, so dropping the transport
/// eventually tears down the endpoint and any live connections.
pub struct IrohTransport {
    ticket: RemoteTicket,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    accept_thread: Option<std::thread::JoinHandle<()>>,
}

impl IrohTransport {
    fn start(
        endpoint: IrohEndpoint,
        token: String,
        dispatcher: Arc<RequestDispatcher>,
        hub: Arc<Hub>,
        shutdown: Arc<std::sync::atomic::AtomicBool>,
        options: Arc<ServerOptions>,
    ) -> Self {
        let IrohEndpoint {
            runtime,
            endpoint,
            ticket,
        } = endpoint;
        let accept_shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let accept_thread = std::thread::Builder::new()
            .name("waku-daemon-iroh-accept".into())
            .spawn({
                let runtime = runtime;
                let endpoint = endpoint.clone();
                let token = token;
                let hub = hub;
                let accept_shutdown = accept_shutdown.clone();
                let daemon_shutdown = shutdown;
                move || {
                    runtime.block_on(accept_loop(
                        endpoint,
                        token,
                        dispatcher,
                        hub,
                        accept_shutdown,
                        daemon_shutdown,
                        options,
                    ));
                }
            })
            .expect("iroh accept thread spawns");

        Self {
            ticket,
            shutdown: accept_shutdown,
            accept_thread: Some(accept_thread),
        }
    }

    pub fn ticket(&self) -> &RemoteTicket {
        &self.ticket
    }
}

impl Drop for IrohTransport {
    fn drop(&mut self) {
        self.shutdown.store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.accept_thread.take() {
            let _ = thread.join();
        }
    }
}

async fn bind_endpoint(secret_key: iroh::SecretKey) -> Result<Endpoint> {
    let relay_url = waku_protocol::resolve_relay_url()
        .context("failed to resolve iroh relay URL")?;
    let relay_map =
        iroh::RelayMap::from(relay_url).with_auth_token(secret_key.public().to_string());
    Endpoint::builder(presets::N0)
        .secret_key(secret_key)
        .relay_mode(RelayMode::Custom(relay_map))
        .alpns(vec![IROH_ALPN.to_vec()])
        .bind()
        .await
        .map_err(|error| anyhow!("failed to bind iroh endpoint: {error}"))
}

async fn accept_loop(
    endpoint: Endpoint,
    token: String,
    dispatcher: Arc<RequestDispatcher>,
    hub: Arc<Hub>,
    accept_shutdown: Arc<std::sync::atomic::AtomicBool>,
    daemon_shutdown: Arc<std::sync::atomic::AtomicBool>,
    options: Arc<ServerOptions>,
) {
    loop {
        // Poll shutdown without blocking the runtime worker on `accept()`.
        if accept_shutdown.load(std::sync::atomic::Ordering::Acquire)
            || daemon_shutdown.load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let incoming = tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(100)) => continue,
            incoming = endpoint.accept() => incoming,
        };
        let Some(incoming) = incoming else {
            // Endpoint closed.
            return;
        };
        let Ok(accepting) = incoming.accept() else {
            continue;
        };
        let accepted = async move {
            // Wait for the QUIC handshake; then let the connection drop if
            // the peer never opens a bi stream.
            let Ok(connection) = accepting.await else {
                return None;
            };
            let accept_bi = connection.accept_bi();
            let Ok((send, recv)) = accept_bi.await else {
                return None;
            };
            Some((send, recv))
        };
        let Some((send, recv)) = accepted.await else {
            continue;
        };

        let dispatcher = dispatcher.clone();
        let token = token.clone();
        let hub = hub.clone();
        let daemon_shutdown = daemon_shutdown.clone();
        let options = options.clone();
        let runtime_handle = tokio::runtime::Handle::current();
        // The message loop is a blocking std-thread loop; run it on the
        // blocking pool so a long-lived connection never occupies a runtime
        // worker.
        tokio::task::spawn_blocking(move || {
            let bridge = IrohBridge::new(send, recv, &runtime_handle);
            let config = WebSocketConfig::default()
                .max_message_size(Some(crate::protocol::MAX_WIRE_MESSAGE_BYTES))
                .max_frame_size(Some(crate::protocol::MAX_WIRE_MESSAGE_BYTES));
            let socket = WebSocket::from_raw_socket(bridge, Role::Server, Some(config));
            if let Err(error) = crate::server::run_message_loop(
                socket,
                &token,
                crate::server::PeerScope::RemoteIroh,
                dispatcher,
                hub,
                daemon_shutdown,
                &options,
            ) {
                eprintln!("waku-daemon iroh connection ended: {error:#}");
            }
        });
    }
}

