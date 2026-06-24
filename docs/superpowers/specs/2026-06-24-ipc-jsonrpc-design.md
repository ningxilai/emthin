# JSON-RPC 2.0 IPC Protocol

**Date:** 2026-06-24
**Status:** Draft
**Drivers:** emskin → Emacs IPC 自定义协议 → JSON-RPC 2.0

## Motivation

当前 IPC 协议是自定义的 4 字节 LE 长度前缀 + 内部 Tagged Enum JSON。Elisp 侧
需要手动维护 framing 解析（160 行）。改用 JSON-RPC 2.0 后，Emacs 侧可用内置
`jsonrpc.el` 托管所有协议细节，显著减少维护压力。

## Constraints

- **零新增 Rust 依赖** — 不使用 jsonrpsee/tokio
- **calloop event loop 不变** — 不引入新线程
- **Unix socket 不变** — 发现机制（PID socket discovery）不变
- **Rust 侧 IncomingMessage/OutgoingMessage enum 语义不变** — dispatch.rs 不改
- **byte-compile-error-on-warn: 零警告**
- **cargo clippy -- -D warnings: 零警告**

## Architecture

```
Emacs (jsonrpc.el)                    emskin (calloop)
──────────────────                    ────────────────
jsonrpc-process-connection            IpcServer
  │   Content-Length: N\r\n\r\n         │
  │   ── jsonrpc-notify ──────────────→ │  connection.rs
  │                                     │    try_recv() parse Content-Length
  │                                     │    enqueue() write Content-Length
  │                                     │       → jsonrpc.rs
  │   ←── :on-notification ──────────── │         parse_incoming() →
  │                                     │         IncomingMessage → dispatch.rs
  │                                     │         OutgoingMessage → serialize_outgoing()
  │                                     │
  jsonrpc-request (未来)               │
  │   ←── id-matched response ──────── │  (JSON-RPC error/result)
```

### 传输层

4 字节 LE 长度前缀 → `Content-Length: N\r\n\r\n` header（跟 jsonrpc.el 一致）

```
旧: [0x2A 0x00 0x00 0x00]{"type":"set_geometry",...}
新: Content-Length: 42\r\n\r\n{"jsonrpc":"2.0","method":"set_geometry","params":{...}}
```

### JSON-RPC 信封

当前所有消息都是 notification（无 `id`，fire-and-forget）。

```json
{
  "jsonrpc": "2.0",
  "method": "window_created",
  "params": {
    "window_id": 42,
    "title": "foot"
  }
}
```

method 命名 = `IncomingMessage`/`OutgoingMessage` variant 的 snake_case
（与当前 `#[serde(rename_all = "snake_case")]` 一致）。

## 消息映射

### Emacs → compositor (notification)

| Method | Variant | Notes |
|--------|---------|-------|
| `set_geometry` | SetGeometry | |
| `close` | Close | |
| `set_visibility` | SetVisibility | |
| `prefix_done` | PrefixDone | 无 params |
| `prefix_clear` | PrefixClear | 无 params |
| `add_mirror` | AddMirror | |
| `update_mirror_geometry` | UpdateMirrorGeometry | |
| `remove_mirror` | RemoveMirror | |
| `promote_mirror` | PromoteMirror | |
| `set_focus` | SetFocus | |
| `switch_workspace` | SwitchWorkspace | |

### Compositor → Emacs (notification)

| Method | Variant | Notes |
|--------|---------|-------|
| `connected` | Connected | 握手，accept 后立即发送 |
| `window_created` | WindowCreated | |
| `window_destroyed` | WindowDestroyed | |
| `title_changed` | TitleChanged | |
| `surface_size` | SurfaceSize | |
| `focus_view` | FocusView | |
| `x_wayland_ready` | XWaylandReady | |
| `workspace_created` | WorkspaceCreated | |
| `workspace_switched` | WorkspaceSwitched | |
| `workspace_destroyed` | WorkspaceDestroyed | |

## Rust 端实现

### 文件变更

| File | Δ | Description |
|------|---|-------------|
| `ipc/connection.rs` | ~40 lines | `try_recv()` Content-Length parser 替换 4-byte LE；`enqueue()` 输出 Content-Length header |
| `ipc/jsonrpc.rs` | ~60 lines (new) | `parse_incoming(payload) → Result<IncomingMessage>` + `serialize_outgoing(msg) → Vec<u8>` |
| `ipc/messages.rs` | ~30 lines | 去掉 `#[serde(tag = "type")]` 和 `#[derive(Deserialize)]`；IncomingMessage 改为 impl 方法 `from_jsonrpc_params(method, &Value)` 手动提取字段；OutgoingMessage 去掉 `#[derive(Serialize)]`，由 jsonrpc.rs 做序列化 |
| `ipc/mod.rs` | +1 line | `pub mod jsonrpc;` |
| `ipc/dispatch.rs` | 0 | 不变 |

### connection.rs 改动

```rust
// try_recv
// 扫描 read_buf 找 \r\n\r\n
//   1. b"Content-Length: " 前缀匹配
//   2. 读数字直到 \r
//   3. 跳过 \n\r\n
//   4. 计算 payload_end = header_end + content_length
//   5. 够 → 返回 payload，不够 → Ok(None)

// enqueue
// write_buf.extend(b"Content-Length: N\r\n\r\n")
// write_buf.extend(json_bytes)
```

