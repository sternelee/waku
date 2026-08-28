use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use parking_lot::Mutex;
use tungstenite::protocol::WebSocketConfig;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use uuid::Uuid;

use waku_protocol::MAX_WIRE_MESSAGE_BYTES;
use waku_protocol::{
    ClientMessage, Command, IROH_ALPN, IrohBridge, PROTOCOL_VERSION, RemoteTicket, ReplayCursor,
    Request, ResponseOutcome, ResponsePayload, RpcError, SequencedEvent, ServerMessage,
};

const READ_POLL_INTERVAL: Duration = Duration::from_millis(25);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_BUFFERED_EVENTS_PER_RUNTIME: usize = 4096;

enum Outgoing {
    Message(ClientMessage),
    Shutdown,
}

struct ClientInner {
    outgoing: Sender<Outgoing>,
    pending: Mutex<HashMap<Uuid, Sender<Result<ResponsePayload, RpcError>>>>,
    sessions: Mutex<HashMap<(Uuid, Uuid), Sender<SequencedEvent>>>,
    pending_events: Mutex<HashMap<(Uuid, Uuid), VecDeque<SequencedEvent>>>,
    task_state_subscribers: Mutex<Vec<Sender<u64>>>,
    last_sequences: Mutex<HashMap<(Uuid, Uuid), LastSequence>>,
    disconnected: AtomicBool,
    /// iroh runtime and endpoint for an iroh-backed connection, kept alive
    /// until the client is dropped.
    iroh_keepalive: Mutex<Option<IrohKeepalive>>,
}

/// Owns the tokio runtime and endpoint behind an iroh connection. Dropping
/// this closes the QUIC connection, which is what tears down the socket
/// thread when the last [`DaemonClient`] clone goes away. The fields are
/// never read; holding them is the point.
#[allow(dead_code)]
struct IrohKeepalive {
    runtime: tokio::runtime::Runtime,
    endpoint: iroh::Endpoint,
}

#[derive(Clone, Copy)]
struct LastSequence {
    epoch: Uuid,
    sequence: u64,
}

#[derive(Clone)]
pub struct DaemonClient {
    inner: Arc<ClientInner>,
}

impl DaemonClient {
    pub fn connect(address: &str, token: String) -> anyhow::Result<Self> {
        Self::connect_with_resume(address, token, Vec::new())
    }

