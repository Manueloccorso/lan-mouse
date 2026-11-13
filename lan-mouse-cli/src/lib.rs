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
    #[error("seamless Niri integration is disabled for this edge")]
    SeamlessNiriDisabled,
    #[error("Niri is not available in the current session")]
    NiriUnavailable,
    #[error("seamless Niri error: {0}")]
    SeamlessNiri(String),
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
    /// Niri-specific focus commands
    Niri(NiriCommand),
}

#[derive(Args, Clone, Debug, PartialEq, Eq)]
struct NiriCommand {
    #[command(subcommand)]
    action: NiriAction,
}

#[derive(Clone, Subcommand, Debug, PartialEq, Eq)]
enum NiriAction {
    /// focus left or transfer to the left edge
    #[command(name = "focus_left")]
    Left,
    /// focus right or transfer to the right edge
    #[command(name = "focus_right")]
    Right,
    /// focus up or transfer to the top edge
    #[command(name = "focus_up")]
    Up,
    /// focus down or transfer to the bottom edge
    #[command(name = "focus_down")]
    Down,
}

pub async fn run(
    args: CliArgs,
    left_enabled: bool,
    right_enabled: bool,
    top_enabled: bool,
    bottom_enabled: bool,
) -> Result<(), CliError> {
    execute(
        args.command,
        left_enabled,
        right_enabled,
        top_enabled,
        bottom_enabled,
    )
    .await?;
    Ok(())
}

