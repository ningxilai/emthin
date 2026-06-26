use std::collections::{HashMap, VecDeque};
use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::Parser;

use emthin_dbus::router::{RouterNotification, RouterRequest, RoutingTable};
use emthin_dbus::{
    build_preedit_chunks, build_reply, classify, fcitx, method_call_to_event, Fcitx5MethodCall,
    InputContextAllocator,
};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "emthin-dbus-router")]
struct Args {
    #[arg(long)]
    listen: PathBuf,
    #[arg(long)]
    ipc: PathBuf,
    #[arg(long)]
    upstream: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

struct RouterState {
    routing_table: Mutex<RoutingTable>,
    ic_registry: Mutex<HashMap<String, RawFd>>,
    event_queue: Mutex<VecDeque<RouterNotification>>,
    fcitx_server_name: Mutex<Option<String>>,
}

impl RouterState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            routing_table: Mutex::new(RoutingTable::default()),
            ic_registry: Mutex::new(HashMap::new()),
            event_queue: Mutex::new(VecDeque::new()),
            fcitx_server_name: Mutex::new(None),
        })
    }

    fn push_event(&self, notification: RouterNotification) {
        self.event_queue.lock().unwrap().push_back(notification);
    }

    fn drain_events(&self) -> Vec<RouterNotification> {
        let mut queue = self.event_queue.lock().unwrap();
        queue.drain(..).collect()
    }

    fn register_ic(&self, ic_path: &str, client_fd: RawFd) {
        self.ic_registry
            .lock()
            .unwrap()
            .insert(ic_path.to_string(), client_fd);
    }

    fn lookup_ic(&self, ic_path: &str) -> Option<RawFd> {
        self.ic_registry.lock().unwrap().get(ic_path).copied()
    }
}

// ---------------------------------------------------------------------------
// SCM_RIGHTS helpers
// ---------------------------------------------------------------------------

const MAX_FDS: usize = 16;

fn recvmsg_fds(fd: RawFd, buf: &mut [u8]) -> io::Result<(usize, Vec<OwnedFd>)> {
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr() as *mut _,
        iov_len: buf.len(),
    };
    let cmsg_space = 64 + MAX_FDS * std::mem::size_of::<RawFd>();
    let mut cmsg_buf = vec![0u8; cmsg_space];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_buf.as_mut_ptr() as *mut _;
    msg.msg_controllen = cmsg_space as _;

    let n = unsafe { libc::recvmsg(fd, &mut msg, libc::MSG_CMSG_CLOEXEC) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }

    let mut fds = Vec::new();
    let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    while !cmsg.is_null() {
        let chdr = unsafe { *cmsg };
        if chdr.cmsg_level == libc::SOL_SOCKET && chdr.cmsg_type == libc::SCM_RIGHTS {
            let data = unsafe { libc::CMSG_DATA(cmsg) } as *const RawFd;
            let payload =
                (chdr.cmsg_len as usize).saturating_sub(unsafe { libc::CMSG_LEN(0) } as usize);
            let count = payload / std::mem::size_of::<RawFd>();
            for i in 0..count {
                let raw = unsafe { data.add(i).read_unaligned() };
                fds.push(unsafe { OwnedFd::from_raw_fd(raw) });
            }
        }
        cmsg = unsafe { libc::CMSG_NXTHDR(&msg, cmsg) };
    }
    Ok((n as usize, fds))
}

