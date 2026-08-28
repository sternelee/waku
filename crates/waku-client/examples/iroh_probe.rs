//! One-shot E2E probe: dial the real waku-daemon over iroh using its
//! published ticket and exercise a round-trip.
use anyhow::Context as _;
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    let ticket_arg = std::env::args().nth(1).context("usage: probe <ticket>")?;
    let ticket: waku_protocol::RemoteTicket = ticket_arg.parse()?;
    println!("dialing daemon '{}' over iroh...", ticket.label);

    let client = waku_client::DaemonClient::connect_iroh(&ticket, Vec::new())?;
    println!("connected, fetching settings...");

    let response = client.request(
        uuid::Uuid::nil(),
        uuid::Uuid::nil(),
        waku_client::Command::GetSettings,
    )?;
    match response {
        waku_client::ResponsePayload::Settings { settings } => {
            println!("OK settings: computer_use_enabled={}", settings.computer_use_enabled);
        }
        other => anyhow::bail!("unexpected settings response: {other:?}"),
    }

    let state = client.request(
        uuid::Uuid::nil(),
        uuid::Uuid::nil(),
        waku_client::Command::LoadTaskState,
    )?;
    match state {
        waku_client::ResponsePayload::TaskState { sessions, .. } => {
            println!("OK task state: {} sessions", sessions.len());
        }
        other => anyhow::bail!("unexpected task-state response: {other:?}"),
    }

    client.shutdown();
    println!("probe OK");
    let _ = Duration::from_secs(1);
    Ok(())
}