async fn execute(
    cmd: CliSubcommand,
    left_enabled: bool,
    right_enabled: bool,
    top_enabled: bool,
    bottom_enabled: bool,
) -> Result<(), CliError> {
    if let CliSubcommand::Niri(NiriCommand { action }) = &cmd {
        let (direction, enabled) = match action {
            NiriAction::Left => (Position::Left, left_enabled),
            NiriAction::Right => (Position::Right, right_enabled),
            NiriAction::Up => (Position::Top, top_enabled),
            NiriAction::Down => (Position::Bottom, bottom_enabled),
        };
        if !enabled {
            return Err(CliError::SeamlessNiriDisabled);
        }
        return seamless_niri_focus(direction).await;
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
        CliSubcommand::Niri(_) => unreachable!(),
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

async fn seamless_niri_focus(direction: Position) -> Result<(), CliError> {
    log::info!("seamless Niri focus request: {direction}");
    let focused = niri_request(serde_json::json!("FocusedOutput")).await?;
    let outputs = niri_request(serde_json::json!("Outputs")).await?;
    let focused = focused
        .get("FocusedOutput")
        .cloned()
        .ok_or_else(|| CliError::SeamlessNiri("missing focused output response".to_owned()))?;
    let focused: Option<NiriOutput> =
        serde_json::from_value(focused).map_err(|e| CliError::SeamlessNiri(e.to_string()))?;
    let outputs = outputs
        .get("Outputs")
        .cloned()
        .ok_or_else(|| CliError::SeamlessNiri("missing outputs response".to_owned()))?;
    let outputs: HashMap<String, NiriOutput> =
        serde_json::from_value(outputs).map_err(|e| CliError::SeamlessNiri(e.to_string()))?;
    let Some(current) = focused.as_ref().and_then(|output| output.logical.as_ref()) else {
        return Err(CliError::SeamlessNiri(
            "no focused logical output".to_owned(),
        ));
    };
    let local_neighbor = outputs.values().any(|candidate| {
        candidate
            .logical
            .as_ref()
            .is_some_and(|other| directional_neighbor(current, other, direction))
    });
    if local_neighbor {
        log::info!("local monitor found to the {direction}; using native Niri focus");
        let action = match direction {
            Position::Left => serde_json::json!({"Action": {"FocusMonitorLeft": {}}}),
            Position::Right => serde_json::json!({"Action": {"FocusMonitorRight": {}}}),
            Position::Top => serde_json::json!({"Action": {"FocusMonitorUp": {}}}),
            Position::Bottom => serde_json::json!({"Action": {"FocusMonitorDown": {}}}),
        };
        niri_request(action).await?;
        return Ok(());
    }
    log::info!("no local monitor to the {direction}; injecting uinput edge motion");
    inject_edge_motion(direction).await
}

async fn niri_request(request: serde_json::Value) -> Result<serde_json::Value, CliError> {
    #[cfg(not(unix))]
    {
        let _ = request;
        Err(CliError::NiriUnavailable)
    }
    #[cfg(unix)]
    {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::net::UnixStream;
        let socket = std::env::var("NIRI_SOCKET").map_err(|_| CliError::NiriUnavailable)?;
        let mut stream = UnixStream::connect(socket)
            .await
            .map_err(|e| CliError::SeamlessNiri(e.to_string()))?;
        let mut request =
            serde_json::to_string(&request).map_err(|e| CliError::SeamlessNiri(e.to_string()))?;
        request.push('\n');
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|e| CliError::SeamlessNiri(e.to_string()))?;
        let mut response = String::new();
        BufReader::new(stream)
            .read_line(&mut response)
            .await
            .map_err(|e| CliError::SeamlessNiri(e.to_string()))?;
        match serde_json::from_str::<NiriReply<serde_json::Value>>(&response)
            .map_err(|e| CliError::SeamlessNiri(e.to_string()))?
        {
            NiriReply::Ok { value } => Ok(value),
            NiriReply::Err { value } => Err(CliError::SeamlessNiri(value)),
        }
    }
}

fn directional_neighbor(
    current: &NiriLogicalOutput,
    other: &NiriLogicalOutput,
    direction: Position,
) -> bool {
    let current_right = current.x + current.width as i32;
    let current_bottom = current.y + current.height as i32;
    let other_right = other.x + other.width as i32;
    let other_bottom = other.y + other.height as i32;
    let horizontal_overlap = current.y < other_bottom && other.y < current_bottom;
    let vertical_overlap = current.x < other_right && other.x < current_right;
    match direction {
        Position::Left => other_right <= current.x && horizontal_overlap,
        Position::Right => other.x >= current_right && horizontal_overlap,
        Position::Top => other_bottom <= current.y && vertical_overlap,
        Position::Bottom => other.y >= current_bottom && vertical_overlap,
    }
}

#[cfg(target_os = "linux")]
async fn inject_edge_motion(direction: Position) -> Result<(), CliError> {
    let mut mouse = UinputMouse::new()?;
    log::debug!("waiting for libinput to enumerate temporary uinput mouse");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (x, y) = match direction {
        Position::Left => (-100_000, 0),
        Position::Right => (100_000, 0),
        Position::Top => (0, -100_000),
        Position::Bottom => (0, 100_000),
    };
    mouse.move_relative(x, y)?;
    log::info!("injected virtual mouse motion toward {direction}");
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
async fn inject_edge_motion(_direction: Position) -> Result<(), CliError> {
    Err(CliError::SeamlessNiri(
        "seamless Niri transfer requires Linux uinput".to_owned(),
    ))
}

#[cfg(target_os = "linux")]
struct UinputMouse {
    file: std::fs::File,
}

#[cfg(target_os = "linux")]
impl UinputMouse {
    fn new() -> Result<Self, CliError> {
        use std::{fs::OpenOptions, mem::zeroed, os::fd::AsRawFd};

        const EV_SYN: u16 = 0x00;
        const EV_KEY: u16 = 0x01;
        const EV_REL: u16 = 0x02;
        const REL_X: u16 = 0x00;
        const REL_Y: u16 = 0x01;
        const BTN_LEFT: u16 = 0x110;
        const BTN_RIGHT: u16 = 0x111;
        const BTN_MIDDLE: u16 = 0x112;
        const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
        const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
        const UI_SET_RELBIT: libc::c_ulong = 0x4004_5566;
        const UI_DEV_SETUP: libc::c_ulong = 0x405c_5503;
        const UI_DEV_CREATE: libc::c_ulong = 0x5501;

        #[repr(C)]
        struct InputId {
            bustype: u16,
            vendor: u16,
            product: u16,
            version: u16,
        }
        #[repr(C)]
        struct UinputSetup {
            id: InputId,
            name: [libc::c_char; 80],
            ff_effects_max: u32,
        }

        let file = OpenOptions::new()
            .write(true)
            .open("/dev/uinput")
            .map_err(|e| CliError::SeamlessNiri(format!("cannot open /dev/uinput: {e}")))?;
        let fd = file.as_raw_fd();
        for (request, value) in [
            (UI_SET_EVBIT, EV_SYN),
            (UI_SET_EVBIT, EV_KEY),
            (UI_SET_EVBIT, EV_REL),
        ] {
            if unsafe { libc::ioctl(fd, request, value as libc::c_int) } < 0 {
                return Err(CliError::SeamlessNiri(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
        }
        for button in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE] {
            if unsafe { libc::ioctl(fd, UI_SET_KEYBIT, button as libc::c_int) } < 0 {
                return Err(CliError::SeamlessNiri(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
        }
        for axis in [REL_X, REL_Y] {
            if unsafe { libc::ioctl(fd, UI_SET_RELBIT, axis as libc::c_int) } < 0 {
                return Err(CliError::SeamlessNiri(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
        }
        let mut setup: UinputSetup = unsafe { zeroed() };
        setup.id.bustype = 0x03;
        for (dst, src) in setup.name.iter_mut().zip(b"lan-mouse-seamless\0") {
            *dst = *src as libc::c_char;
        }
        if unsafe { libc::ioctl(fd, UI_DEV_SETUP, &setup) } < 0
            || unsafe { libc::ioctl(fd, UI_DEV_CREATE) } < 0
        {
            return Err(CliError::SeamlessNiri(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        log::debug!("created temporary uinput mouse device");
        Ok(Self { file })
    }

    fn move_relative(&mut self, x: i32, y: i32) -> Result<(), CliError> {
        use std::io::Write;
        #[repr(C)]
        struct InputEvent {
            time: libc::timeval,
            type_: u16,
            code: u16,
            value: i32,
        }
        let events = [
            InputEvent {
                time: libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
                type_: 0x02,
                code: 0,
                value: x,
            },
            InputEvent {
                time: libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
                type_: 0x02,
                code: 1,
                value: y,
            },
            InputEvent {
                time: libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
                type_: 0,
                code: 0,
                value: 0,
            },
        ];
        let bytes = unsafe {
            std::slice::from_raw_parts(
                events.as_ptr().cast::<u8>(),
                events.len() * std::mem::size_of::<InputEvent>(),
            )
        };
        self.file
            .write_all(bytes)
            .map_err(|e| CliError::SeamlessNiri(format!("cannot inject uinput motion: {e}")))
    }
}

#[cfg(target_os = "linux")]
impl Drop for UinputMouse {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        const UI_DEV_DESTROY: libc::c_ulong = 0x5502;
        unsafe {
            libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY);
        }
    }
}
