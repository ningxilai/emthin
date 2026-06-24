# JSON-RPC 2.0 IPC Protocol — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace custom 4-byte LE framing + serde tagged-enum IPC protocol with JSON-RPC 2.0 + `Content-Length` header framing, using Emacs's built-in `jsonrpc.el`.

**Architecture:** Two-layer split — `connection.rs` owns byte framing (`Content-Length: N\r\n\r\n`), `jsonrpc.rs` owns protocol envelope (`jsonrpc/method/params`). `messages.rs` enums hold data without serde derives. `emskin-ipc.el` delegates to `jsonrpc-process-connection`.

**Tech Stack:** Rust serde_json (existing dep), Emacs jsonrpc.el (built-in)

---

### Task 1: connection.rs — Rewrite framing to Content-Length headers

**Files:**
- Modify: `crates/emskin/src/ipc/connection.rs`
- Test: `crates/emskin/src/ipc/connection.rs` (inline `#[cfg(test)]`)

**Design:**
- `try_recv()`: scan for `Content-Length: N\r\n\r\n` instead of reading 4-byte LE prefix
- `enqueue()` → rename `enqueue_raw(bytes: &[u8])`: prepend `Content-Length: N\r\n\r\n` header
- `MAX_MSG_SIZE` kept at 1 MiB (applies to the JSON body after headers)

- [ ] **Step 1: Write failing tests for Content-Length framing**

Replace the existing test helper `write_framed` and all test bodies.

```rust
// Inside mod tests — replace write_framed and all test functions
fn write_content_length(stream: &mut UnixStream, payload: &[u8]) {
    let header = format!("Content-Length: {}\r\n\r\n", payload.len());
    stream.write_all(header.as_bytes()).unwrap();
    stream.write_all(payload).unwrap();
}

#[test]
fn try_recv_returns_none_on_empty_buffer() {
    let (mut conn, _peer) = make_pair();
    assert!(conn.try_recv().unwrap().is_none());
}

#[test]
fn try_recv_returns_none_on_incomplete_header() {
    let (mut conn, mut peer) = make_pair();
    peer.write_all(b"Content-L").unwrap(); // incomplete header, no \r\n\r\n
    conn.fill_read_buf().ok();
    assert!(conn.try_recv().unwrap().is_none());
}

#[test]
fn try_recv_decodes_single_message() {
    let (mut conn, mut peer) = make_pair();
    let payload = b"hello world";
    write_content_length(&mut peer, payload);
    conn.fill_read_buf().ok();
    let msg = conn.try_recv().unwrap().unwrap();
    assert_eq!(msg, payload);
}

#[test]
fn try_recv_returns_none_on_incomplete_payload() {
    let (mut conn, mut peer) = make_pair();
    // Header says 10 bytes, but only send 5
    peer.write_all(b"Content-Length: 10\r\n\r\n").unwrap();
    peer.write_all(b"hello").unwrap();
    conn.fill_read_buf().ok();
    assert!(conn.try_recv().unwrap().is_none());
}

#[test]
fn try_recv_handles_multiple_messages_in_one_read() {
    let (mut conn, mut peer) = make_pair();
    write_content_length(&mut peer, b"msg1");
    write_content_length(&mut peer, b"msg2");
    write_content_length(&mut peer, b"msg3");
    conn.fill_read_buf().ok();

    assert_eq!(conn.try_recv().unwrap().unwrap(), b"msg1");
    assert_eq!(conn.try_recv().unwrap().unwrap(), b"msg2");
    assert_eq!(conn.try_recv().unwrap().unwrap(), b"msg3");
    assert!(conn.try_recv().unwrap().is_none());
}

#[test]
fn try_recv_handles_empty_payload() {
    let (mut conn, mut peer) = make_pair();
    write_content_length(&mut peer, b"");
    conn.fill_read_buf().ok();
    let msg = conn.try_recv().unwrap().unwrap();
    assert!(msg.is_empty());
}

#[test]
fn try_recv_rejects_oversized_message() {
    let (mut conn, mut peer) = make_pair();
    // Header claims 2 MiB (exceeds MAX_MSG_SIZE of 1 MiB)
    let huge = b"Content-Length: 2097152\r\n\r\n";
    peer.write_all(huge).unwrap();
    conn.fill_read_buf().ok();
    let result = conn.try_recv();
    assert!(result.is_err());
}

#[test]
fn enqueue_and_flush_roundtrip() {
    let (mut conn, mut peer) = make_pair();
    peer.set_nonblocking(true).unwrap();

    conn.enqueue_raw(b"{\"type\":\"connected\",\"version\":\"0.1\"}");
    assert!(conn.has_pending_writes());

    conn.try_flush().unwrap();
    assert!(!conn.has_pending_writes());

    // Read the Content-Length framed message from the peer side
    peer.set_nonblocking(false).unwrap();
    let mut expected = "Content-Length: 35\r\n\r\n{\"type\":\"connected\",\"version\":\"0.1\"}".as_bytes();
    let mut buf = vec![0u8; expected.len()];
    peer.read_exact(&mut buf).unwrap();
    assert_eq!(buf, expected);
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p emskin -- ipc::connection::tests 2>&1 | head -30
```

