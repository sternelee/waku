//! Shareable connect ticket for reaching a Waku daemon over iroh P2P.
//!
//! The design mirrors dumbpipex's `ConnectTicket`: the daemon publishes a
//! URL-safe base64 JSON blob containing its iroh [`EndpointAddr`], and any
//! Waku client can dial that endpoint with [`IROH_ALPN`]. The daemon's own
//! `WAKU_DAEMON_TOKEN` still authenticates the wire-level `Hello`, so a
//! leaked ticket alone grants no access.

use std::fmt::{Display, Formatter};
use std::io;
use std::str::FromStr;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use crossbeam_channel::{Receiver, Sender, unbounded};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Application-layer protocol the daemon serves over iroh. Versioned
/// independently of the WebSocket path so the two transports can evolve.
pub const IROH_ALPN: &[u8] = b"waku-daemon-iroh-v1";

/// Environment variable that overrides the iroh relay URL used by the daemon
/// and remote clients. When unset, the official number0 relay is used.
pub const IROH_RELAY_URL_ENV: &str = "WAKU_IROH_RELAY_URL";

/// Hostname of the official number0 NA-East iroh relay. Mirrors
/// `iroh::defaults::NA_EAST_RELAY_HOSTNAME`.
pub const OFFICIAL_RELAY_URL: &str = "https://use1-1.relay.n0.iroh-canary.iroh.link";

/// Resolve the iroh relay URL, honoring [`IROH_RELAY_URL_ENV`] at runtime.
pub fn resolve_relay_url() -> anyhow::Result<iroh::RelayUrl> {
    match std::env::var(IROH_RELAY_URL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(value) => value
            .parse::<iroh::RelayUrl>()
            .map_err(|error| anyhow::anyhow!("invalid {IROH_RELAY_URL_ENV} value {value:?}: {error}")),
        None => OFFICIAL_RELAY_URL
            .parse::<iroh::RelayUrl>()
            .map_err(|error| anyhow::anyhow!("invalid official iroh relay URL constant: {error}")),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteTicket {
    /// Ticket format version. Bump on incompatible layout changes.
    pub version: u32,
    /// Human-readable label, typically the daemon host's name.
    pub label: String,
    /// The daemon's dialable iroh endpoint address.
    pub endpoint_addr: iroh::EndpointAddr,
    /// The daemon's wire token. The ticket is a full capability: whoever
    /// holds it can dial the daemon and authenticate as its owner.
    pub token: String,
}

impl RemoteTicket {
    pub fn new(label: String, endpoint_addr: iroh::EndpointAddr, token: String) -> Self {
        Self {
            version: 1,
            label,
            endpoint_addr,
            token,
        }
    }

    pub fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).expect("ticket serializes"))
    }
}

impl Display for RemoteTicket {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.encode())
    }
}

impl FromStr for RemoteTicket {
    type Err = anyhow::Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bytes = URL_SAFE_NO_PAD
            .decode(text.trim())
            .map_err(|_| anyhow::anyhow!("ticket is not valid URL-safe base64"))?;
        let ticket: Self = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("ticket payload is not a Waku remote ticket"))?;
        if ticket.version != 1 {
            anyhow::bail!("ticket version {} is unsupported", ticket.version);
        }
        Ok(ticket)
    }
}

/// Poll cadence for bridging an iroh QUIC stream into a synchronous
/// [`io::Read`] / [`io::Write`]. Matches the WebSocket transport's
/// SO_RCVTIMEO contract so a shared message loop sees `WouldBlock` and
/// drains its outgoing queue between reads.
pub const IROH_POLL_INTERVAL: Duration = Duration::from_millis(25);
const IROH_CHUNK_CAPACITY: usize = 64 * 1024;

