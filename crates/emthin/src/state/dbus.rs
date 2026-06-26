//! DBus bridge — manages the `emthin-dbus-router` subprocess and relays
//! IPC messages (routing rules, fcitx events) between the main process
//! and the router.
//!
//! Every field is optional: if the router binary is missing or the host
//! has no session bus, the bridge stays inert and the compositor keeps
//! running.

use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use emthin_dbus::router::{RouterNotification, RouterRequest};
use emthin_dbus::FcitxEvent;

/// The bridge is "live" iff `router_ipc.is_some()`.
#[derive(Default)]
pub struct DbusBridge {
    /// Router subprocess (emthin-dbus-router).
    router_child: Option<Child>,
    /// IPC connection to the router (Content-Length framed JSON-RPC).
    router_ipc: Option<UnixStream>,
    /// Bus socket path injected into children via DBUS_SESSION_BUS_ADDRESS.
    listen_path: Option<PathBuf>,
    /// Runtime session dir owned by us; cleaned up on shutdown.
    session_dir: Option<PathBuf>,
    /// Private dbus-daemon child when --dbus-isolated is in effect.
    isolated_daemon: Option<Child>,
    /// Buffer for partial IPC frame reads.
    read_buf: Vec<u8>,
    /// Non-fcitx notifications from router (RuleAdded, RuleRemoved, RuleList).
    pending_notifications: Vec<RouterNotification>,
}

impl std::fmt::Debug for DbusBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbusBridge")
            .field("router", &self.router_child.is_some())
            .field("listen_path", &self.listen_path)
            .field("session_dir", &self.session_dir)
            .field("isolated_daemon", &self.isolated_daemon.is_some())
            .finish()
    }
}