Expected: compilation errors — `enqueue` renamed, `write_framed` removed, `try_recv` signature may differ.

- [ ] **Step 3: Implement Content-Length framing**

Replace the body of `try_recv` and `enqueue`:

```rust
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;

pub struct IpcConn {
    pub(super) stream: UnixStream,
    read_buf: Vec<u8>,
    write_buf: VecDeque<u8>,
}

impl IpcConn {
    pub fn new(stream: UnixStream) -> io::Result<Self> {
        stream.set_nonblocking(true)?;
        Ok(Self { stream, read_buf: Vec::new(), write_buf: VecDeque::new() })
    }

    pub fn fill_read_buf(&mut self) -> io::Result<bool> {
        let mut tmp = [0u8; 4096];
        loop {
            match self.stream.read(&mut tmp) {
                Ok(0) => return Ok(true),
                Ok(n) => self.read_buf.extend_from_slice(&tmp[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(false),
                Err(e) => return Err(e),
            }
        }
    }

    /// Parse one `Content-Length: N\r\n\r\n<body>` message from `read_buf`.
    pub fn try_recv(&mut self) -> io::Result<Option<Vec<u8>>> {
        // Find the \r\n\r\n header terminator
        let header_end = self.read_buf.windows(4).position(|w| w == b"\r\n\r\n");
        let Some(header_end) = header_end else {
            return Ok(None);
        };
        let header = &self.read_buf[..header_end];
        // Parse "Content-Length: N"
        let header_str = std::str::from_utf8(header).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "non-utf8 header")
        })?;
        let len = header_str
            .strip_prefix("Content-Length:")
            .and_then(|s| s.trim().parse::<usize>().ok())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length")
            })?;
        if len > MAX_MSG_SIZE {
            self.read_buf.clear();
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Content-Length {len} exceeds maximum {MAX_MSG_SIZE}"),
            ));
        }
        let body_start = header_end + 4; // after \r\n\r\n
        let body_end = body_start + len;
        if self.read_buf.len() < body_end {
            return Ok(None);
        }
        let payload = self.read_buf[body_start..body_end].to_vec();
        self.read_buf.drain(..body_end);
        Ok(Some(payload))
    }

    /// Enqueue raw JSON bytes wrapped in `Content-Length: N\r\n\r\n`.
    pub fn enqueue_raw(&mut self, data: &[u8]) {
        let header = format!("Content-Length: {}\r\n\r\n", data.len());
        self.write_buf.extend(header.as_bytes());
        self.write_buf.extend(data);
    }

    pub fn try_flush(&mut self) -> io::Result<bool> {
        while !self.write_buf.is_empty() {
            let (front, back) = self.write_buf.as_slices();
            let slice = if !front.is_empty() { front } else { back };
            match self.stream.write(slice) {
                Ok(0) => return Ok(!self.write_buf.is_empty()),
                Ok(n) => { self.write_buf.drain(..n); }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(true),
                Err(e) => return Err(e),
            }
        }
        Ok(false)
    }

    #[cfg(test)]
    pub fn has_pending_writes(&self) -> bool {
        !self.write_buf.is_empty()
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p emskin -- ipc::connection::tests 2>&1
```

Expected: 8 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/emskin/src/ipc/connection.rs
git commit -m "refactor(ipc): Content-Length framing in connection.rs"
```

---

### Task 2: messages.rs — Strip serde derives, add from_jsonrpc_params

**Files:**
- Modify: `crates/emskin/src/ipc/messages.rs`
- Test: `crates/emskin/src/ipc/messages.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Remove serde derives, add from_jsonrpc_params**