fn sendmsg_fds(fd: RawFd, buf: &[u8], fds: &[RawFd]) -> io::Result<usize> {
    if buf.is_empty() {
        return Ok(0);
    }
    let iov = libc::iovec {
        iov_base: buf.as_ptr() as *mut _,
        iov_len: buf.len(),
    };
    let cmsg_len = std::mem::size_of_val(fds);
    let cmsg_space = if fds.is_empty() {
        0
    } else {
        unsafe { libc::CMSG_SPACE(cmsg_len as _) as usize }
    };
    let mut cmsg_buf = vec![0u8; cmsg_space];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &iov as *const _ as *mut _;
    msg.msg_iovlen = 1;

    if !fds.is_empty() {
        msg.msg_control = cmsg_buf.as_mut_ptr() as *mut _;
        msg.msg_controllen = cmsg_space as _;
        let cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
        unsafe {
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(cmsg_len as _) as _;
            std::ptr::copy_nonoverlapping(
                fds.as_ptr() as *const u8,
                libc::CMSG_DATA(cmsg),
                cmsg_len,
            );
        }
    }

    let n = unsafe { libc::sendmsg(fd, &msg, libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(n as usize)
}

// ---------------------------------------------------------------------------
// DBus frame boundary detection (replaces Frame::bytes_needed)
// ---------------------------------------------------------------------------

const FIXED_HEADER_LEN: usize = 16;
const MAX_MESSAGE_SIZE: usize = 128 * 1024 * 1024;
const DBUS_PROTOCOL_VERSION: u8 = 1;

fn msg_bytes_needed(buf: &[u8]) -> Option<usize> {
    if buf.len() < FIXED_HEADER_LEN {
        return None;
    }
    if buf[3] != DBUS_PROTOCOL_VERSION {
        return None;
    }
    let endian = match buf[0] {
        b'l' => Endian::Little,
        b'B' => Endian::Big,
        _ => return None,
    };
    let body_len = endian.read_u32(&buf[4..8]) as usize;
    let fields_len = endian.read_u32(&buf[12..16]) as usize;
    let header_section = FIXED_HEADER_LEN.checked_add(fields_len)?;
    let body_start = align8(header_section)?;
    let total = body_start.checked_add(body_len)?;
    if total > MAX_MESSAGE_SIZE {
        return None;
    }
    Some(total)
}

fn align8(n: usize) -> Option<usize> {
    n.checked_add(7).map(|v| v & !7)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endian {
    Little,
    Big,
}

impl Endian {
    fn read_u32(self, bytes: &[u8]) -> u32 {
        match self {
            Endian::Little => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            Endian::Big => u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        }
    }
}

// ---------------------------------------------------------------------------
// Parse bytes into zbus::Message
// ---------------------------------------------------------------------------

fn parse_msg(bytes: &[u8]) -> Option<zbus::Message> {
    let ctx = zvariant::serialized::Context::new_dbus(zvariant::Endian::Little, 0);
    let data = zvariant::serialized::Data::new(bytes.to_vec(), ctx);
    unsafe { zbus::Message::from_bytes(data).ok() }
}

// ---------------------------------------------------------------------------
// IME signal builders (using zbus::Message::signal)
// ---------------------------------------------------------------------------

fn build_commit_signal(ic_path: &str, text: &str, _sender: Option<&str>) -> Option<Vec<u8>> {
    let msg = zbus::Message::signal(ic_path, fcitx::INPUT_CONTEXT_INTERFACE, "CommitString")
        .ok()?
        .build(&text.to_string())
        .ok()?;
    Some(msg.data().to_vec())
}

fn build_preedit_signal(
    ic_path: &str,
    text: &str,
    cursor_begin: i32,
    cursor_end: i32,
    _sender: Option<&str>,
) -> Option<Vec<u8>> {
    let cursor = if cursor_begin != cursor_end || cursor_begin >= 0 {
        Some((cursor_begin, cursor_end))
    } else {
        None
    };
    const UNDERLINE: i32 = 1 << 3;
    const HIGHLIGHT: i32 = 1 << 4;
    let chunks = build_preedit_chunks(text, cursor, UNDERLINE, HIGHLIGHT);
    let cursor_offset = cursor.map(|(_, e)| e).unwrap_or(-1);
    let msg = zbus::Message::signal(
        ic_path,
        fcitx::INPUT_CONTEXT_INTERFACE,
        "UpdateFormattedPreedit",
    )
    .ok()?
    .build(&(chunks, cursor_offset))
    .ok()?;
    Some(msg.data().to_vec())
}

// ---------------------------------------------------------------------------
// Per-client connection handler
// ---------------------------------------------------------------------------

struct ClientConn {
    client_fd: RawFd,
    upstream_fd: RawFd,
    client_buf: Vec<u8>,
    upstream_buf: Vec<u8>,
    client_in_fds: Vec<OwnedFd>,
    upstream_in_fds: Vec<OwnedFd>,
    authenticated: bool,
    ic_alloc: InputContextAllocator,
    fcitx_server_name: Option<String>,
    router: Arc<RouterState>,
}

impl ClientConn {
    fn new(client: &UnixStream, upstream: &UnixStream, router: Arc<RouterState>) -> Self {
        Self {
            client_fd: client.as_raw_fd(),
            upstream_fd: upstream.as_raw_fd(),
            client_buf: Vec::new(),
            upstream_buf: Vec::new(),
            client_in_fds: Vec::new(),
            upstream_in_fds: Vec::new(),
            authenticated: false,
            ic_alloc: InputContextAllocator::new(),
            fcitx_server_name: None,
            router,
        }
    }

    fn pump_upstream_to_client(&mut self) -> io::Result<bool> {
        let fd = self.upstream_fd;
        let mut tmp = [0u8; 8192];
        let (n, fds) = match recvmsg_fds(fd, &mut tmp) {
            Ok((0, _)) => return Ok(false),
            Ok(t) => t,
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(true),
            Err(e) => return Err(e),
        };
        self.upstream_buf.extend_from_slice(&tmp[..n]);
        self.upstream_in_fds.extend(fds);

        if !self.authenticated {
            self.forward_to_client()?;
            return Ok(true);
        }

        loop {
            match msg_bytes_needed(&self.upstream_buf) {
                None => break,
                Some(needed) if self.upstream_buf.len() < needed => break,
                Some(needed) => {
                    let bytes: Vec<u8> = self.upstream_buf.drain(..needed).collect();
                    if let Some(msg) = parse_msg(&bytes) {
                        let hdr = msg.header();
                        let iface = hdr.interface().map(|i| i.as_str());
                        let member = hdr.member().map(|m| m.as_str());
                        if iface == Some("org.freedesktop.DBus")
                            && member == Some("NameOwnerChanged")
                        {
                            let body = msg.body();
                            let body_data = body.data();
                            if let Ok(((name, _old, new_owner), _)) = body_data
                                .deserialize_for_signature::<&str, (String, String, String)>("sss")
                            {
                                if fcitx::is_fcitx_well_known(&name) {
                                    self.fcitx_server_name = if new_owner.is_empty() {
                                        None
                                    } else {
                                        Some(new_owner)
                                    };
                                    *self.router.fcitx_server_name.lock().unwrap() =
                                        self.fcitx_server_name.clone();
                                }
                            }
                        }
                    }
                    self.write_client(&bytes)?;
                }
            }
        }
        Ok(true)
    }

    fn pump_client_to_upstream(&mut self) -> io::Result<bool> {
        let fd = self.client_fd;
        let mut tmp = [0u8; 8192];
        let (n, fds) = match recvmsg_fds(fd, &mut tmp) {
            Ok((0, _)) => return Ok(false),
            Ok(t) => t,
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(true),
            Err(e) => return Err(e),
        };
        self.client_buf.extend_from_slice(&tmp[..n]);
        self.client_in_fds.extend(fds);

        if !self.authenticated {
            let bytes = std::mem::take(&mut self.client_buf);
            self.write_upstream(&bytes)?;
            if bytes.windows(7).any(|w| w == b"BEGIN\r\n") {
                self.authenticated = true;
            }
            return Ok(true);
        }

        loop {
            match msg_bytes_needed(&self.client_buf) {
                None => break,
                Some(needed) if self.client_buf.len() < needed => break,
                Some(needed) => {
                    let bytes: Vec<u8> = self.client_buf.drain(..needed).collect();
                    let msg = match parse_msg(&bytes) {
                        Some(m) => m,
                        None => {
                            self.write_upstream(&bytes)?;
                            continue;
                        }
                    };

                    let hdr = msg.header();
                    if let Some(fm) = classify(&bytes) {
                        if let Some(dest) = hdr.destination() {
                            let dest_str = dest.as_str();
                            if dest_str.starts_with(':')
                                && self.fcitx_server_name.as_deref() != Some(dest_str)
                            {
                                self.fcitx_server_name = Some(dest_str.to_string());
                                *self.router.fcitx_server_name.lock().unwrap() =
                                    self.fcitx_server_name.clone();
                            }
                        }
                        let reply = build_reply(&bytes, &fm, &mut self.ic_alloc);
                        let Some(reply) = reply else {
                            self.write_upstream(&bytes)?;
                            continue;
                        };
                        self.write_client(&reply)?;

                        if let Fcitx5MethodCall::CreateInputContext { .. } = &fm {
                            if let Some(reply_msg) = parse_msg(&reply) {
                                let body = reply_msg.body();
                                let body_data = body.data();
                                if let Ok(((path, _uuid), _)) = body_data
                                    .deserialize_for_signature::<&str, (String, Vec<u8>)>("oay")
                                {
                                    self.router.register_ic(&path, self.client_fd);
                                }
                            }
                        }
                        if let Some(event) = method_call_to_event(&fm) {
                            self.router
                                .push_event(RouterNotification::FcitxEvent(event));
                        }
                        continue;
                    }

                    self.write_upstream(&bytes)?;
                }
            }
        }
        Ok(true)
    }

    fn write_client(&mut self, data: &[u8]) -> io::Result<()> {
        let mut written = 0;
        while written < data.len() {
            match sendmsg_fds(self.client_fd, &data[written..], &[]) {
                Ok(n) => written += n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn write_upstream(&mut self, data: &[u8]) -> io::Result<()> {
        let mut written = 0;
        while written < data.len() {
            match sendmsg_fds(self.upstream_fd, &data[written..], &[]) {
                Ok(n) => written += n,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn forward_to_client(&mut self) -> io::Result<()> {
        let bytes = std::mem::take(&mut self.upstream_buf);
        self.write_client(&bytes)
    }

    fn pump(&mut self) -> io::Result<bool> {
        let c_alive = self.pump_client_to_upstream()?;
        let u_alive = self.pump_upstream_to_client()?;
        Ok(c_alive && u_alive)
    }
}

// ---------------------------------------------------------------------------
// IPC handler
// ---------------------------------------------------------------------------

fn send_frame(stream: &mut UnixStream, data: &[u8]) -> io::Result<()> {
    let header = format!("Content-Length: {}\r\n\r\n", data.len());
    stream.write_all(header.as_bytes())?;
    stream.write_all(data)?;
    Ok(())
}

fn try_read_frame(stream: &mut UnixStream, buf: &mut Vec<u8>) -> io::Result<Option<Vec<u8>>> {
    let mut tmp = [0u8; 16384];
    match stream.read(&mut tmp) {
        Ok(0) => return Ok(None),
        Ok(n) => buf.extend_from_slice(&tmp[..n]),
        Err(e) if e.kind() == ErrorKind::WouldBlock => {}
        Err(e) => return Err(e),
    }

    if buf.len() < 2 {
        return Ok(None);
    }

    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|pos| pos + 4);

    let Some(end) = header_end else {
        return Ok(None);
    };

    let header = std::str::from_utf8(&buf[..end]).unwrap_or("");
    let len = header
        .lines()
        .find_map(|line| {
            line.strip_prefix("Content-Length:")
                .and_then(|s| s.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);

    if len == 0 || buf.len() < end + len {
        return Ok(None);
    }

    let frame: Vec<u8> = buf.drain(..end + len).collect();
    let body = frame[end..].to_vec();
    Ok(Some(body))
}

fn handle_ipc(mut ipc: UnixStream, state: Arc<RouterState>) {
    ipc.set_nonblocking(true).ok();
    let mut read_buf = Vec::new();

    loop {
        let frame = match try_read_frame(&mut ipc, &mut read_buf) {
            Ok(Some(f)) => f,
            Ok(None) => {
                drain_and_send_events(&mut ipc, &state);
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(_) => break,
        };

        let request: RouterRequest = match serde_json::from_slice(&frame) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "IPC: invalid RouterRequest");
                continue;
            }
        };

        match request {
            RouterRequest::AddRule { rule } => {
                state.routing_table.lock().unwrap().add(rule.clone());
                state.push_event(RouterNotification::RuleAdded {
                    id: rule.id.clone(),
                    rule,
                });
            }
            RouterRequest::RemoveRule { id } => {
                state.routing_table.lock().unwrap().remove(&id);
                state.push_event(RouterNotification::RuleRemoved { id });
            }
            RouterRequest::ListRules => {
                let rules = state.routing_table.lock().unwrap().rules().to_vec();
                state.push_event(RouterNotification::RuleList { rules });
            }
            RouterRequest::ImeCommit { ic_path, text } => {
                let sender = state.fcitx_server_name.lock().unwrap().clone();
                if let Some(bytes) = build_commit_signal(&ic_path, &text, sender.as_deref()) {
                    if let Some(fd) = state.lookup_ic(&ic_path) {
                        if let Err(e) = sendmsg_fds(fd, &bytes, &[]) {
                            tracing::warn!(error = %e, ic_path, "ImeCommit: sendmsg failed");
                        }
                    }
                }
            }
            RouterRequest::ImePreedit {
                ic_path,
                text,
                cursor_begin,
                cursor_end,
            } => {
                let sender = state.fcitx_server_name.lock().unwrap().clone();
                if let Some(bytes) = build_preedit_signal(
                    &ic_path,
                    &text,
                    cursor_begin,
                    cursor_end,
                    sender.as_deref(),
                ) {
                    if let Some(fd) = state.lookup_ic(&ic_path) {
                        if let Err(e) = sendmsg_fds(fd, &bytes, &[]) {
                            tracing::warn!(error = %e, ic_path, "ImePreedit: sendmsg failed");
                        }
                    }
                }
            }
        }

        drain_and_send_events(&mut ipc, &state);
    }
}

fn drain_and_send_events(ipc: &mut UnixStream, state: &Arc<RouterState>) {
    let events = state.drain_events();
    for event in events {
        let data = match serde_json::to_string(&event) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(error = %e, "IPC: serialize event failed");
                continue;
            }
        };
        if let Err(e) = send_frame(ipc, data.as_bytes()) {
            tracing::warn!(error = %e, "IPC: send event failed");
            state.push_event(event);
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    let upstream_path: PathBuf = if let Some(ref p) = args.upstream {
        p.clone()
    } else {
        let addr = std::env::var("DBUS_SESSION_BUS_ADDRESS")
            .map_err(|_| "DBUS_SESSION_BUS_ADDRESS not set")?;
        parse_bus_address(&addr)?
    };

    let _ = std::fs::remove_file(&args.listen);
    let listener = std::os::unix::net::UnixListener::bind(&args.listen)?;
    listener.set_nonblocking(true)?;

    let _ = std::fs::remove_file(&args.ipc);
    let ipc_listener = std::os::unix::net::UnixListener::bind(&args.ipc)?;
    ipc_listener.set_nonblocking(true)?;

    let state = RouterState::new();

    loop {
        match listener.accept() {
            Ok((client, _addr)) => {
                client.set_nonblocking(true)?;
                let upstream = UnixStream::connect(&upstream_path)?;
                upstream.set_nonblocking(true)?;
                let state = state.clone();
                tokio::task::spawn_blocking(move || {
                    let mut conn = ClientConn::new(&client, &upstream, state);
                    loop {
                        match conn.pump() {
                            Ok(true) => std::thread::sleep(Duration::from_millis(1)),
                            Ok(false) => break,
                            Err(e) => {
                                tracing::debug!(error = %e, "client pump error");
                                break;
                            }
                        }
                    }
                });
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
            Err(e) => {
                tracing::warn!(error = %e, "accept error");
            }
        }

        match ipc_listener.accept() {
            Ok((ipc, _addr)) => {
                ipc.set_nonblocking(true)?;
                let state = state.clone();
                tokio::task::spawn_blocking(|| {
                    handle_ipc(ipc, state);
                });
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
            Err(e) => {
                tracing::warn!(error = %e, "ipc accept error");
            }
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

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
