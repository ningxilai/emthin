# DBus 路由子系统设计

## 动机

emthin 内嵌入式应用（Chromium、Electron 等）依赖 DBus 会话总线注册 well-known name 和调用服务。当前两种模式各有不足：

- **默认模式**（无 `--dbus-isolated`）：嵌入式应用的 DBus 请求通过进程内 broker 透传到宿主会话总线，well-known name 与宿主实例冲突
- **隔离模式**（`--dbus-isolated`）：私有 `dbus-daemon` 提供独立命名空间，但全量隔离导致宿主服务（portal、NetworkManager 等）完全不可用

目标是提供一个**可路由的 DBus 代理**，按 `(destination, interface, method)` 粒度决定消息目标（隔离 daemon vs 宿主总线），并通过 Emacs IPC 暴露规则管理接口。

## 架构

```
┌─ emthin 主进程 ───────────────────────┐
│  DbusBridge (子进程管理器)             │
│  ├─ spawn/kill emthin-dbus-router      │
│  ├─ spawn/kill dbus-daemon(可选)       │
│  └─ IPC (JSON-RPC) 与 router 通信      │
│        │ add_rule / remove_rule        │
│        │ fcitx_event / ime_commit      │
└────────┬───────────────────────────────┘
         │ Unix socket IPC
         ▼
┌─ emthin-dbus-router (独立子进程) ──────┐
│                                        │
│  ┌────────────────────────────────────┐│
│  │ 监听 socket (bus.sock)             ││
│  │ 嵌入式 app 连接此 → zbus::Conn     ││
│  └──────────┬─────────────────────────┘│
│             │ 每连接一个 zbus Conn      │
│             ▼                          │
│  ┌────────────────────────────────────┐│
│  │ 路由引擎                           ││
│  │ match (destination, interface,     ││
│  │        method) → target            ││
│  │ 内置默认表 + IPC 可扩展            ││
│  └┬──────────────────────────────────┬┘│
│   │ isolated            host         │ │
│   ▼                    ▼             │ │
│  ┌────────┐    ┌──────────────┐      │ │
│  │zbus::Conn│  │zbus::Conn    │      │ │
│  │(私有     │  │(宿主总线)    │      │ │
│  │ daemon) │  │              │      │ │
│  └────────┘    └──────────────┘      │ │
│                                        │
│  ┌────────────────────────────────────┐│
│  │ IPC handler ←→ emthin 主进程       ││
│  │ 规则管理 + fcitx 事件              ││
│  └────────────────────────────────────┘│
└────────────────────────────────────────┘
```

## 组件

### emthin-dbus crate

| 变更 | 详情 |
|---|---|
| 移除 `wire/` | `zbus` 替代帧编解码 + SASL 握手 |
| 移除 `broker/` | `zbus::Connection` 替代连接状态机 |
| 移除 `proxy/cmsg.rs` | `zbus` unix-fd feature 替代 |
| 保留 `fcitx.rs` | fcitx5 方法分类、IC 分配、应答合成，无变化 |
| 保留 `proxy/signals.rs` | `build_preedit_chunks` 复用到 router |
| 新增 `router/` | 路由引擎 + 路由表 + IPC handler |
| 新增 binary target | `emthin-dbus-router` |

依赖：
- `zbus` (with `unix-fd` feature) — DBus 协议层
- `tokio` — router 子进程的异步运行时
- `serde` / `serde_json` — IPC 序列化
- `tracing` — 日志（同主进程风格）

### 路由引擎

```rust
struct RouteRule {
    id: String,
    priority: u32,
    destination: Option<GlobPattern>,  // well-known name, None = 通配
    interface: Option<String>,
    method: Option<String>,
    target: RouteTarget,
}

enum RouteTarget {
    Host,       // 宿主 DBus 会话总线
    Isolated,   // 私有 dbus-daemon（仅 --dbus-isolated 时有效）
    Deny,       // 拒绝并回复错误
}
```

匹配逻辑：
1. 按 `priority` 降序排列
2. 同优先级内，按匹配字段数降序（3字段 > 2字段 > 1字段）
3. `destination` 支持 glob 通配（`org.freedesktop.portal.*`）
4. 首次匹配即返回 target
5. 无匹配 → 使用默认 target

### 内建默认路由表

```json
[
  { "destination": "org.freedesktop.portal.*",        "target": "host" },
  { "destination": "org.freedesktop.NetworkManager",  "target": "host" },
  { "destination": "org.freedesktop.Notifications",    "target": "host" },
  { "destination": "org.freedesktop.Secrets",          "target": "host" }
]
```

默认 target（表中无匹配时）：
- 有 `--dbus-isolated` → `isolated`
- 无 `--dbus-isolated` → `host`

### IPC 协议（emthin 主进程 ↔ router）

JSON-RPC 2.0 over Unix socket，`Content-Length` 帧（同 Emacs IPC）。

**主进程 → router：**

| 方法 | 参数 | 说明 |
|---|---|---|
| `dbus_router_add_rule` | `{ rule: RouteRule }` | 添加路由规则 |
| `dbus_router_remove_rule` | `{ id: String }` | 删除规则 |
| `dbus_router_list_rules` | — | 列出全部规则 |
| `ime_commit` | `{ text: String }` | 转发 winit IME commit→DBus CommitString |
| `ime_preedit` | `{ text, cursor_begin, cursor_end }` | 转发 winit preedit→DBus UpdateFormattedPreedit |

**router → 主进程：**