```rust
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct IpcRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Emacs → emskin
#[derive(Debug, Clone, PartialEq)]
pub enum IncomingMessage {
    SetGeometry { window_id: u64, rect: IpcRect },
    Close { window_id: u64 },
    SetVisibility { window_id: u64, visible: bool },
    PrefixDone,
    PrefixClear,
    AddMirror { window_id: u64, view_id: u64, rect: IpcRect },
    UpdateMirrorGeometry { window_id: u64, view_id: u64, rect: IpcRect },
    RemoveMirror { window_id: u64, view_id: u64 },
    PromoteMirror { window_id: u64, view_id: u64 },
    SetFocus { window_id: Option<u64> },
    SwitchWorkspace { workspace_id: u64 },
}

impl IncomingMessage {
    pub fn from_jsonrpc(method: &str, params: &serde_json::Value) -> Result<Self, String> {
        Ok(match method {
            "set_geometry" => Self::SetGeometry {
                window_id: params_get_u64(params, "window_id")?,
                rect: IpcRect {
                    x: params_get_i32(params, "x")?,
                    y: params_get_i32(params, "y")?,
                    w: params_get_i32(params, "w")?,
                    h: params_get_i32(params, "h")?,
                },
            },
            "close" => Self::Close {
                window_id: params_get_u64(params, "window_id")?,
            },
            "set_visibility" => Self::SetVisibility {
                window_id: params_get_u64(params, "window_id")?,
                visible: params_get_bool(params, "visible")?,
            },
            "prefix_done" => Self::PrefixDone,
            "prefix_clear" => Self::PrefixClear,
            "add_mirror" => Self::AddMirror {
                window_id: params_get_u64(params, "window_id")?,
                view_id: params_get_u64(params, "view_id")?,
                rect: IpcRect {
                    x: params_get_i32(params, "x")?,
                    y: params_get_i32(params, "y")?,
                    w: params_get_i32(params, "w")?,
                    h: params_get_i32(params, "h")?,
                },
            },
            "update_mirror_geometry" => Self::UpdateMirrorGeometry {
                window_id: params_get_u64(params, "window_id")?,
                view_id: params_get_u64(params, "view_id")?,
                rect: IpcRect {
                    x: params_get_i32(params, "x")?,
                    y: params_get_i32(params, "y")?,
                    w: params_get_i32(params, "w")?,
                    h: params_get_i32(params, "h")?,
                },
            },
            "remove_mirror" => Self::RemoveMirror {
                window_id: params_get_u64(params, "window_id")?,
                view_id: params_get_u64(params, "view_id")?,
            },
            "promote_mirror" => Self::PromoteMirror {
                window_id: params_get_u64(params, "window_id")?,
                view_id: params_get_u64(params, "view_id")?,
            },
            "set_focus" => Self::SetFocus {
                window_id: params.get("window_id").and_then(|v| v.as_u64()),
            },
            "switch_workspace" => Self::SwitchWorkspace {
                workspace_id: params_get_u64(params, "workspace_id")?,
            },
            other => return Err(format!("unknown IPC method: {other}")),
        })
    }
}

fn params_get_u64(params: &serde_json::Value, key: &str) -> Result<u64, String> {
    params[key].as_u64().ok_or_else(|| format!("missing/invalid {key}"))
}

fn params_get_i32(params: &serde_json::Value, key: &str) -> Result<i32, String> {
    params[key].as_i64()
        .ok_or_else(|| format!("missing/invalid field '{key}'"))
        .map(|v| v as i32)
}

fn params_get_bool(params: &serde_json::Value, key: &str) -> Result<bool, String> {
    params[key].as_bool().ok_or_else(|| format!("missing/invalid field '{key}'"))
}

Helpers:
fn params_get_u64(params: &serde_json::Value, key: &str) -> Result<u64, String> {
    params[key].as_u64().ok_or_else(|| format!("missing/invalid field '{key}'"))
}

fn params_get_i32(params: &serde_json::Value, key: &str) -> Result<i32, String> {
    params[key].as_i64()
        .ok_or_else(|| format!("missing/invalid field '{key}'"))
        .map(|v| v as i32)
}

fn params_get_bool(params: &serde_json::Value, key: &str) -> Result<bool, String> {
    params[key].as_bool().ok_or_else(|| format!("missing/invalid field '{key}'"))
}
```

Also update `OutgoingMessage`: remove `#[derive(Serialize)]`, add `method_name()` and `into_params_value()`:

```rust
/// emskin → Emacs
#[derive(Debug, Clone)]
pub enum OutgoingMessage {
    Connected { version: &'static str },
    WindowCreated { window_id: u64, title: String },
    WindowDestroyed { window_id: u64 },
    TitleChanged { window_id: u64, title: String },
    SurfaceSize { width: i32, height: i32 },
    FocusView { window_id: u64, view_id: u64 },
    XWaylandReady { display: u32 },
    WorkspaceCreated { workspace_id: u64 },
    WorkspaceSwitched { workspace_id: u64 },
    WorkspaceDestroyed { workspace_id: u64 },
}

impl OutgoingMessage {
    pub fn method_name(&self) -> &'static str {
        match self {
            Self::Connected { .. } => "connected",
            Self::WindowCreated { .. } => "window_created",
            Self::WindowDestroyed { .. } => "window_destroyed",
            Self::TitleChanged { .. } => "title_changed",
            Self::SurfaceSize { .. } => "surface_size",
            Self::FocusView { .. } => "focus_view",
            Self::XWaylandReady { .. } => "x_wayland_ready",
            Self::WorkspaceCreated { .. } => "workspace_created",
            Self::WorkspaceSwitched { .. } => "workspace_switched",
            Self::WorkspaceDestroyed { .. } => "workspace_destroyed",
        }
    }

    pub fn into_params_value(self) -> serde_json::Value {
        match self {
            Self::Connected { version } => serde_json::json!({"version": version}),
            Self::WindowCreated { window_id, title } => serde_json::json!({"window_id": window_id, "title": title}),
            Self::WindowDestroyed { window_id } => serde_json::json!({"window_id": window_id}),
            Self::TitleChanged { window_id, title } => serde_json::json!({"window_id": window_id, "title": title}),
            Self::SurfaceSize { width, height } => serde_json::json!({"width": width, "height": height}),
            Self::FocusView { window_id, view_id } => serde_json::json!({"window_id": window_id, "view_id": view_id}),
            Self::XWaylandReady { display } => serde_json::json!({"display": display}),
            Self::WorkspaceCreated { workspace_id } => serde_json::json!({"workspace_id": workspace_id}),
            Self::WorkspaceSwitched { workspace_id } => serde_json::json!({"workspace_id": workspace_id}),
            Self::WorkspaceDestroyed { workspace_id } => serde_json::json!({"workspace_id": workspace_id}),
        }
    }
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check -p emskin 2>&1
```

