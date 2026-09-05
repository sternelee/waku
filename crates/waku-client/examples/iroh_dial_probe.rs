//! Dial the running daemon's iroh endpoint directly (same path as a phone)
//! to observe the full error chain, without needing the QR ticket. Reads the
//! daemon secret from `temp/iroh-secret.key` to compute its NodeId, then
//! dials by NodeId (relay path) with the same WebSocket-over-iroh framing
//! the mobile client uses.

use std::time::Duration;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let token = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "f107260e363844b0827b369c06f7b139".into());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async_main(token))
}

async fn async_main(token: String) -> anyhow::Result<()> {
    let secret_path = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "temp/iroh-secret.key".into());
    let text = std::fs::read_to_string(&secret_path)?.trim().to_owned();
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(text.as_bytes())
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(text.as_bytes()))
        .map_err(|error| anyhow::anyhow!("decode secret key: {error}"))?;
    let daemon_secret = iroh::SecretKey::from_bytes(
        &bytes
            .clone()
            .try_into()
            .map_err(|_: Vec<u8>| anyhow::anyhow!("secret key must be 32 bytes, got {}", bytes.len()))?,
    );
    let node_id = daemon_secret.public();
    println!("daemon node id: {node_id}");

    let relay_url = waku_protocol::resolve_relay_url()?;
    println!("relay: {relay_url}");

    let client_secret = iroh::SecretKey::generate();
    let relay_map =
        iroh::RelayMap::from(relay_url).with_auth_token(client_secret.public().to_string());
    let endpoint = iroh::endpoint::Endpoint::builder(iroh::endpoint::presets::N0)
        .secret_key(client_secret)
        .relay_mode(iroh::RelayMode::Custom(relay_map))
        .alpns(vec![waku_protocol::IROH_ALPN.to_vec()])
        .bind()
        .await
        .map_err(|error| anyhow::anyhow!("bind: {error}"))?;

    let started = Instant::now();
    // Dial by NodeId only — forces the relay path, like a phone on a hostile
    // network where the ticket's direct addresses are unreachable.
    let addr = iroh::EndpointAddr::new(node_id);
    let connection = endpoint
        .connect(addr, waku_protocol::IROH_ALPN)
        .await
        .map_err(|error| anyhow::anyhow!("connect failed after {started:?}: {error:#}"))?;
    println!("QUIC connected in {started:?}");

    let (send, recv) = connection
        .open_bi()
        .await
        .map_err(|error| anyhow::anyhow!("open_bi: {error:#}"))?;

    let runtime_handle = tokio::runtime::Handle::current();
    // IrohBridge blocks on channels in its sync Read/Write impls, so the
    // tungstenite loop must run off the async runtime.
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    std::thread::spawn(move || {
        let outcome = wire_loop(send, recv, runtime_handle, token, started);
        let _ = tx.send(outcome.map_err(|e| format!("{e:#}")));
    });
    match rx.recv_timeout(Duration::from_secs(90)) {
        Ok(Ok(())) => println!("probe done"),
        Ok(Err(error)) => anyhow::bail!("{error}"),
        Err(_) => anyhow::bail!("probe timed out"),
    }
    Ok(())
}

fn wire_loop(
    send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
    runtime_handle: tokio::runtime::Handle,
    token: String,
    started: Instant,
) -> anyhow::Result<()> {
    let bridge = waku_protocol::IrohBridge::new(send, recv, &runtime_handle);
    let mut socket =
        tungstenite::WebSocket::from_raw_socket(bridge, tungstenite::protocol::Role::Client, None);

    let payload = serde_json::json!({
        "type": "hello",
        "protocolVersion": waku_protocol::PROTOCOL_VERSION,
        "token": token,
        "clientId": uuid::Uuid::new_v4().to_string(),
        "resumeFrom": [],
    });
    socket
        .write_message(tungstenite::Message::text(payload.to_string()))
        .map_err(|error| anyhow::anyhow!("write hello: {error:#}"))?;
    let reply = read_retry(&mut socket, "hello reply")?;
    println!("daemon hello reply: {reply}");

    // Follow the mobile client: request settings right after the hello.
    let request_id = uuid::Uuid::new_v4();
    let request = serde_json::json!({
        "type": "request",
        "requestId": request_id.to_string(),
        "sessionId": uuid::Uuid::nil().to_string(),
        "runtimeId": uuid::Uuid::nil().to_string(),
        "command": { "type": "getSettings" },
    });
    socket
        .write_message(tungstenite::Message::text(request.to_string()))
        .map_err(|error| anyhow::anyhow!("write request: {error:#}"))?;
    println!("request sent, waiting for response…");
    let reply = read_retry(&mut socket, "settings response")?;
    println!(
        "response ({} bytes): {}…",
        reply.to_string().len(),
        &reply.to_string()[..reply.to_string().len().min(200)]
    );

    // Keep the connection open briefly and echo any pushed messages, to test
    // whether the daemon can push over the relay path.
    let push_started = Instant::now();
    loop {
        let message = socket
            .read_message()
            .map_err(|error| anyhow::anyhow!("read after {started:?}: {error:#}"))?;
        let text = message.to_string();
        println!(
            "[+{:?}] push: {}…",
            push_started.elapsed(),
            &text[..text.len().min(120)]
        );
        if push_started.elapsed() > Duration::from_secs(15) {
            break;
        }
    }
    Ok(())
}

/// The iroh bridge reads with a short poll that surfaces `WouldBlock`; the
/// real client loops retry it. Mirror that here.
fn read_retry(
    socket: &mut tungstenite::WebSocket<waku_protocol::IrohBridge>,
    label: &str,
) -> anyhow::Result<tungstenite::Message> {
    loop {
        match socket.read_message() {
            Ok(message) => return Ok(message),
            Err(tungstenite::Error::Io(error))
                if error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                continue;
            }
            Err(error) => anyhow::bail!("read {label}: {error:#}"),
        }
    }
}