### jsonrpc.rs

```rust
/// 输入方向: Content-Length body → IncomingMessage
pub fn parse_incoming(payload: &[u8]) -> Result<IncomingMessage, Error> {
    let v: serde_json::Value = serde_json::from_slice(payload)?;
    let method = v["method"].as_str().ok_or(MissingMethod)?;
    let params = &v["params"];
    Ok(match method {
        "set_geometry" => IncomingMessage::SetGeometry(serde_json::from_value(params.clone())?),
        "close" => IncomingMessage::Close(serde_json::from_value(params.clone())?),
        "prefix_done" => IncomingMessage::PrefixDone,
        // ...
    })
}

/// 输出方向: OutgoingMessage → JSON-RPC notification body
pub fn serialize_outgoing(msg: &OutgoingMessage) -> Result<Vec<u8>, Error> {
    let (method, params) = match msg {
        OutgoingMessage::Connected { version } => ("connected", serde_json::json!({"version": version})),
        OutgoingMessage::WindowCreated { window_id, title } =>
            ("window_created", serde_json::json!({"window_id": window_id, "title": title})),
        // ...
    };
    let envelope = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    serde_json::to_vec(&envelope)
}
```

### messages.rs 改动

```rust
// 去掉 #[serde(tag = "type", rename_all = "snake_case")]
// 保留 #[derive(Debug, Deserialize)] IncomingMessage — 每个 variant 直接从 params 反序列化
// 保留 #[derive(Debug, Clone)] OutgoingMessage — 去掉 #[derive(Serialize)]

// Add: impl OutgoingMessage { fn method_name(&self) -> &'static str }
// 让 jsonrpc.rs serialize_outgoing 用
```

## Emacs 端实现

### emskin-ipc.el 重写

从 ~160 行缩至 ~40 行。

```elisp
;; 状态: 只保留 conn 引用
(defvar emskin--jsonrpc-conn nil
  "JSON-RPC connection object.")

(defvar emskin-ipc-path nil
  "Explicit IPC socket path.  When nil, auto-discovered via parent PID.")

(defvar emskin--message-hook nil
  "Hook run with (METHOD PARAMS) on each incoming notification.")

(defvar emskin-connected-hook nil
  "Hook run after the IPC connection is established.")

;; 发送: 纯函数式通知
(defun emskin--send (method params)
  "Send JSON-RPC notification METHOD with PARAMS."
  (when emskin--jsonrpc-conn
    (jsonrpc-notify emskin--jsonrpc-conn method params)))

(defun emskin--send-thunk (method params)
  "Return thunk that sends METHOD+PARAMS when called."
  (let ((conn emskin--jsonrpc-conn))
    (lambda ()
      (when conn (jsonrpc-notify conn method params)))))

;; 连接
(defun emskin-connect ()
  (interactive)
  (when emskin--jsonrpc-conn ...) ;; 清理旧连接
  (let* ((path (emskin--ipc-path))
         (proc (make-network-process
                :name "emskin-ipc"
                :family 'local
                :service path
                :coding 'binary)))
    (setq emskin--jsonrpc-conn
          (make-jsonrpc-process-connection
           :process proc
           :on-notification #'emskin--dispatch-notification
           :on-shutdown
           (lambda (_c)
             (message "emskin: IPC disconnected")
             (setq emskin--jsonrpc-conn nil))))
    (message "emskin: connecting to %s" path)))

;; notification dispatch → 现有 hook 系统
(defun emskin--dispatch-notification (_conn method params)
  (run-hook-with-args 'emskin--message-hook method params))
```

### Dispatch 适配

`emskin-app.el` 和 `emskin-workspace.el` 中的 handler 从 hash-table 取 field 改为 plist：

```elisp
;; 旧: (gethash "window_id" msg)
;; 新: (plist-get params :window_id)
```

## 测试策略

### Rust 侧

- `connection.rs` 已有单元测试 → 适配 Content-Length 格式
- `jsonrpc.rs` 新测试 → 解析 + 序列化 round-trip
- `messages.rs` 已有 `IncomingMessage` 反序列化测试 → 去掉或改为从 params 反序列化

### Elisp 侧

- `tests/elisp/emskin-app-tests.el` 中有 `emskin--on-window-destroyed` / `-on-window-created` 测试
- Dispatch 从 hash-table 改为 plist → 更新测试参数格式

## 过渡方案

单次提交，同时改 Rust 和 Elisp 侧——wire format 不兼容新旧版本。

1. 改 Rust: connection.rs → jsonrpc.rs → messages.rs
2. 改 Elisp: emskin-ipc.el → dispatch 适配
3. 更新测试
4. cargo clippy + byte-compile 验证
5. `cargo test -p emskin` 验证 IPC 测试

## 未来

- `set_focus` / `switch_workspace` 等操作可升级为 request（带 `id`），Emacs 侧收到 error response 并处理
- 外部调试工具可直接连 Unix socket 发送 JSON-RPC 消息