| 方法 | 参数 | 说明 |
|---|---|---|
| `fcitx_event` | `{ ty, conn, ic_path, ... }` | 同现有 FcitxEvent 格式 (*) |
| `rule_added` | `{ id, rule }` | 确认规则已添加 |
| `rule_removed` | `{ id }` | 确认规则已删除 |

(*) FcitxEvent 的数据结构与现有 `emthin_dbus::FcitxEvent` 相同（FocusIn/FocusOut/CommitString/UpdateFormattedPreedit/SetCursorRect/Disconnected），序列化为 JSON 后通过 IPC 传递。

### Emacs 扩展接口

通过现有 Compositor→Emacs JSON-RPC IPC 透传：

| IPC 方向 | 方法 |
|---|---|
| Emacs → Compositor → Router | `dbus_router_add_rule` |
| Emacs → Compositor → Router | `dbus_router_remove_rule` |
| Emacs → Compositor → Router | `dbus_router_list_rules` |
| Router → Compositor → Emacs | `dbus_router_rule_applied` (通知) |

### emthin 主进程侧变更

`DbusBridge`（`state/dbus.rs`）重构：

```rust
pub struct DbusBridge {
    router_child: Option<Child>,
    router_ipc: Option<IpcClient>,   // JSON-RPC 客户端连接 router
    listen_path: Option<PathBuf>,
    session_dir: Option<PathBuf>,
    isolated_daemon: Option<Child>,
}
```

移除 `main.rs` 中的 `register_dbus_listen_source` / `handle_dbus_accept` / `register_dbus_connection` / `drop_dbus_connection`——这些完全由 router 子进程处理。

`tick.rs` 中的 `drain_fcitx_events` 改为通过 IPC 接收 fcitx event。

## 启动流程

### 默认模式（无 `--dbus-isolated`）

```
main() → DbusBridge::init()
  1. 创建 session dir
  2. spawn emthin-dbus-router --listen bus.sock --ipc ipc.sock
  3. 等待 router ready（轮询 ipc.sock）
  4. 连接 router IPC
```

Router 启动后：
1. 绑定 listen socket
2. 绑定 IPC socket
3. 连接宿主 session bus（`$DBUS_SESSION_BUS_ADDRESS`）
4. 进入事件循环（tokio）

### 隔离模式（`--dbus-isolated`）

```
main() → DbusBridge::init_isolated()
  1. 创建 session dir
  2. 写入最小化 session.conf（无 servicedir）
  3. spawn dbus-daemon --nofork --config-file=session.conf
  4. 等待 daemon 就绪（socket 文件出现）
  5. spawn emthin-dbus-router --listen bus.sock --ipc ipc.sock --upstream daemon.sock
  6. 连接 router IPC
```

## Fcitx5 事件流

```
嵌入式 app (Chrome)                      emthin-dbus-router              emthin 主进程
       │                                      │                              │
       │ DBus: FocusIn(/ic/1)                 │                              │
       ├─────────────────────────────────────►│                              │
       │                                      │ fcitx.rs: classify + reply  │
       │◄─────────────────────────────────────┤                              │
       │                                      │ IPC: fcitx_event(FocusIn)   │
       │                                      ├────────────────────────────►│
       │                                      │                              │ ImeBridge::on_fcitx_event
       │                                      │                              │ → set_ime_allowed(true)
       │ DBus: KeyEvent(key)                  │                              │
       ├─────────────────────────────────────►│                              │
       │                                      │ IPC: fcitx_event(KeyEvent)  │
       │                                      ├────────────────────────────►│
       │                                      │                              │ → 按键转发到 winit IME
       │                                      │                              │ winit → Ime::Preedit / Commit
       │                                      │ IPC: ime_commit(text)       │
       │                                      │◄────────────────────────────┤
       │                                      │ DBus: CommitString(text)    │
       │◄─────────────────────────────────────┤                              │
```

### 与现有架构的关键差异

1. **fcitx5 拦截在 router 子进程内完成**，不再需要主进程介入
2. **主进程只需接收 FcitxEvent 驱动 ImeBridge**，不再管理 DBus 连接生命周期
3. **IME commit/preedit 回调**：winit 投递 Ime 事件后，主进程通过 IPC 发给 router，router 作为 DBus signal 发送给嵌入式 app

## 移除 `wire/` 和 `broker/` 的副作用

| 项目 | 影响 |
|---|---|
| `emthin_dbus::Frame` 等类型不再公开 | 替换为 `zbus::Message` |
| `emthin_dbus::ConnectionState` 不再公开 | 路由子进程内部使用 zbus |
| `emthin_dbus::FcitxEvent` 保留 | 从 `proxy/mod.rs` 移到 `fcitx.rs`，IPC 序列化使用 |
| `emthin_dbus::ConnId` 移除 | 主进程不再跟踪 DBus 连接 ID，ImeBridge 改用 IC path 标识 |
| `emthin_dbus::DbusBroker` 整体移除 | 由 router 子进程替代 |
| `state/dbus.rs::DbusBridge` 不再引用 `DbusBroker` | 改为管理子进程 + IPC |

## 测试策略

1. **路由规则匹配** — 纯函数测试（无需 DBus 连接），验证 glob 匹配、优先级、默认回退
2. **Router 集成测试** — 用 socketpair 模拟客户端 → router → mock upstream，验证转发决策
3. **Fcitx5 拦截测试** — 复用现有 `Fcitx5MethodCall` 分类测试

## 非目标

- 不在 emthin 内实现 portal 服务（拒绝 portal stub）
- 不替代 xdg-dbus-proxy 的完整安全策略
- 不处理 DBus 系统总线
