//! Direct Hyprland request socket and the window/monitor data used by the panel.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, de::DeserializeOwned};
use std::{
    env,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::{Component, Path, PathBuf},
    time::Duration,
};

const TIMEOUT: Duration = Duration::from_millis(180);
const MAX_REPLY: u64 = 8 * 1024 * 1024;

pub fn socket_path(events: bool) -> Result<PathBuf> {
    let runtime =
        PathBuf::from(env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is unset")?);
    ensure!(runtime.is_absolute(), "XDG_RUNTIME_DIR is not absolute");
    let instance = env::var_os("HYPRLAND_INSTANCE_SIGNATURE")
        .context("HYPRLAND_INSTANCE_SIGNATURE is unset")?;
    let mut parts = Path::new(&instance).components();
    ensure!(
        matches!(parts.next(), Some(Component::Normal(name)) if name == instance.as_os_str())
            && parts.next().is_none(),
        "invalid Hyprland instance signature"
    );
    Ok(runtime.join("hypr").join(instance).join(if events {
        ".socket2.sock"
    } else {
        ".socket.sock"
    }))
}

fn request_at(path: &Path, command: &str) -> Result<String> {
    let mut stream = UnixStream::connect(path).context("connect to Hyprland")?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream
        .write_all(command.as_bytes())
        .context("send Hyprland request")?;
    let mut reply = String::new();
    stream
        .take(MAX_REPLY + 1)
        .read_to_string(&mut reply)
        .context("read Hyprland response")?;
    ensure!(
        reply.len() as u64 <= MAX_REPLY,
        "oversized Hyprland response"
    );
    Ok(reply)
}

fn request(command: &str) -> Result<String> {
    request_at(&socket_path(false)?, command)
}

fn query<T: DeserializeOwned>(command: &str) -> Result<T> {
    serde_json::from_str(&request(command)?)
        .with_context(|| format!("decode Hyprland {command} response"))
}

#[derive(Clone, Debug)]
struct Address(String);

impl Address {
    fn parse(value: &str) -> Option<Self> {
        let hex = value.strip_prefix("0x")?;
        if hex.is_empty()
            || !hex.bytes().all(|b| b.is_ascii_hexdigit())
            || hex.bytes().all(|b| b == b'0')
        {
            return None;
        }
        Some(Self(value.to_owned()))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Target {
    address: Address,
    stable_id: String,
}

impl Target {
    fn matches(&self, client: &Client) -> bool {
        self.address.0 == client.address
            && (self.stable_id.is_empty() || self.stable_id == client.stable_id)
    }

    fn selector(&self) -> String {
        // Prefer a stable ID so recycling the address between the read and
        // dispatch cannot select a replacement window.
        if !self.stable_id.is_empty() && self.stable_id.bytes().all(|b| b.is_ascii_hexdigit()) {
            format!("stableid:{}", self.stable_id)
        } else {
            format!("address:{}", self.address.0)
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Client {
    address: String,
    #[serde(rename = "stableId")]
    stable_id: String,
    pub title: String,
    pub floating: bool,
    pub hidden: bool,
    mapped: Option<bool>,
    visible: Option<bool>,
    #[serde(deserialize_with = "fullscreen")]
    pub fullscreen: bool,
    pub at: (i32, i32),
    pub size: (i32, i32),
    pub monitor: Option<i64>,
    workspace: Workspace,
}

fn fullscreen<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Fullscreen {
        Flag(bool),
        Mode(i32),
    }
    Ok(match Fullscreen::deserialize(deserializer)? {
        Fullscreen::Flag(value) => value,
        Fullscreen::Mode(value) => value != 0,
    })
}

impl Client {
    pub fn target(&self) -> Option<Target> {
        Some(Target {
            address: Address::parse(&self.address)?,
            stable_id: self.stable_id.clone(),
        })
    }

    pub fn adjustable(&self) -> bool {
        !self.fullscreen && !self.hidden && self.mapped != Some(false)
    }

    pub fn visible_on(&self, monitor: &Monitor) -> bool {
        self.visible != Some(false)
            && self.monitor == Some(monitor.id)
            && (self.workspace.matches(&monitor.active_workspace)
                || self.workspace.matches(&monitor.special_workspace))
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Workspace {
    address: String,
    id: Option<i64>,
}

impl Workspace {
    fn matches(&self, workspace: &Self) -> bool {
        if !self.address.is_empty() && !workspace.address.is_empty() {
            self.address == workspace.address
        } else {
            self.id
                .is_some_and(|id| id != -1 && Some(id) == workspace.id)
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Monitor {
    pub id: i64,
    name: String,
    width: i32,
    height: i32,
    x: i32,
    y: i32,
    reserved: [i32; 4],
    scale: f64,
    transform: i32,
    #[serde(default)]
    focused: bool,
    #[serde(default, rename = "activeWorkspace")]
    active_workspace: Workspace,
    #[serde(default, rename = "specialWorkspace")]
    special_workspace: Workspace,
}

impl Monitor {
    fn logical_size(&self) -> Option<(i32, i32)> {
        if !self.scale.is_finite() || self.scale <= 0.0 || !(0..=7).contains(&self.transform) {
            return None;
        }
        let scaled = |extent: i32| {
            let value = (f64::from(extent) / self.scale).round_ties_even();
            (value >= 1.0 && value <= f64::from(i32::MAX)).then_some(value as i32)
        };
        let (width, height) = if self.transform % 2 == 0 {
            (self.width, self.height)
        } else {
            (self.height, self.width)
        };
        Some((scaled(width)?, scaled(height)?))
    }

    pub fn work_area(&self) -> Option<(i32, i32, i32, i32)> {
        let (width, height) = self.logical_size()?;
        let [left, top, right, bottom] = self.reserved;
        if self.reserved.iter().any(|margin| *margin < 0) {
            return None;
        }
        let x1 = self.x.checked_add(left)?;
        let y1 = self.y.checked_add(top)?;
        let x2 = self.x.checked_add(width.checked_sub(right)?)?;
        let y2 = self.y.checked_add(height.checked_sub(bottom)?)?;
        (x1 < x2 && y1 < y2).then_some((x1, y1, x2, y2))
    }
}

pub fn monitor_size() -> Option<(i32, i32)> {
    let monitors = monitors().ok()?;
    let configured = crate::shell::output();
    configured
        .as_ref()
        .and_then(|name| monitors.iter().find(|m| &m.name == name))
        .or_else(|| monitors.iter().find(|m| m.focused))
        .or_else(|| monitors.first())?
        .logical_size()
}

pub(crate) fn active_client() -> Result<Option<(Target, Client)>> {
    let client: Client = query("j/activewindow")?;
    Ok(client.target().map(|target| (target, client)))
}

pub(crate) fn client(target: &Target) -> Result<Option<Client>> {
    let clients: Vec<Client> = query("j/clients")?;
    Ok(clients.into_iter().find(|client| target.matches(client)))
}

pub(crate) fn monitors() -> Result<Vec<Monitor>> {
    query("j/monitors")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Geometry {
    Move,
    Resize,
}

fn dispatch(
    target: &Target,
    action: &str,
    args: &str,
    geometry_guard: Option<(i64, bool)>,
) -> Result<()> {
    // A bare dispatcher selector can fall back to the focused window when
    // it no longer resolves. Resolve and validate it in the compositor in
    // the same request, then pass the actual window object to the dispatcher.
    let selector = target.selector();
    let geometry_guard = geometry_guard.map(|(monitor, require_visible)| {
        let visible = if require_visible { " and window.visible" } else { "" };
        format!("assert(window.floating and window.monitor and window.monitor.id == {monitor}{visible}, 'window is no longer adjustable on this monitor'); ")
    }).unwrap_or_default();
    let command = format!(
        "/eval local window = assert(hl.get_window(\"{selector}\"), 'window closed'); \
         assert(window.mapped and not window.hidden and window.fullscreen == 0, 'window is not adjustable'); \
         {geometry_guard}local result = hl.dispatch(hl.dsp.window.{action}({{window=window, {args}}})); \
         assert(result.ok, result.error or 'dispatcher failed')"
    );
    let reply = request(&command)?;
    if reply.trim() == "ok" {
        Ok(())
    } else {
        bail!(
            "Hyprland rejected dispatch: {}",
            reply.trim().chars().take(256).collect::<String>()
        )
    }
}

pub(crate) fn set_floating(target: &Target, floating: bool) -> Result<()> {
    let action = if floating { "set" } else { "unset" };
    dispatch(target, "float", &format!("action=\"{action}\""), None)
}

pub(crate) fn set_geometry(
    target: &Target,
    mode: Geometry,
    (x, y): (i32, i32),
    monitor: i64,
    require_visible: bool,
) -> Result<()> {
    let action = match mode {
        Geometry::Move => "move",
        Geometry::Resize => "resize",
    };
    dispatch(
        target,
        action,
        &format!("x={x}, y={y}, relative=false"),
        Some((monitor, require_visible)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn socket_request_reads_complete_reply_without_waiting_for_request_eof() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hypr.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; b"j/monitors".len()];
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            stream.read_exact(&mut request).unwrap();
            assert_eq!(&request, b"j/monitors");
            stream.write_all(b"[{").unwrap();
            stream.write_all(b"\"id\":0}]").unwrap();
        });
        assert_eq!(request_at(&path, "j/monitors").unwrap(), r#"[{"id":0}]"#);
        server.join().unwrap();
    }

    #[test]
    fn addresses_are_non_null_exact_selectors() {
        assert!(Address::parse("0xaBc012").is_some());
        for address in ["", "0x", "0x000", "0X42", "0x42\n", "0x42\"", "42"] {
            assert!(Address::parse(address).is_none(), "accepted {address:?}");
        }
    }

    #[test]
    fn monitors_rotate_then_scale_and_remove_reserved_space() {
        let monitor: Monitor = serde_json::from_str(
            r#"{
            "id":0,"name":"DSI-1","width":1904,"height":3040,"x":-1140,"y":10,
            "reserved":[5,34,7,42],"scale":2.6666667,"transform":1
        }"#,
        )
        .unwrap();
        assert_eq!(monitor.logical_size(), Some((1140, 714)));
        assert_eq!(monitor.work_area(), Some((-1135, 44, -7, 682)));
    }

    #[test]
    fn exact_target_cannot_be_replaced_at_the_same_address() {
        let original: Client =
            serde_json::from_str(r#"{"address":"0x42","stableId":"one"}"#).unwrap();
        let replacement: Client =
            serde_json::from_str(r#"{"address":"0x42","stableId":"two"}"#).unwrap();
        assert!(!original.target().unwrap().matches(&replacement));
    }

    #[test]
    fn dispatch_uses_stable_id_or_the_validated_address() {
        let client = |json| serde_json::from_str::<Client>(json).unwrap();
        let modern = client(r#"{"address":"0x42","stableId":"1800000a"}"#);
        let older = client(r#"{"address":"0x42"}"#);
        assert_eq!(modern.target().unwrap().selector(), "stableid:1800000a");
        assert_eq!(older.target().unwrap().selector(), "address:0x42");
    }
}
