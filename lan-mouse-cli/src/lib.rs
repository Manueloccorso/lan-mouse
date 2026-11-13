use clap::{Args, Parser, Subcommand};
use futures::StreamExt;

use serde::Deserialize;
use std::{collections::HashMap, net::IpAddr, time::Duration};
use thiserror::Error;

use lan_mouse_ipc::{
    ClientHandle, ConnectionError, FrontendEvent, FrontendRequest, IpcError, Position,
    connect_async,
};

#[derive(Debug, Error)]
pub enum CliError {
    /// is the service running?
    #[error("could not connect: `{0}` - is the service running?")]
    ServiceNotRunning(#[from] ConnectionError),
    #[error("error communicating with service: {0}")]
    Ipc(#[from] IpcError),
    #[error("Niri integration is disabled in the Lan Mouse configuration")]
    NiriDisabled,
    #[error("Niri is not available in the current session")]
    NiriUnavailable,
    #[error("Niri IPC error: {0}")]
    Niri(String),
}

#[derive(Parser, Clone, Debug, PartialEq, Eq)]
#[command(name = "lan-mouse-cli", about = "LanMouse CLI interface")]
pub struct CliArgs {
    #[command(subcommand)]
    command: CliSubcommand,
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
struct Client {
    #[arg(long)]
    hostname: Option<String>,
    #[arg(long)]
    port: Option<u16>,
    #[arg(long)]
    ips: Option<Vec<IpAddr>>,
    #[arg(long)]
    enter_hook: Option<String>,
    #[arg(long)]
    leave_hook: Option<String>,
}

#[derive(Clone, Subcommand, Debug, PartialEq, Eq)]
enum CliSubcommand {
    /// add a new client
    AddClient(Client),
    /// remove an existing client
    RemoveClient { id: ClientHandle },
    /// activate a client
    Activate { id: ClientHandle },
    /// deactivate a client
    Deactivate { id: ClientHandle },
    /// list configured clients
    List,
    /// change hostname
    SetHost {
        id: ClientHandle,
        host: Option<String>,
    },
    /// change port
    SetPort { id: ClientHandle, port: u16 },
    /// set position
    SetPosition { id: ClientHandle, pos: Position },
    /// set ips
    SetIps { id: ClientHandle, ips: Vec<IpAddr> },
    /// re-enable capture
    EnableCapture,
    /// re-enable emulation
    EnableEmulation,
    /// authorize a public key
    AuthorizeKey {
        description: String,
        sha256_fingerprint: String,
    },
    /// deauthorize a public key
    RemoveAuthorizedKey { sha256_fingerprint: String },
    /// save configuration to file
    SaveConfig,
    /// navigate focus through local Niri monitors and Lan Mouse clients
    Niri(NiriCommand),
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
struct NiriCommand {
    #[command(subcommand)]
    action: NiriAction,
}

#[derive(Clone, Subcommand, Debug, PartialEq, Eq)]
enum NiriAction {
    /// focus in a direction
    Focus { direction: Position },
}

pub async fn run(args: CliArgs, niri_enabled: bool) -> Result<(), CliError> {
    execute(args.command, niri_enabled).await?;
    Ok(())
}

async fn execute(cmd: CliSubcommand, niri_enabled: bool) -> Result<(), CliError> {
    if let CliSubcommand::Niri(NiriCommand { action }) = &cmd {
        if !niri_enabled {
            return Err(CliError::NiriDisabled);
        }
        match action {
            NiriAction::Focus { direction } => return niri_focus(*direction).await,
        }
    }
    let (mut rx, mut tx) = connect_async(Some(Duration::from_millis(500))).await?;
    match cmd {
        CliSubcommand::AddClient(Client {
            hostname,
            port,
            ips,
            enter_hook,
            leave_hook,
        }) => {
            tx.request(FrontendRequest::Create).await?;
            while let Some(e) = rx.next().await {
                if let FrontendEvent::Created(handle, _, _) = e? {
                    if let Some(hostname) = hostname {
                        tx.request(FrontendRequest::UpdateHostname(handle, Some(hostname)))
                            .await?;
                    }
                    if let Some(port) = port {
                        tx.request(FrontendRequest::UpdatePort(handle, port))
                            .await?;
                    }
                    if let Some(ips) = ips {
                        tx.request(FrontendRequest::UpdateFixIps(handle, ips))
                            .await?;
                    }
                    if let Some(enter_hook) = enter_hook {
                        tx.request(FrontendRequest::UpdateEnterHook(handle, Some(enter_hook)))
                            .await?;
                    }
                    if let Some(leave_hook) = leave_hook {
                        tx.request(FrontendRequest::UpdateLeaveHook(handle, Some(leave_hook)))
                            .await?;
                    }
                    break;
                }
            }
        }
        CliSubcommand::RemoveClient { id } => tx.request(FrontendRequest::Delete(id)).await?,
        CliSubcommand::Activate { id } => tx.request(FrontendRequest::Activate(id, true)).await?,
        CliSubcommand::Deactivate { id } => {
            tx.request(FrontendRequest::Activate(id, false)).await?
        }
        CliSubcommand::List => {
            tx.request(FrontendRequest::Enumerate()).await?;
            while let Some(e) = rx.next().await {
                if let FrontendEvent::Enumerate(clients) = e? {
                    for (handle, config, state) in clients {
                        let host = config.hostname.unwrap_or("unknown".to_owned());
                        let port = config.port;
                        let pos = config.pos;
                        let active = state.active;
                        let ips = state.ips;
                        println!(
                            "id {handle}: {host}:{port} ({pos}) active: {active}, ips: {ips:?}"
                        );
                    }
                    break;
                }
            }
        }
        CliSubcommand::SetHost { id, host } => {
            tx.request(FrontendRequest::UpdateHostname(id, host))
                .await?
        }
        CliSubcommand::SetPort { id, port } => {
            tx.request(FrontendRequest::UpdatePort(id, port)).await?
        }
        CliSubcommand::SetPosition { id, pos } => {
            tx.request(FrontendRequest::UpdatePosition(id, pos)).await?
        }
        CliSubcommand::SetIps { id, ips } => {
            tx.request(FrontendRequest::UpdateFixIps(id, ips)).await?
        }
        CliSubcommand::EnableCapture => tx.request(FrontendRequest::EnableCapture).await?,
        CliSubcommand::EnableEmulation => tx.request(FrontendRequest::EnableEmulation).await?,
        CliSubcommand::AuthorizeKey {
            description,
            sha256_fingerprint,
        } => {
            tx.request(FrontendRequest::AuthorizeKey(
                description,
                sha256_fingerprint,
            ))
            .await?
        }
        CliSubcommand::RemoveAuthorizedKey { sha256_fingerprint } => {
            tx.request(FrontendRequest::RemoveAuthorizedKey(sha256_fingerprint))
                .await?
        }
        CliSubcommand::SaveConfig => tx.request(FrontendRequest::SaveConfiguration).await?,
        CliSubcommand::Niri(_) => {
            unreachable!("Niri commands are handled before frontend connection")
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct NiriLogicalOutput {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Debug, Deserialize)]
struct NiriOutput {
    name: String,
    logical: Option<NiriLogicalOutput>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum NiriReply<T> {
    Ok {
        #[serde(rename = "Ok")]
        value: T,
    },
    Err {
        #[serde(rename = "Err")]
        value: String,
    },
}

async fn niri_request(request: serde_json::Value) -> Result<serde_json::Value, CliError> {
    #[cfg(not(unix))]
    {
        let _ = request;
        return Err(CliError::NiriUnavailable);
    }
    #[cfg(unix)]
    {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::UnixStream;

        let socket = std::env::var("NIRI_SOCKET").map_err(|_| CliError::NiriUnavailable)?;
        let mut stream = UnixStream::connect(socket)
            .await
            .map_err(|e| CliError::Niri(e.to_string()))?;
        let mut line =
            serde_json::to_string(&request).map_err(|e| CliError::Niri(e.to_string()))?;
        line.push('\n');
        stream
            .write_all(line.as_bytes())
            .await
            .map_err(|e| CliError::Niri(e.to_string()))?;
        let mut response = String::new();
        BufReader::new(stream)
            .read_line(&mut response)
            .await
            .map_err(|e| CliError::Niri(e.to_string()))?;
        let response: NiriReply<serde_json::Value> =
            serde_json::from_str(&response).map_err(|e| CliError::Niri(e.to_string()))?;
        match response {
            NiriReply::Ok { value } => Ok(value),
            NiriReply::Err { value } => Err(CliError::Niri(value)),
        }
    }
}

async fn niri_focus(direction: Position) -> Result<(), CliError> {
    let focused = niri_request(serde_json::json!("FocusedOutput")).await?;
    let outputs = niri_request(serde_json::json!("Outputs")).await?;
    let focused = focused
        .get("FocusedOutput")
        .cloned()
        .ok_or_else(|| CliError::Niri("missing FocusedOutput response".to_owned()))?;
    let focused: Option<NiriOutput> =
        serde_json::from_value(focused).map_err(|e| CliError::Niri(e.to_string()))?;
    let outputs = outputs
        .get("Outputs")
        .cloned()
        .ok_or_else(|| CliError::Niri("missing Outputs response".to_owned()))?;
    let outputs: HashMap<String, NiriOutput> =
        serde_json::from_value(outputs).map_err(|e| CliError::Niri(e.to_string()))?;
    let has_neighbor = focused
        .as_ref()
        .and_then(|output| {
            output
                .logical
                .as_ref()
                .map(|logical| (&output.name, logical))
        })
        .is_some_and(|(name, current)| {
            outputs.values().any(|candidate| {
                candidate.name != *name
                    && candidate
                        .logical
                        .as_ref()
                        .is_some_and(|other| directional_neighbor(current, other, direction))
            })
        });

    if has_neighbor {
        let action = match direction {
            Position::Left => serde_json::json!({"Action": {"FocusMonitorLeft": {}}}),
            Position::Right => serde_json::json!({"Action": {"FocusMonitorRight": {}}}),
            Position::Top => serde_json::json!({"Action": {"FocusMonitorUp": {}}}),
            Position::Bottom => serde_json::json!({"Action": {"FocusMonitorDown": {}}}),
        };
        niri_request(action).await?;
    } else {
        let (mut rx, mut tx) = connect_async(Some(Duration::from_millis(500))).await?;
        tx.request(FrontendRequest::TransferToPosition(direction))
            .await?;
        while let Some(event) = rx.next().await {
            match event? {
                FrontendEvent::TransferStarted(pos) if pos == direction => return Ok(()),
                FrontendEvent::Error(error) => return Err(CliError::Niri(error)),
                _ => {}
            }
        }
        return Err(CliError::Niri("Lan Mouse service disconnected".to_owned()));
    }
    Ok(())
}

fn directional_neighbor(
    current: &NiriLogicalOutput,
    other: &NiriLogicalOutput,
    direction: Position,
) -> bool {
    let cx2 = current.x + current.width as i32;
    let cy2 = current.y + current.height as i32;
    let ox2 = other.x + other.width as i32;
    let oy2 = other.y + other.height as i32;
    let horizontal_overlap = current.y < oy2 && other.y < cy2;
    let vertical_overlap = current.x < ox2 && other.x < cx2;
    match direction {
        Position::Left => other.x + other.width as i32 == current.x && horizontal_overlap,
        Position::Right => other.x == cx2 && horizontal_overlap,
        Position::Top => other.y + other.height as i32 == current.y && vertical_overlap,
        Position::Bottom => other.y == cy2 && vertical_overlap,
    }
}

#[cfg(test)]
mod tests {
    use super::{NiriLogicalOutput, directional_neighbor};
    use lan_mouse_ipc::Position;

    fn output(x: i32, y: i32, width: u32, height: u32) -> NiriLogicalOutput {
        NiriLogicalOutput {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn detects_touching_outputs_in_each_direction() {
        let current = output(100, 100, 1000, 800);
        assert!(directional_neighbor(
            &current,
            &output(-900, 100, 1000, 800),
            Position::Left
        ));
        assert!(directional_neighbor(
            &current,
            &output(1100, 100, 1000, 800),
            Position::Right
        ));
        assert!(directional_neighbor(
            &current,
            &output(100, -700, 1000, 800),
            Position::Top
        ));
        assert!(directional_neighbor(
            &current,
            &output(100, 900, 1000, 800),
            Position::Bottom
        ));
    }

    #[test]
    fn rejects_gaps_and_corner_only_contacts() {
        let current = output(0, 0, 100, 100);
        assert!(!directional_neighbor(
            &current,
            &output(101, 0, 100, 100),
            Position::Right
        ));
        assert!(!directional_neighbor(
            &current,
            &output(100, 100, 100, 100),
            Position::Right
        ));
    }

    #[test]
    fn accepts_overlapping_unequal_outputs() {
        let current = output(0, 100, 100, 100);
        assert!(directional_neighbor(
            &current,
            &output(100, 0, 200, 150),
            Position::Right
        ));
        assert!(directional_neighbor(
            &current,
            &output(-200, 150, 200, 200),
            Position::Left
        ));
    }
}