    pub fn connect_with_resume(
        address: &str,
        token: String,
        resume_from: Vec<ReplayCursor>,
    ) -> anyhow::Result<Self> {
        let url = daemon_url(address)?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_WIRE_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_WIRE_MESSAGE_BYTES));
        let (mut socket, _) =
            tungstenite::client::connect_with_config(url.as_str(), Some(config), 3)
                .context("could not connect to Waku daemon")?;
        set_client_read_timeout(&mut socket, Some(Duration::from_secs(5)))?;
        finish_handshake(&mut socket, &token, &resume_from)?;
        set_client_read_timeout(&mut socket, Some(READ_POLL_INTERVAL))?;
        Ok(Self::from_socket(socket, &resume_from))
    }

    /// Connect over iroh P2P using a daemon's published ticket.
    ///
    /// The ticket is a full capability: it carries both the daemon's dialable
    /// endpoint address and the wire token that authenticates the `Hello`
    /// exactly as it does over WebSocket. The tokio runtime and endpoint are
    /// kept alive by the returned client for the lifetime of the connection.
    pub fn connect_iroh(
        ticket: &RemoteTicket,
        resume_from: Vec<ReplayCursor>,
    ) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .thread_name("waku-daemon-client-iroh")
            .enable_all()
            .build()
            .context("could not start iroh client runtime")?;
        let endpoint = runtime
            .block_on(bind_client_endpoint())
            .context("failed to bind iroh client endpoint")?;
        let connection = runtime
            .block_on(endpoint.connect(ticket.endpoint_addr.clone(), IROH_ALPN))
            .context("could not connect to Waku daemon over iroh")?;
        let (send, recv) = runtime
            .block_on(connection.open_bi())
            .context("could not open iroh control stream")?;

        let bridge = IrohBridge::new(send, recv, runtime.handle());
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_WIRE_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_WIRE_MESSAGE_BYTES));
        let mut socket =
            WebSocket::from_raw_socket(bridge, tungstenite::protocol::Role::Client, Some(config));
        finish_handshake(&mut socket, &ticket.token, &resume_from)?;
        Ok(Self::from_socket_iroh(socket, runtime, endpoint, &resume_from))
    }

    fn from_socket<S: std::io::Read + std::io::Write + Send + 'static>(
        socket: WebSocket<S>,
        resume_from: &[ReplayCursor],
    ) -> Self {
        let last_sequences = last_sequences_from(resume_from);
        let (outgoing, outgoing_rx) = unbounded();
        let inner = Arc::new(ClientInner {
            outgoing,
            pending: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            pending_events: Mutex::new(HashMap::new()),
            task_state_subscribers: Mutex::new(Vec::new()),
            last_sequences: Mutex::new(last_sequences),
            disconnected: AtomicBool::new(false),
            iroh_keepalive: Mutex::new(None),
        });
        let thread_inner = inner.clone();
        std::thread::Builder::new()
            .name("waku-daemon-client".into())
            .spawn(move || run_client(socket, outgoing_rx, thread_inner))
            .expect("Waku daemon client thread spawns");
        Self { inner }
    }

    fn from_socket_iroh(
        socket: WebSocket<IrohBridge>,
        runtime: tokio::runtime::Runtime,
        endpoint: iroh::Endpoint,
        resume_from: &[ReplayCursor],
    ) -> Self {
        let client = Self::from_socket(socket, resume_from);
        // Keep the runtime and endpoint alive for the life of the connection;
        // dropping either would close the QUIC streams underneath the socket
        // thread.
        client
            .inner
            .iroh_keepalive
            .lock()
            .replace(IrohKeepalive { runtime, endpoint });
        client
    }

    pub fn subscribe(&self, session_id: Uuid, runtime_id: Uuid) -> Receiver<SequencedEvent> {
        let (events, receiver) = unbounded();
        let key = (session_id, runtime_id);
        let mut sessions = self.inner.sessions.lock();
        sessions.insert(key, events.clone());
        // Keep the subscription lock while draining the pre-subscription
        // replay queue. The socket thread takes these locks in the same order,
        // so a new live event cannot overtake older replayed events here.
        if let Some(buffered) = self.inner.pending_events.lock().remove(&key) {
            for event in buffered {
                let _ = events.send(event);
            }
        }
        receiver
    }

    /// Whether two handles send through the same WebSocket connection.
    ///
    /// The daemon supervisor publishes replacement clients after a managed
    /// restart. Runtime adapters use this identity check to ignore the
    /// subscription's initial snapshot and wait for an actual replacement.
    pub fn same_connection(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    pub fn is_disconnected(&self) -> bool {
        self.inner.disconnected.load(Ordering::Acquire)
    }

    pub fn unsubscribe(&self, session_id: Uuid, runtime_id: Uuid) {
        self.inner.sessions.lock().remove(&(session_id, runtime_id));
    }

    pub fn subscribe_task_state(&self) -> Receiver<u64> {
        let (events, receiver) = unbounded();
        self.inner.task_state_subscribers.lock().push(events);
        receiver
    }

    pub fn request(
        &self,
        session_id: Uuid,
        runtime_id: Uuid,
        command: Command,
    ) -> anyhow::Result<ResponsePayload> {
        if self.inner.disconnected.load(Ordering::Acquire) {
            bail!("Waku daemon is disconnected");
        }
        let request_id = Uuid::new_v4();
        let (response, response_rx) = bounded(1);
        self.inner.pending.lock().insert(request_id, response);
        let message = ClientMessage::Request(Request {
            request_id,
            session_id,
            runtime_id,
            command,
        });
        if self
            .inner
            .outgoing
            .send(Outgoing::Message(message))
            .is_err()
        {
            self.inner.pending.lock().remove(&request_id);
            bail!("Waku daemon connection is closed");
        }
        match response_rx.recv_timeout(REQUEST_TIMEOUT) {
            Ok(Ok(payload)) => Ok(payload),
            Ok(Err(error)) => Err(anyhow!(error.message)),
            Err(error) => {
                self.inner.pending.lock().remove(&request_id);
                Err(anyhow!("timed out waiting for Waku daemon: {error}"))
            }
        }
    }

    pub fn notify(
        &self,
        session_id: Uuid,
        runtime_id: Uuid,
        command: Command,
    ) -> anyhow::Result<()> {
        if self.inner.disconnected.load(Ordering::Acquire) {
            bail!("Waku daemon is disconnected");
        }
        self.inner
            .outgoing
            .send(Outgoing::Message(ClientMessage::Request(Request {
                // The nil request id is reserved for fire-and-forget controls;
                // the daemon executes them in the runtime mailbox but does
                // not allocate or send a response.
                request_id: Uuid::nil(),
                session_id,
                runtime_id,
                command,
            })))
            .map_err(|_| anyhow!("Waku daemon connection is closed"))
    }

    pub fn last_sequences(&self) -> Vec<ReplayCursor> {
        self.inner
            .last_sequences
            .lock()
            .iter()
            .map(|(&(session_id, runtime_id), cursor)| ReplayCursor {
                session_id,
                runtime_id,
                epoch: cursor.epoch,
                sequence: cursor.sequence,
            })
            .collect()
    }

    pub fn shutdown(&self) {
        let _ = self.inner.outgoing.send(Outgoing::Shutdown);
    }
}

