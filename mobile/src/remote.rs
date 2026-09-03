//! Remote bridge — the mobile app's entire relationship with a Waku daemon
//! host.
//!
//! The phone is a pure remote client in the same role as the desktop app's
//! secondary iroh connection: it reads the shared session catalog, attaches to
//! live runtimes, streams driver events, and sends prompts and control
//! commands. Nothing here runs providers or touches Git; all such work happens
//! on the daemon host.

use anyhow::{Context as _, anyhow, bail};
use uuid::Uuid;
use waku_client::{
    Command, DaemonClient, DaemonSupervisor, RemoteTicket, ResponsePayload,
    WireDriverStartOptions, encode_enum,
};
use waku_client::model::ProviderProbe;
use waku_client::model::AgentSession;

/// Dial `ticket` and return the supervisor plus a client handle. Blocking —
/// run it on a background executor, never on the UI thread.
pub fn connect(ticket: &str) -> anyhow::Result<(DaemonSupervisor, DaemonClient, String)> {
    let parsed = ticket
        .trim()
        .parse::<RemoteTicket>()
        .map_err(|error| anyhow!("票据无法解析：{error}"))?;
    let label = parsed.label.clone();
    let supervisor = DaemonSupervisor::connect_iroh(ticket.trim())
        .context("无法连接远程 Waku 主机")?;
    let client = supervisor.client();
    Ok((supervisor, client, label))
}

/// The shared session catalog: only sessions the owner marked for remote sync
/// ever reach this side — the daemon filters both the catalog and the live
/// event stream.
#[derive(Clone)]
pub struct Catalog {
    pub sessions: Vec<AgentSession>,
    /// Daemon-owned projects, used to resolve a session's working directory
    /// when the session works directly in the project checkout
    /// (`SessionWorkspace::Local` carries no path of its own).
    pub projects: Vec<waku_client::model::Project>,
    /// Fallback working directory for sessions without a project.
    pub default_cwd: std::path::PathBuf,
}

/// Fetch the shared session catalog (list projections: no transcript bodies).
pub fn load_catalog(client: &DaemonClient) -> anyhow::Result<Catalog> {
    let response = client.request(Uuid::nil(), Uuid::nil(), Command::LoadTaskState)?;
    let ResponsePayload::TaskState {
        sessions,
        projects,
        default_cwd,
        ..
    } = response
    else {
        bail!("远程主机返回了无效的会话目录");
    };
    Ok(Catalog {
        sessions,
        projects,
        default_cwd,
    })
}

/// Fetch one session's full transcript.
pub fn hydrate(client: &DaemonClient, session_id: Uuid) -> anyhow::Result<AgentSession> {
    let response = client.request(
        Uuid::nil(),
        Uuid::nil(),
        Command::HydrateSession { session_id },
    )?;
    let ResponsePayload::Session { session } = response else {
        bail!("远程主机返回了无效的会话内容");
    };
    session.ok_or_else(|| anyhow!("会话在远程主机上已不存在"))
}

/// A live runtime on the daemon host, ready to receive prompts.
pub struct AttachedRuntime {
    pub runtime_id: Uuid,
    pub supports_steer: bool,
}

/// Attach to the session's live runtime. A session that has never run has no
/// runtime yet — for those, probe the daemon host for the provider binary and
/// start one, exactly like the owner's desktop would on its first prompt. All
/// provider work happens on the daemon host; the phone only sends options.
pub fn ensure_runtime(
    client: &DaemonClient,
    session: &AgentSession,
    catalog: &Catalog,
) -> anyhow::Result<AttachedRuntime> {
    let response = client.request(session.id, Uuid::nil(), Command::AttachSession)?;
    let ResponsePayload::SessionRuntime {
        runtime_id,
        supports_steer,
    } = response
    else {
        bail!("远程主机返回了无效的运行时附着响应");
    };
    if let Some(runtime_id) = runtime_id {
        return Ok(AttachedRuntime {
            runtime_id,
            supports_steer,
        });
    }
    start_runtime(client, session, catalog)
}

/// Resolve the working directory the daemon should run the session in.
/// A worktree session carries its own path; a session working directly in the
/// project checkout resolves to the project's root path.
fn session_cwd(
    session: &AgentSession,
    catalog: &Catalog,
) -> Option<std::path::PathBuf> {
    session
        .workspace
        .path()
        .map(std::path::Path::to_path_buf)
        .or_else(|| {
            catalog
                .projects
                .iter()
                .find(|project| project.id == session.project_id)
                .map(|project| project.path.clone())
        })
        .or_else(|| {
            let fallback = catalog.default_cwd.clone();
            (!fallback.as_os_str().is_empty()).then_some(fallback)
        })
}

fn start_runtime(
    client: &DaemonClient,
    session: &AgentSession,
    catalog: &Catalog,
) -> anyhow::Result<AttachedRuntime> {
    let probe = client
        .request(
            Uuid::nil(),
            Uuid::nil(),
            Command::ProbeProvider {
                provider: session.provider,
                binary_override: None,
                discover_models: false,
                probe_version: false,
            },
        )?
        .into_probe()?;
    let ProviderProbe { installed, path, .. } = probe;
    let binary = if installed { path } else { None }
        .ok_or_else(|| anyhow!("远程主机未安装 {}", session.provider.display_name()))?;
    let runtime_id = Uuid::new_v4();
    let response = client.request(
        session.id,
        runtime_id,
        Command::Start {
            options: WireDriverStartOptions {
                provider: encode_enum(session.provider)?,
                binary,
                cwd: session_cwd(session, catalog)
                    .ok_or_else(|| anyhow!("远程会话没有可用的工作目录"))?,
                mode: encode_enum(session.runtime_mode)?,
                model: session.model.clone(),
                reasoning_effort: session.reasoning_effort.clone(),
                service_tier: session.service_tier.clone(),
                context_window: session.context_window.clone(),
                agent_preset: session.agent_preset.clone(),
                // Computer use needs the desktop host's screen; never on the
                // daemon head.
                computer_use_enabled: false,
                provider_cursor: session
                    .provider_cursor
                    .clone()
                    .map(serde_json::to_value)
                    .transpose()?,
            },
        },
    )?;
    let ResponsePayload::Started { supports_steer } = response else {
        bail!("远程主机返回了无效的启动响应");
    };
    Ok(AttachedRuntime {
        runtime_id,
        supports_steer,
    })
}

/// The provider probe lives inside an opaque payload on older responses; the
/// protocol keeps it typed now, so a tiny helper keeps the call sites flat.
trait ProbeResponse {
    fn into_probe(self) -> anyhow::Result<ProviderProbe>;
}

impl ProbeResponse for ResponsePayload {
    fn into_probe(self) -> anyhow::Result<ProviderProbe> {
        match self {
            ResponsePayload::ProviderProbe { probe, .. } => Ok(probe),
            _ => bail!("远程主机返回了无效的探测响应"),
        }
    }
}