Expected: compiles. Some warnings about dead code in `IpcRect` `Serialize` (used by tests) — OK.

- [ ] **Step 3: Update inline tests**

Replace all existing test functions (they tested `serde_json::from_str`/`serde_json::to_string` directly on enums which no longer work).

Replace with tests that exercise `from_jsonrpc`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn call_from_jsonrpc(method: &str, params: serde_json::Value) -> Result<IncomingMessage, String> {
        IncomingMessage::from_jsonrpc(method, &params)
    }

    #[test]
    fn parses_set_geometry() {
        let params = serde_json::json!({"window_id":42,"x":10,"y":20,"w":800,"h":600});
        let msg = call_from_jsonrpc("set_geometry", params).unwrap();
        assert!(matches!(msg, IncomingMessage::SetGeometry { window_id: 42, rect: IpcRect { x: 10, y: 20, w: 800, h: 600 }}));
    }

    #[test]
    fn parses_close() {
        let params = serde_json::json!({"window_id":7});
        let msg = call_from_jsonrpc("close", params).unwrap();
        assert!(matches!(msg, IncomingMessage::Close { window_id: 7 }));
    }

    #[test]
    fn parses_set_visibility() {
        let params = serde_json::json!({"window_id":3,"visible":false});
        let msg = call_from_jsonrpc("set_visibility", params).unwrap();
        assert!(matches!(msg, IncomingMessage::SetVisibility { window_id: 3, visible: false }));
    }

    #[test]
    fn parses_prefix_done() {
        let params = serde_json::Value::Null;
        let msg = call_from_jsonrpc("prefix_done", params).unwrap();
        assert!(matches!(msg, IncomingMessage::PrefixDone));
    }

    #[test]
    fn parses_add_mirror() {
        let params = serde_json::json!({"window_id":1,"view_id":2,"x":0,"y":0,"w":400,"h":300});
        let msg = call_from_jsonrpc("add_mirror", params).unwrap();
        assert!(matches!(msg, IncomingMessage::AddMirror { window_id: 1, view_id: 2, .. }));
    }

    #[test]
    fn parses_set_focus_with_window_id() {
        let params = serde_json::json!({"window_id":9});
        let msg = call_from_jsonrpc("set_focus", params).unwrap();
        assert!(matches!(msg, IncomingMessage::SetFocus { window_id: Some(9) }));
    }

    #[test]
    fn parses_set_focus_without_window_id() {
        let params = serde_json::json!({});
        let msg = call_from_jsonrpc("set_focus", params).unwrap();
        assert!(matches!(msg, IncomingMessage::SetFocus { window_id: None }));
    }

    #[test]
    fn parses_switch_workspace() {
        let params = serde_json::json!({"workspace_id":5});
        let msg = call_from_jsonrpc("switch_workspace", params).unwrap();
        assert!(matches!(msg, IncomingMessage::SwitchWorkspace { workspace_id: 5 }));
    }

    #[test]
    fn rejects_unknown_method() {
        let params = serde_json::json!({});
        let result = call_from_jsonrpc("unknown_command", params);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_missing_required_fields() {
        let params = serde_json::json!({"window_id":1});
        let result = call_from_jsonrpc("set_geometry", params);
        assert!(result.is_err());
    }

    #[test]
    fn outgoing_method_name_is_snake_case() {
        assert_eq!(OutgoingMessage::Connected { version: "0.1" }.method_name(), "connected");
        assert_eq!(OutgoingMessage::WindowCreated { window_id: 1, title: "t".into() }.method_name(), "window_created");
        assert_eq!(OutgoingMessage::XWaylandReady { display: 42 }.method_name(), "x_wayland_ready");
        assert_eq!(OutgoingMessage::SurfaceSize { width: 1920, height: 1080 }.method_name(), "surface_size");
    }

    #[test]
    fn outgoing_into_params_value() {
        let v = OutgoingMessage::Connected { version: "0.1" }.into_params_value();
        assert_eq!(v["version"], "0.1");
        let v = OutgoingMessage::WindowCreated { window_id: 42, title: "test".into() }.into_params_value();
        assert_eq!(v["window_id"], 42);
        assert_eq!(v["title"], "test");
        let v = OutgoingMessage::XWaylandReady { display: 99 }.into_params_value();
        assert_eq!(v["display"], 99);
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p emskin -- ipc::messages::tests 2>&1
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/emskin/src/ipc/messages.rs
git commit -m "refactor(ipc): strip serde derives, add from_jsonrpc"
```

---

### Task 3: jsonrpc.rs — New JSON-RPC envelope module

**Files:**
- Create: `crates/emskin/src/ipc/jsonrpc.rs`
- Test: inline `#[cfg(test)]`

- [ ] **Step 1: Write the module**

```rust
use crate::ipc::{IncomingMessage, OutgoingMessage};

/// Parse a JSON-RPC 2.0 notification payload into an `IncomingMessage`.
pub fn parse_incoming(payload: &[u8]) -> Result<IncomingMessage, String> {
    let v: serde_json::Value = serde_json::from_slice(payload)
        .map_err(|e| format!("JSON parse error: {e}"))?;
    let jsonrpc = v.get("jsonrpc").and_then(|v| v.as_str()).unwrap_or("");
    if jsonrpc != "2.0" {
        return Err(format!("invalid jsonrpc version: {jsonrpc:?}"));
    }
    let method = v["method"].as_str()
        .ok_or_else(|| "missing 'method' field".to_string())?;
    let params = v.get("params").unwrap_or(&serde_json::Value::Null);
    IncomingMessage::from_jsonrpc(method, params)
}

/// Serialize an `OutgoingMessage` as a JSON-RPC 2.0 notification.
pub fn serialize_outgoing(msg: OutgoingMessage) -> Result<Vec<u8>, serde_json::Error> {
    let method = msg.method_name();
    let params = msg.into_params_value();
    let envelope = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    serde_json::to_vec(&envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{IncomingMessage, IpcRect};

    #[test]
    fn roundtrip_connected() {
        let msg = OutgoingMessage::Connected { version: "0.1" };
        let wire = serialize_outgoing(msg).unwrap();
        let wire_str = String::from_utf8_lossy(&wire);
        assert!(wire_str.contains(r#""jsonrpc":"2.0""#));
        assert!(wire_str.contains(r#""method":"connected""#));
        assert!(wire_str.contains(r#""version":"0.1""#));
    }

    #[test]
    fn parse_set_geometry() {
        let wire = br#"{"jsonrpc":"2.0","method":"set_geometry","params":{"window_id":42,"x":10,"y":20,"w":800,"h":600}}"#;
        let msg = parse_incoming(wire).unwrap();
        assert!(matches!(msg, IncomingMessage::SetGeometry { window_id: 42, rect: IpcRect { x: 10, y: 20, w: 800, h: 600 }}));
    }

    #[test]
    fn parse_close() {
        let wire = br#"{"jsonrpc":"2.0","method":"close","params":{"window_id":7}}"#;
        let msg = parse_incoming(wire).unwrap();
        assert!(matches!(msg, IncomingMessage::Close { window_id: 7 }));
    }

    #[test]
    fn parse_prefix_done() {
        let wire = br#"{"jsonrpc":"2.0","method":"prefix_done","params":null}"#;
        let msg = parse_incoming(wire).unwrap();
        assert!(matches!(msg, IncomingMessage::PrefixDone));
    }

    #[test]
    fn parse_set_focus_no_window_id() {
        let wire = br#"{"jsonrpc":"2.0","method":"set_focus","params":{}}"#;
        let msg = parse_incoming(wire).unwrap();
        assert!(matches!(msg, IncomingMessage::SetFocus { window_id: None }));
    }

    #[test]
    fn rejects_missing_jsonrpc_field() {
        let wire = br#"{"method":"close","params":{"window_id":1}}"#;
        let msg = parse_incoming(wire);
        assert!(msg.is_err());
    }

    #[test]
    fn rejects_unknown_method() {
        let wire = br#"{"jsonrpc":"2.0","method":"bogus","params":{}}"#;
        let msg = parse_incoming(wire);
        assert!(msg.is_err());
    }
}
```

- [ ] **Step 2: Register in mod.rs**

```rust
// in crates/emskin/src/ipc/mod.rs
pub mod jsonrpc;
```

- [ ] **Step 3: Verify compilation + run tests**

```bash
cargo test -p emskin -- ipc::jsonrpc::tests 2>&1
```

Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add crates/emskin/src/ipc/jsonrpc.rs crates/emskin/src/ipc/mod.rs
git commit -m "feat(ipc): JSON-RPC 2.0 envelope module"
```

---

### Task 4: mod.rs (IpcServer) — Wire jsonrpc.rs into send/recv

**Files:**
- Modify: `crates/emskin/src/ipc/mod.rs`

- [ ] **Step 1: Update IpcServer::accept, send, recv_all**

Add `use crate::ipc::jsonrpc;` at top.

Replace `conn.enqueue(&msg)` with serialize-then-enqueue_raw in `accept` and `send`:

```rust
// In accept():
conn.enqueue_raw(&jsonrpc::serialize_outgoing(OutgoingMessage::Connected { version: "0.1" }).unwrap());
for msg in self.pending.drain(..) {
    if let Ok(json) = jsonrpc::serialize_outgoing(msg) {
        conn.enqueue_raw(&json);
    }
}

// In send():
pub fn send(&mut self, msg: OutgoingMessage) {
    let Some(conn) = &mut self.connection else {
        self.pending.push(msg);
        return;
    };
    let json = match jsonrpc::serialize_outgoing(msg) {
        Ok(j) => j,
        Err(e) => { tracing::error!("IPC serialize error: {e}"); return; }
    };
    conn.enqueue_raw(&json);
    if let Err(e) = conn.try_flush() {
        tracing::warn!("IPC write error: {e}");
        self.connection = None;
    }
}
```

Replace `serde_json::from_slice::<IncomingMessage>` in `recv_all()`:

```rust
// In recv_all(), replace:
match serde_json::from_slice::<IncomingMessage>(&payload) {
    Ok(msg) => msgs.push(msg),
    Err(e) => { tracing::warn!("IPC parse error: {e} — payload: {}", String::from_utf8_lossy(&payload)); }
},
// with:
match jsonrpc::parse_incoming(&payload) {
    Ok(msg) => msgs.push(msg),
    Err(e) => { tracing::warn!("IPC parse error: {e} — payload: {}", String::from_utf8_lossy(&payload)); }
},
```

Also remove `use crate::ipc::IncomingMessage` at top if it's now imported differently.

The full `mod.rs` changes — show the affected functions:

```rust
use std::os::unix::net::UnixListener;
use connection::IpcConn;
pub use messages::{IncomingMessage, IpcRect, OutgoingMessage};
pub mod jsonrpc;

impl IpcServer {
    pub fn accept(&mut self) -> bool {
        match self.listener.accept() {
            Ok((stream, _)) => {
                match IpcConn::new(stream) {
                    Ok(mut conn) => {
                        tracing::info!("Emacs IPC connected");
                        let ok = jsonrpc::serialize_outgoing(OutgoingMessage::Connected { version: "0.1" });
                        if let Ok(json) = ok {
                            conn.enqueue_raw(&json);
                        }
                        for msg in self.pending.drain(..) {
                            if let Ok(json) = jsonrpc::serialize_outgoing(msg) {
                                conn.enqueue_raw(&json);
                            }
                        }
                        let _ = conn.try_flush();
                        self.connection = Some(conn);
                        true
                    }
                    Err(e) => { tracing::error!("IPC accept error: {e}"); false }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => false,
            Err(e) => { tracing::error!("IPC listener error: {e}"); false }
        }
    }

    pub fn send(&mut self, msg: OutgoingMessage) {
        let Some(conn) = &mut self.connection else {
            self.pending.push(msg);
            return;
        };
        let json = match jsonrpc::serialize_outgoing(msg) {
            Ok(j) => j,
            Err(e) => { tracing::error!("IPC serialize error: {e}"); return; }
        };
        conn.enqueue_raw(&json);
        if let Err(e) = conn.try_flush() {
            tracing::warn!("IPC write error: {e}");
            self.connection = None;
        }
    }

    pub fn recv_all(&mut self) -> Option<Vec<IncomingMessage>> {
        let conn = self.connection.as_mut()?;
        match conn.fill_read_buf() {
            Err(e) => { tracing::warn!("IPC read error: {e}"); self.connection = None; return None; }
            Ok(true) => { tracing::info!("Emacs IPC disconnected"); self.connection = None; return None; }
            Ok(false) => {}
        }
        let mut msgs = Vec::new();
        loop {
            match conn.try_recv() {
                Ok(Some(payload)) => match jsonrpc::parse_incoming(&payload) {
                    Ok(msg) => msgs.push(msg),
                    Err(e) => tracing::warn!("IPC parse error: {e} — payload: {}", String::from_utf8_lossy(&payload)),
                },
                Ok(None) => break,
                Err(e) => { tracing::warn!("IPC protocol error: {e}"); self.connection = None; return None; }
            }
        }
        Some(msgs)
    }
}
```

- [ ] **Step 2: Verify compilation**

```bash
cargo check -p emskin 2>&1
```

Expected: compiles clean.

- [ ] **Step 3: Run all IPC tests**

```bash
cargo test -p emskin -- ipc:: 2>&1
```

Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add crates/emskin/src/ipc/mod.rs
git commit -m "refactor(ipc): wire JSON-RPC into IpcServer send/recv"
```

---

### Task 5: emskin-ipc.el — Rewrite with jsonrpc.el

**Files:**
- Rewrite: `elisp/emskin-ipc.el`

- [ ] **Step 1: Replace entire file content**

```elisp
;;; emskin-ipc.el --- IPC connection and protocol for emskin  -*- lexical-binding: t; -*-

(require 'jsonrpc)

;; ---------------------------------------------------------------------------
;; IPC connection state
;; ---------------------------------------------------------------------------

(defvar emskin--jsonrpc-conn nil
  "JSON-RPC connection to emskin compositor.")

(defvar emskin-ipc-path nil
  "Explicit IPC socket path.  When nil, auto-discovered via parent PID.")

;; ---------------------------------------------------------------------------
;; Hooks
;; ---------------------------------------------------------------------------

(defvar emskin--message-hook nil
  "Hook run with (METHOD PARAMS) for each incoming JSON-RPC notification.")

(defvar emskin-connected-hook nil
  "Hook run after the IPC connection to emskin is (re-)established.")

;; ---------------------------------------------------------------------------
;; Sending
;; ---------------------------------------------------------------------------

(defun emskin--send (method params)
  "Send JSON-RPC notification METHOD with PARAMS.
PARAMS is a plist suitable for `json-serialize'."
  (when emskin--jsonrpc-conn
    (jsonrpc-notify emskin--jsonrpc-conn method params)))

(defun emskin--send-thunk (method params)
  "Return thunk that sends METHOD+PARAMS when called.
Encoding happens at thunk-creation time; network write happens
when the thunk is called."
  (let ((conn emskin--jsonrpc-conn))
    (lambda ()
      (when conn (jsonrpc-notify conn method params)))))

;; ---------------------------------------------------------------------------
;; Socket discovery
;; ---------------------------------------------------------------------------

(defun emskin--ipc-path ()
  "Return the IPC socket path, auto-discovering via parent PID."
  (or emskin-ipc-path
      (with-temp-buffer
        (insert-file-contents-literally
         (format "/proc/%d/status" (emacs-pid)))
        (goto-char (point-min))
        (let ((ppid (and (re-search-forward "^PPid:\t\\([0-9]+\\)" nil t)
                         (match-string 1))))
          (format "%s/emskin-%s.ipc"
                  (or (getenv "XDG_RUNTIME_DIR") "/tmp")
                  ppid)))))

;; ---------------------------------------------------------------------------
;; Connection
;; ---------------------------------------------------------------------------

(defun emskin-connect ()
  "Connect to the emskin IPC socket (auto-discovers path)."
  (interactive)
  ;; Clean up stale connection
  (when emskin--jsonrpc-conn
    (jsonrpc-shutdown emskin--jsonrpc-conn)
    (setq emskin--jsonrpc-conn nil))
  (let* ((path (emskin--ipc-path))
         (proc (condition-case err
                   (make-network-process
                    :name "emskin-ipc"
                    :family 'local
                    :service path
                    :coding 'binary)
                 (error
                  (message "emskin: failed to connect to %s: %s" path err)
                  nil))))
    (when proc
      (setq emskin--jsonrpc-conn
            (make-jsonrpc-process-connection
             :process proc
             :on-notification #'emskin--dispatch-notification
             :on-shutdown
             (lambda (_c)
               (message "emskin: IPC disconnected")
               (setq emskin--jsonrpc-conn nil))))
      (message "emskin: connecting to %s" path))))

(defun emskin--dispatch-notification (_conn method params)
  "Dispatch incoming JSON-RPC notification METHOD with PARAMS."
  (run-hook-with-args 'emskin--message-hook method params))

(provide 'emskin-ipc)
;;; emskin-ipc.el ends here
```

- [ ] **Step 2: Byte-compile to verify no warnings**

```bash
emacs -Q --batch -L elisp -f batch-byte-compile elisp/emskin-ipc.el 2>&1
```

Expected: zero warnings.

- [ ] **Step 3: Commit**

```bash
git add elisp/emskin-ipc.el
git commit -m "refactor(elisp): rewrite emskin-ipc.el with jsonrpc.el"
```

---

### Task 6: Dispatch adapt — hash-table → plist

**Files:**
- Modify: `elisp/emskin-app.el`
- Modify: `elisp/emskin-workspace.el`

The hook signature changes from `(msg : hash-table)` to `(method : symbol, params : plist)`.
- `method` = symbol (`connected`, `window_created` …) — jsonrpc.el calls `(intern method_string)` internally
- `params` = plist with keyword keys (`:window_id`, `:title` …) — from `json-parse-buffer` with `:object-type 'plist`

Replacement dispatch:

```elisp
;; emskin-app.el — adapt emskin--dispatch
(defun emskin--dispatch (method params)
  (pcase method
    ('connected
     (message "emskin: connected (version %s)" (plist-get params :version))
     (setq emskin--active-workspace-id 1)
     (emskin--map-frame-to-workspace (selected-frame) 1)
     (run-hooks 'emskin-connected-hook))
    ('window_created
     (emskin--on-window-created (plist-get params :window_id)
                                 (plist-get params :title)))
    ('window_destroyed
     (emskin--on-window-destroyed (plist-get params :window_id)))
    ('title_changed
     (emskin--on-title-changed (plist-get params :window_id)
                                (plist-get params :title)))
    ('focus_view
     (emskin--on-focus-view (plist-get params :window_id)
                             (plist-get params :view_id)))
    ('surface_size
     (let* ((w (plist-get params :width))
            (h (plist-get params :height))
            (frame-h (frame-pixel-height))
            (offset (or emskin--header-offset
                        (max 0 (- h frame-h)))))
       (setq emskin--header-offset offset)
       (message "emskin: surface=%sx%s bars=%dpx" w h offset)
       (dolist (frame (frame-list))
         (emskin--sync-frame frame))))
    ('x_wayland_ready
     nil)
    (_
     (message "emskin: unknown message method %s" method))))
```

And `emskin-workspace.el`:

```elisp
(defun emskin--handle-workspace-message (method params)
  (pcase method
    ('workspace_created
     (emskin--on-workspace-created (plist-get params :workspace_id)))
    ('workspace_switched
     (emskin--on-workspace-switched (plist-get params :workspace_id)))
    ('workspace_destroyed
     (emskin--on-workspace-destroyed (plist-get params :workspace_id)))))

(add-hook 'emskin--message-hook #'emskin--handle-workspace-message)
```

Also need to update all `emskin--send` call sites. The old signature was `(emskin--send '((type . "set_geometry") (window_id . ,id) ...))`. The new signature is `(emskin--send 'set_geometry '(:window_id ,id ...))`.

- [ ] **Step 1: Find and update all `emskin--send` calls**

```bash
rg -n 'emskin--send' elisp/
```

Replace each call:
- Old: `(emskin--send '((type . "set_geometry") (window_id . ,id) (x . ,x) ...))`
- New: `(emskin--send 'set_geometry '(:window_id ,id :x ,x ...))`

Mapping: first element `(type . "method_name")` → method symbol; remaining alist elements become plist key-value pairs.

- [ ] **Step 2: Rewrite emskin--dispatch in emskin-app.el**

Replace the `emskin--dispatch` function body (lines 156-191) with the pcase version using plist.

- [ ] **Step 3: Rewrite emskin--handle-workspace-message in emskin-workspace.el**

Replace the function body to use `pcase` and `plist-get`.

- [ ] **Step 4: Byte-compile both files**

```bash
emacs -Q --batch -L elisp -f batch-byte-compile elisp/emskin-app.el 2>&1
emacs -Q --batch -L elisp -f batch-byte-compile elisp/emskin-workspace.el 2>&1
```

Expected: zero warnings.

- [ ] **Step 5: Commit**

```bash
git add elisp/emskin-app.el elisp/emskin-workspace.el
git commit -m "refactor(elisp): adapt dispatch from hash-table to plist"
```

---

### Task 7: Verify Elisp tests unchanged

**Files:**
- Check: `tests/elisp/emskin-app-tests.el`

The test file calls `emskin--on-window-created` and `emskin--on-window-destroyed` with positional args directly (not through `emskin--dispatch`), so the dispatch layer change doesn't affect them.

- [ ] **Step 1: Run existing tests to confirm no breakage**

```bash
emacs -Q --batch -L elisp -l tests/elisp/emskin-app-tests.el \
  --eval "(ert-run-tests-batch-and-exit)" 2>&1
```

Expected: all pass.

- [ ] **Step 2: If there are dispatch-level tests, update them**

Run this to check if any test exercises `emskin--dispatch` or `emskin--handle-workspace-message`:

```bash
grep -n 'dispatch\|message-hook' tests/elisp/*.el
```

If none found, skip this step.

---

### Task 8: Integration — build + verify + E2E

- [ ] **Step 1: Full check**

```bash
cargo fmt --all --check && cargo clippy --workspace -- -D warnings && cargo check --workspace 2>&1
```

Expected: zero warnings.

- [ ] **Step 2: Full test**

```bash
cargo build -p emez 2>&1 && cargo test -p emskin 2>&1
```

Expected: all existing E2E tests pass (smoke, capture, clipboard). The IPC protocol change is transparent to tests that spawn their own emez host — the `Connected` message still gets sent on accept, just in JSON-RPC format.

- [ ] **Step 3: Final commit**

```bash
# If there are uncommitted adjustments from the integration pass:
git commit -am "fixup: fmt/clippy/test adjustments"
```