fn daemon_url(address: &str) -> anyhow::Result<String> {
    let normalized = if address.starts_with("ws://") || address.starts_with("wss://") {
        address.to_owned()
    } else {
        format!("ws://{address}")
    };
    let mut url = url::Url::parse(&normalized).context("Waku daemon address is invalid")?;
    url.set_path("/v1");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.into())
}

fn run_client<S: std::io::Read + std::io::Write>(
    mut socket: WebSocket<S>,
    outgoing: Receiver<Outgoing>,
    inner: Arc<ClientInner>,
) {
    'connection: loop {
        while let Ok(message) = outgoing.try_recv() {
            match message {
                Outgoing::Message(message) => {
                    if write_json(&mut socket, &message).is_err() {
                        break 'connection;
                    }
                }
                Outgoing::Shutdown => {
                    let _ = write_json(&mut socket, &ClientMessage::Shutdown);
                    let _ = socket.flush();
                    break 'connection;
                }
            }
        }

        match socket.read() {
            Ok(Message::Text(text)) => {
                let Ok(message) = serde_json::from_str::<ServerMessage>(text.as_ref()) else {
                    continue;
                };
                match message {
                    ServerMessage::Response {
                        request_id,
                        outcome,
                    } => {
                        if let Some(pending) = inner.pending.lock().remove(&request_id) {
                            let result = match outcome {
                                ResponseOutcome::Ok { payload } => Ok(payload),
                                ResponseOutcome::Error { error } => Err(error),
                            };
                            let _ = pending.send(result);
                        }
                    }
                    ServerMessage::Event(event) => {
                        let should_deliver = {
                            let mut sequences = inner.last_sequences.lock();
                            let previous = sequences
                                .entry((event.session_id, event.runtime_id))
                                .or_insert(LastSequence {
                                    epoch: event.epoch,
                                    sequence: 0,
                                });
                            if previous.epoch == event.epoch && event.sequence <= previous.sequence
                            {
                                false
                            } else {
                                previous.epoch = event.epoch;
                                previous.sequence = event.sequence;
                                true
                            }
                        };
                        if should_deliver {
                            let key = (event.session_id, event.runtime_id);
                            let sessions = inner.sessions.lock();
                            if let Some(events) = sessions.get(&key) {
                                let _ = events.send(event);
                            } else {
                                let mut pending = inner.pending_events.lock();
                                let buffered = pending.entry(key).or_default();
                                buffered.push_back(event);
                                while buffered.len() > MAX_BUFFERED_EVENTS_PER_RUNTIME {
                                    buffered.pop_front();
                                }
                            }
                        }
                    }
                    ServerMessage::TaskStateChanged { revision } => {
                        inner
                            .task_state_subscribers
                            .lock()
                            .retain(|subscriber| subscriber.send(revision).is_ok());
                    }
                    ServerMessage::ShuttingDown => break,
                    ServerMessage::Hello { .. } | ServerMessage::Rejected { .. } => {}
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(_)) => {
                let _ = socket.flush();
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(error)) if retryable_io(&error) => {}
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => break,
            Err(_) => break,
        }
    }

    inner.disconnected.store(true, Ordering::Release);
    let pending = std::mem::take(&mut *inner.pending.lock());
    for (_, response) in pending {
        let _ = response.send(Err(RpcError {
            message: "Waku daemon disconnected".into(),
        }));
    }
    // Closing the desktop transport is not evidence that a daemon-owned
    // provider exited. Drop the subscription senders so runtime adapters can
    // hand off to a replacement client and ask the daemon whether the same
    // runtime still exists. Real provider exits arrive through the replayable
    // `processExited` event emitted by the daemon.
    drop(std::mem::take(&mut *inner.sessions.lock()));
    inner.task_state_subscribers.lock().clear();
}