/// A synchronous byte stream backed by an iroh QUIC bi-stream.
///
/// A tokio task pumps `RecvStream` bytes into an unbounded channel that
/// [`Read`] drains with a short poll; writes go through a bounded channel to
/// a task that `write_all`s them to the `SendStream`. Reads time out with
/// `WouldBlock`, matching the TCP transport's SO_RCVTIMEO polling contract.
///
/// The runtime must outlive the bridge; pump tasks are spawned on the given
/// handle, so the caller owns the [`tokio::runtime::Runtime`].
pub struct IrohBridge {
    recv_rx: Receiver<Vec<u8>>,
    send_tx: mpsc::Sender<Vec<u8>>,
    pending: Vec<u8>,
    eof: bool,
    /// Bytes read from `recv_rx` but not yet consumed by `Read`.
    read_buf: Vec<u8>,
}

impl IrohBridge {
    pub fn new(
        send: iroh::endpoint::SendStream,
        recv: iroh::endpoint::RecvStream,
        runtime_handle: &tokio::runtime::Handle,
    ) -> Self {
        let (recv_tx, recv_rx) = unbounded();
        let (send_tx, send_rx) = mpsc::channel(64);
        runtime_handle.spawn(pump_recv(recv, recv_tx));
        runtime_handle.spawn(pump_send(send_rx, send));
        Self {
            recv_rx,
            send_tx,
            pending: Vec::new(),
            eof: false,
            read_buf: Vec::new(),
        }
    }
}

async fn pump_recv(mut recv: iroh::endpoint::RecvStream, tx: Sender<Vec<u8>>) {
    let mut buf = vec![0u8; IROH_CHUNK_CAPACITY];
    loop {
        match recv.read(&mut buf).await {
            Ok(None) => break,
            Ok(Some(n)) => {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

async fn pump_send(mut rx: mpsc::Receiver<Vec<u8>>, mut send: iroh::endpoint::SendStream) {
    while let Some(chunk) = rx.recv().await {
        if send.write_all(&chunk).await.is_err() {
            break;
        }
    }
}

impl io::Read for IrohBridge {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        // Drain previously buffered bytes first.
        if !self.read_buf.is_empty() {
            let n = buf.len().min(self.read_buf.len());
            buf[..n].copy_from_slice(&self.read_buf[..n]);
            self.read_buf.drain(..n);
            return Ok(n);
        }
        if self.eof {
            return Ok(0);
        }
        match self.recv_rx.recv_timeout(IROH_POLL_INTERVAL) {
            Ok(chunk) => {
                let n = buf.len().min(chunk.len());
                buf[..n].copy_from_slice(&chunk[..n]);
                self.read_buf = chunk[n..].to_vec();
                Ok(n)
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                Err(io::Error::new(io::ErrorKind::WouldBlock, "iroh poll"))
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                self.eof = true;
                Ok(0)
            }
        }
    }
}

impl io::Write for IrohBridge {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let pending = std::mem::take(&mut self.pending);
        if !pending.is_empty() {
            self.send_tx
                .blocking_send(pending)
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "iroh send closed"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::{EndpointAddr, EndpointId, RelayUrl};

    fn test_addr() -> EndpointAddr {
        let key = iroh::SecretKey::generate();
        EndpointAddr {
            id: key.public().into(),
            addrs: Default::default(),
        }
    }

    #[test]
    fn ticket_round_trips() {
        let ticket = RemoteTicket::new("dev-mac".into(), test_addr(), "secret".into());
        let parsed: RemoteTicket = ticket.to_string().parse().unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.label, "dev-mac");
        assert_eq!(
            parsed.endpoint_addr.id.as_bytes(),
            ticket.endpoint_addr.id.as_bytes()
        );
    }

    #[test]
    fn ticket_rejects_garbage_and_wrong_versions() {
        assert!("not a ticket".parse::<RemoteTicket>().is_err());
        let wrong_version = serde_json::to_vec(&serde_json::json!({
            "version": 99,
            "label": "x",
            "endpoint_addr": test_addr(),
        }))
        .unwrap();
        let encoded = URL_SAFE_NO_PAD.encode(wrong_version);
        assert!(encoded.parse::<RemoteTicket>().is_err());
    }

    #[test]
    fn relay_url_parses() {
        let url: RelayUrl = "https://use1-1.relay.n0.iroh-canary.iroh.link"
            .parse()
            .unwrap();
        assert_eq!(url.as_str(), "https://use1-1.relay.n0.iroh-canary.iroh.link/");
    }
}