impl DbusBridge {
    /// Resolve upstream address, create session dir, spawn router.
    fn create(
        listen_path: PathBuf,
        ipc_path: PathBuf,
        session_dir: PathBuf,
        upstream_path: PathBuf,
    ) -> Self {
        let mut cmd = Command::new("emthin-dbus-router");
        cmd.arg("--listen")
            .arg(&listen_path)
            .arg("--ipc")
            .arg(&ipc_path)
            .arg("--upstream")
            .arg(&upstream_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let mut router_child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "failed to spawn emthin-dbus-router; bridge inert");
                std::fs::remove_dir_all(&session_dir).ok();
                return Self::default();
            }
        };

        // Wait for IPC socket to appear
        if let Err(e) = wait_for_socket(&ipc_path, &mut router_child) {
            tracing::warn!(error = %e, "router IPC socket never appeared; bridge inert");
            kill_child(router_child);
            let _ = std::fs::remove_dir_all(&session_dir);
            return Self::default();
        }

        // Connect to router IPC
        let router_ipc = match UnixStream::connect(&ipc_path) {
            Ok(s) => {
                let _ = s.set_nonblocking(true);
                Some(s)
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to connect to router IPC; bridge inert");
                kill_child(router_child);
                let _ = std::fs::remove_dir_all(&session_dir);
                return Self::default();
            }
        };

        tracing::info!(
            ?listen_path,
            ?session_dir,
            router_pid = router_child.id(),
            "dbus router spawned; bus injected into children"
        );

        Self {
            router_child: Some(router_child),
            router_ipc,
            listen_path: Some(listen_path),
            session_dir: Some(session_dir),
            isolated_daemon: None,
            read_buf: Vec::new(),
            pending_notifications: Vec::new(),
        }
    }

    /// Initialize bridge for the host session bus.
    pub fn init() -> Self {
        let Ok(upstream_addr) = std::env::var("DBUS_SESSION_BUS_ADDRESS") else {
            tracing::info!("DBUS_SESSION_BUS_ADDRESS not set; dbus bridge inert");
            return Self::default();
        };
        let upstream_path = match parse_bus_address(&upstream_addr) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, addr = %upstream_addr, "unsupported DBUS_SESSION_BUS_ADDRESS; bridge inert");
                return Self::default();
            }
        };

        let Some(session_dir) = create_session_dir() else {
            return Self::default();
        };

        let listen_path = session_dir.join("bus.sock");
        let ipc_path = session_dir.join("router-ipc.sock");
        Self::create(listen_path, ipc_path, session_dir, upstream_path)
    }

    /// Initialize with an isolated dbus-daemon as upstream.
    pub fn init_isolated() -> Self {
        let Some(session_dir) = create_session_dir() else {
            return Self::default();
        };

        let daemon_socket = session_dir.join("upstream-bus.sock");
        let services_dir = session_dir.join("services");
        let config_path = session_dir.join("session.conf");
        if let Err(e) = write_minimal_session_config(&config_path, &services_dir, &daemon_socket) {
            tracing::warn!(error = %e, "failed to write session.conf; bridge inert");
            let _ = std::fs::remove_dir_all(&session_dir);
            return Self::default();
        }

        let mut daemon = match spawn_isolated_daemon(&config_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "failed to spawn dbus-daemon; bridge inert");
                let _ = std::fs::remove_dir_all(&session_dir);
                return Self::default();
            }
        };

        if let Err(e) = wait_for_daemon_socket(&daemon_socket, &mut daemon) {
            tracing::warn!(error = %e, "dbus-daemon never came up; bridge inert");
            let _ = daemon.kill();
            let _ = daemon.wait();
            let _ = std::fs::remove_dir_all(&session_dir);
            return Self::default();
        }

        let listen_path = session_dir.join("bus.sock");
        let ipc_path = session_dir.join("router-ipc.sock");
        let mut bridge = Self::create(listen_path, ipc_path, session_dir, daemon_socket);
        bridge.isolated_daemon = Some(daemon);
        bridge
    }

    /// Inject DBUS_SESSION_BUS_ADDRESS into cmd if bridge is live.
    pub fn inject_env(&self, cmd: &mut Command) {
        if let Some(path) = &self.listen_path {
            cmd.env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}", path.display()),
            );
        }
    }

    /// Send a RouterRequest to the router subprocess.
    pub fn send_rpc(&mut self, msg: &RouterRequest) {
        let Some(ref mut ipc) = self.router_ipc else {
            return;
        };
        let data = match serde_json::to_string(msg) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "send_rpc: serialize failed");
                return;
            }
        };
        let header = format!("Content-Length: {}\r\n\r\n", data.len());
        if let Err(e) = ipc
            .write_all(header.as_bytes())
            .and_then(|_| ipc.write_all(data.as_bytes()))
        {
            tracing::warn!(error = %e, "send_rpc: write failed");
        }
    }

    /// Drain available FcitxEvent notifications from the router IPC socket.
    pub fn take_fcitx_events(&mut self) -> Vec<FcitxEvent> {
        let Some(ref mut ipc) = self.router_ipc else {
            return vec![];
        };

        // Read available bytes
        let mut tmp = [0u8; 16384];
        loop {
            match ipc.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => self.read_buf.extend_from_slice(&tmp[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => {
                    tracing::warn!(error = %e, "router IPC read error");
                    break;
                }
            }
        }

        if self.read_buf.is_empty() {
            return vec![];
        }

        // Parse complete Content-Length framed notifications
        let mut events = Vec::new();
        loop {
            let header_end = self.read_buf.windows(4).position(|w| w == b"\r\n\r\n");
            let Some(end) = header_end else {
                break;
            };

            let header = std::str::from_utf8(&self.read_buf[..end]).unwrap_or("");
            let len = header
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length:")
                        .and_then(|s| s.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);

            if len == 0 || self.read_buf.len() < end + 4 + len {
                break;
            }

            let body: Vec<u8> = self.read_buf.drain(..end + 4 + len).collect();
            let notification: RouterNotification = match serde_json::from_slice(&body[end + 4..]) {
                Ok(n) => n,
                Err(e) => {
                    tracing::warn!(error = %e, "router IPC: parse notification failed");
                    continue;
                }
            };

            if let RouterNotification::FcitxEvent(event) = notification {
                events.push(event);
            } else {
                self.pending_notifications.push(notification);
            }
        }

        events
    }

    /// Drain pending non-fcitx notifications from the router.
    pub fn take_router_notifications(&mut self) -> Vec<RouterNotification> {
        std::mem::take(&mut self.pending_notifications)
    }

    /// Drop the router, daemon, and session dir.
    pub fn shutdown(&mut self) {
        self.router_ipc = None;
        if let Some(child) = self.router_child.take() {
            kill_child(child);
        }
        if let Some(mut daemon) = self.isolated_daemon.take() {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        if let Some(dir) = self.session_dir.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_bus_address(addr: &str) -> io::Result<PathBuf> {
    const PREFIX: &str = "unix:path=";
    let stripped = addr.strip_prefix(PREFIX).ok_or_else(|| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("unsupported bus address: {addr}"),
        )
    })?;
    let path = stripped.split(',').next().unwrap_or(stripped);
    Ok(PathBuf::from(path))
}

fn create_session_dir() -> Option<PathBuf> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let dir = runtime_dir.join(format!("emthin-dbus-{}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(error = %e, ?dir, "failed to create dbus session dir");
        return None;
    }
    Some(dir)
}

fn kill_child(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn wait_for_socket(socket: &Path, child: &mut Child) -> io::Result<()> {
    const TIMEOUT: Duration = Duration::from_secs(3);
    const POLL_INTERVAL: Duration = Duration::from_millis(25);
    let start = Instant::now();
    loop {
        if socket.exists() {
            return Ok(());
        }
        // Check if child exited (try_wait returns io::Result<Option<ExitStatus>>)
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "router exited before binding socket: {status}"
            )));
        }
        if start.elapsed() > TIMEOUT {
            return Err(io::Error::new(
                ErrorKind::TimedOut,
                "router IPC socket did not appear within 3s",
            ));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn spawn_isolated_daemon(config_path: &Path) -> std::io::Result<Child> {
    let mut cmd = Command::new("dbus-daemon");
    cmd.arg("--nofork")
        .arg(format!("--config-file={}", config_path.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn()
}

fn wait_for_daemon_socket(socket: &Path, daemon: &mut Child) -> std::io::Result<()> {
    const TIMEOUT: Duration = Duration::from_secs(3);
    const POLL_INTERVAL: Duration = Duration::from_millis(25);
    let start = Instant::now();
    loop {
        if socket.exists() {
            return Ok(());
        }
        if let Some(status) = daemon.try_wait()? {
            return Err(std::io::Error::other(format!(
                "dbus-daemon exited before binding socket: {status}"
            )));
        }
        if start.elapsed() > TIMEOUT {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "dbus-daemon listen socket did not appear within 3s",
            ));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn write_minimal_session_config(
    path: &Path,
    services_dir: &Path,
    socket: &Path,
) -> std::io::Result<()> {
    std::fs::create_dir_all(services_dir)?;
    let xml = format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={socket}</listen>
  <auth>EXTERNAL</auth>
  <servicedir>{services}</servicedir>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
  <limit name="reply_timeout">300000</limit>
</busconfig>
"#,
        socket = socket.display(),
        services = services_dir.display(),
    );
    std::fs::write(path, xml)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_env_is_noop_when_inert() {
        let bridge = DbusBridge::default();
        let mut cmd = Command::new("true");
        bridge.inject_env(&mut cmd);
        let dbg = format!("{cmd:?}");
        assert!(
            !dbg.contains("DBUS_SESSION_BUS_ADDRESS"),
            "inert bridge must not set env; got: {dbg}"
        );
    }

    #[test]
    fn inject_env_sets_unix_path_when_live() {
        let bridge = DbusBridge {
            listen_path: Some(PathBuf::from("/run/user/1000/emthin-dbus-42/bus.sock")),
            ..DbusBridge::default()
        };
        let mut cmd = Command::new("true");
        bridge.inject_env(&mut cmd);
        let dbg = format!("{cmd:?}");
        assert!(dbg.contains("unix:path=/run/user/1000/emthin-dbus-42/bus.sock"));
    }

    #[test]
    fn parse_bus_address_works() {
        assert_eq!(
            parse_bus_address("unix:path=/run/user/1000/bus").unwrap(),
            PathBuf::from("/run/user/1000/bus")
        );
    }
}