/// Exchange the wire `Hello` with the daemon and verify the response. Shared
/// by the WebSocket and iroh transports; both speak the same versioned JSON
/// framing.
fn finish_handshake<S: std::io::Read + std::io::Write>(
    socket: &mut WebSocket<S>,
    token: &str,
    resume_from: &[ReplayCursor],
) -> anyhow::Result<()> {
    write_json(
        socket,
        &ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            token: token.to_owned(),
            client_id: Uuid::new_v4(),
            resume_from: resume_from.to_vec(),
        },
    )?;
    match read_server_message(socket)? {
        ServerMessage::Hello {
            protocol_version, ..
        } if protocol_version == PROTOCOL_VERSION => {}
        ServerMessage::Hello {
            protocol_version, ..
        } => bail!(
            "daemon protocol {protocol_version} does not match desktop protocol {PROTOCOL_VERSION}"
        ),
        ServerMessage::Rejected { message } => bail!("daemon rejected connection: {message}"),
        other => bail!("daemon sent an invalid handshake response: {other:?}"),
    }
    Ok(())
}

fn last_sequences_from(resume_from: &[ReplayCursor]) -> HashMap<(Uuid, Uuid), LastSequence> {
    resume_from
        .iter()
        .map(|cursor| {
            (
                (cursor.session_id, cursor.runtime_id),
                LastSequence {
                    epoch: cursor.epoch,
                    sequence: cursor.sequence,
                },
            )
        })
        .collect()
}

async fn bind_client_endpoint() -> anyhow::Result<iroh::Endpoint> {
    let relay_url = waku_protocol::resolve_relay_url()?;
    let secret_key = iroh::SecretKey::generate();
    let relay_map =
        iroh::RelayMap::from(relay_url).with_auth_token(secret_key.public().to_string());
    iroh::endpoint::Endpoint::builder(iroh::endpoint::presets::N0)
        .secret_key(secret_key)
        .relay_mode(iroh::RelayMode::Custom(relay_map))
        .alpns(vec![IROH_ALPN.to_vec()])
        .bind()
        .await
        .map_err(|error| anyhow::anyhow!("failed to bind iroh client endpoint: {error}"))
}

fn set_client_read_timeout(
    socket: &mut WebSocket<MaybeTlsStream<TcpStream>>,
    timeout: Option<Duration>,
) -> io::Result<()> {
    match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream.set_read_timeout(timeout),
        MaybeTlsStream::Rustls(stream) => stream.sock.set_read_timeout(timeout),
        #[allow(unreachable_patterns)]
        _ => Ok(()),
    }
}

fn retryable_io(error: &io::Error) -> bool {
    retryable_error(error)
}

fn retryable_error(error: &(dyn std::error::Error + 'static)) -> bool {
    if let Some(error) = error.downcast_ref::<io::Error>() {
        if matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
        ) {
            return true;
        }
        #[cfg(unix)]
        if error.raw_os_error() == Some(libc::EAGAIN)
            || error.raw_os_error() == Some(libc::EWOULDBLOCK)
        {
            return true;
        }
    }
    error.source().is_some_and(retryable_error)
}

fn write_json<S: io::Read + io::Write, T: serde::Serialize>(
    socket: &mut WebSocket<S>,
    value: &T,
) -> anyhow::Result<()> {
    let payload = serde_json::to_string(value)?;
    socket.send(Message::Text(payload.into()))?;
    Ok(())
}

fn read_server_message<S: std::io::Read + std::io::Write>(
    socket: &mut WebSocket<S>,
) -> anyhow::Result<ServerMessage> {
    loop {
        match socket.read()? {
            Message::Text(text) => return Ok(serde_json::from_str(text.as_ref())?),
            Message::Ping(_) => socket.flush()?,
            Message::Close(_) => bail!("Waku daemon closed during handshake"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_endpoint_accepts_addresses_and_secure_urls() {
        assert_eq!(
            daemon_url("127.0.0.1:4312").unwrap(),
            "ws://127.0.0.1:4312/v1"
        );
        assert_eq!(
            daemon_url("wss://waku.example.test/old?ignored=1").unwrap(),
            "wss://waku.example.test/v1"
        );
    }
}
