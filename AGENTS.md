# Design Philosophy

**Elastic Emacs / Thin Kernel.** `emthin` inverts the typical compositor
architecture: the Rust side should remain a *small, stable kernel* providing
minimal Wayland/input/IPC primitives, while all layout policy, window
management decisions, keybindings, and application lifecycle live in **Elisp**,
where users can customize and extend without touching the compositor.

```
╔═══════════════════════════════════════════╗
║          Elisp (thick, flexible)          ║
║  layout  ·  placement  ·  keybinds       ║
║  app lifecycle  ·  frame tree            ║
║  window rules  ·  hooks                  ║
╚═══════════════════════════════════════════╝
                    │ IPC (f64 fractions, JSON-RPC)
                    ▼
╔═══════════════════════════════════════════╗
║        Rust kernel (small, stable)        ║
║  wayland protocol  ·  input dispatch     ║
║  pixel→fraction conversion  ·  ResizeGrab║
║  DBus broker  ·  clipboard bridge        ║
╚═══════════════════════════════════════════╝
```

This is a deliberate departure from conventional compositors (i3, river,
niri) where the C/Rust binary encodes all layout policy. Here, the compositor
speaks only in **f64 fractions** (0..=1 relative to the Emacs frame) —
geometry is *described*, not dictated. The Elisp side converts user intent
into fractions, and the kernel converts fractions into pixels.

**Consequences:**
- **`emthin-ipc.el` and `emthin-launch.el` are frozen** — no modifications.
  IPC transport (jsonrpc-process-connection) and process management are
  stable; all new features go into `emthin-app.el` or new Elisp libraries.
- **New IPC messages require Rust + Elisp changes** but should be rare — most
  features should be implementable with existing primitives (`set_geometry`,
  `close`, `set_visibility`, `set_focus`, `set_focus_view`, `switch_workspace`).
- **Layout is Elisp code**, not kernel state. The kernel does not track
  tiling, stacking, or placement policies.

---

# emthin workspace

Cargo workspace, three crates:

```
crates/
├── emthin/            # compositor binary, IPC, handlers/, tests/
├── emthin-clipboard/  # smithay-free host clipboard proxy (data-control / wl_data_device / X11)
└── emthin-dbus/       # DBus fcitx5 frontend for IME
elisp/                 # Emacs-side client, embedded via include_dir!
```

```
emthin      ──→  emthin-clipboard
       └──→  emthin-dbus
```

- `emthin-clipboard` **cannot** `use smithay` — it's a self-contained
  host clipboard proxy usable by any nested Wayland compositor. The
  smithay-aware glue (SelectionTarget ↔ SelectionKind mapping, XWM
  replay, async pipe drain for X11) lives in `emthin/src/clipboard_bridge.rs`.

## Invariants (every session)

1. **Compositor is self-adaptive via layer-shell.** Emacs's geometry is
   `EmthinState::usable_area() = LayerMap::non_exclusive_zone()`. Any
   layer-shell client declaring `exclusive_zone` shrinks it and
   `relayout_emacs()` pushes the new size.
2. **`crates/emthin/Cargo.toml` keeps literal `version`/`edition`/… values**
   because cargo-aur 0.x doesn't support `version.workspace = true`. Both
   this and root `[workspace.package].version` must bump together
   (`cargo release` handles both via `release.toml`).

## Testing

Two integration tests for xwayland-satellite helpers:

```
cargo test -p emthin
```

Test files:
- `tests/xwayland_satellite.rs` — pure pieces (socket pre-binding, spawn-command construction)
- `tests/xwayland_satellite_watch.rs` — calloop watch integration

### Test gotchas

- `IpcServer::send` drains its write buffer synchronously on EAGAIN (temporarily switches fd to blocking). calloop IPC source is READ-only.
- xclip: (1) default `-loops=0` means xclip **never exits** — spawn and don't wait. (2) Use `Stdio::null()` for stderr.
- calloop 0.14 `LoopSignal::stop()` only sets a flag — must call `stop(); wakeup();` as a pair.
- `std::process::Child::kill()` sends SIGKILL — use `graceful_kill` (SIGTERM + 1.5s wait + SIGKILL fallback).
- X socket/lock cleanup is owned by `Unlink` RAII guards in `xwayland_satellite::sockets::X11Sockets`.
- Diagnose orphan satellite: `pgrep -af xwayland-satellite`; orphan Xwayland: `ps -eo pid,cmd | grep 'Xwayland :' | grep -vE 'Xwayland :[01] '`; residual X sockets: `ls /tmp/.X11-unix/`.

## See also

- `CONTRIBUTING.md` — setup, local checks, PR flow for outside contributors.

---

# emthin — Nested Wayland Compositor for Emacs

## Build

- `cargo check` / `cargo clippy -- -D warnings` / `cargo fmt`
- smithay: forked at `emskin/smithay` branch `emskin-patches`. Upstream: `Smithay/smithay`
- smithay patches: `backend/winit/mod.rs` (expose `WinitEvent::Ime`, 8-bit pixel format priority), `text_input/text_input_handle.rs` (remove `has_instance()` guard, add `cursor_rectangle` accessor), `selection/seat_data.rs` (fix GTK3 clipboard on focus change)

## Architecture

- Nested Wayland compositor using smithay, hosting Emacs inside a winit window
- First toplevel = Emacs (fullscreen), subsequent toplevels = **arbitrary embedded programs** (any Wayland or XWayland client) managed by AppManager. Not limited to EAF — any GTK/Qt/Electron/X11 app can be embedded as a child window whose geometry is controlled by Emacs via IPC.
- IPC protocol: JSON-RPC 2.0 over Unix socket (`Content-Length: N\r\n\r\n` framing). All messages are notifications (no `id`). Emacs→compositor: `set_geometry`, `close`, `set_visibility`, `prefix_done`, `prefix_clear`, `set_focus`, `add_mirror`, `update_mirror_geometry`, `remove_mirror`, `promote_mirror`, `switch_workspace`. Compositor→Emacs: `connected`, `surface_size`, `window_created`, `window_destroyed`, `title_changed`, `focus_view`, `xwayland_ready`, `workspace_created`, `workspace_switched`, `workspace_destroyed`.
- Elisp client: split across `elisp/emthin.el` (entry + shared state), `emthin-ipc.el` (`jsonrpc-process-connection` + notification dispatch + `emthin-connected-hook`), `emthin-app.el` (app lifecycle + geometry + mirrors + dispatch), `emthin-workspace.el` (workspace CRUD + frame mapping). Auto-connects via parent PID socket discovery. All files are embedded into the binary via `include_dir!` and extracted at runtime in standalone mode. Elisp dispatch uses `pcase` on method symbol (not hash-table lookup).
- Mirror system: same embedded program displays in multiple Emacs windows. Source = first window (real surface), mirrors = subsequent windows (TextureRenderElement from same GPU texture). Elisp tracks source/mirror in `emthin--mirror-table`
- Keyboard input: compositor detects Emacs prefix keys (C-x, C-c, M-x) and clipboard shortcuts (M-w, C-w, C-y) via `input_intercept` in `input.rs`. Three-way dispatch based on focus and key:
  - **Prefix key + Emacs focus** → enter prefix state, forward key to Emacs for the chord
  - **Prefix key + embedded app focus** → redirect focus to Emacs, forward original key
  - **M-w (Copy) + embedded app focus** → read PRIMARY selection via async pipe, cache data, set compositor-owned CLIPBOARD selection (no key injection; focus stays on the app)
  - **C-w/C-y (Cut/Paste) + embedded app focus** → suppress original, synthesise Ctrl+X/V injection on the app (focus stays on the app)
  `prefix_done` IPC restores focus. `set_focus` IPC for explicit focus control. Prefix state: `Option<Option<WlSurface>>` (outer None = inactive)
- Focus: `SeatHandler::KeyboardFocus = KeyboardFocusTarget` enum (Window/Layer/Popup) in `focus.rs` — lets smithay's per-variant `KeyboardTarget` impl handle protocol specifics. Single-policy helper `EmthinState::auto_focus_new_window(window, window_id)` is the one entry `xdg_shell::new_toplevel` calls — respects `prefix_saved_focus`. On window destroy, fallback to `emacs_focus_target()` if `current_focus().is_none()`; Emacs's buffer MRU drives further recovery via `set_focus` IPC. X clients arrive through xwayland-satellite and present as ordinary Wayland clients, so there is no dedicated X11 focus plumbing in emthin itself — satellite handles ICCCM `SetInputFocus` / `WM_TAKE_FOCUS` and EWMH focus-state bits internally before translating to `wl_keyboard.enter`
- Embedded toplevel configure: `ipc_set_geometry` sets all four `TiledLeft/Right/Top/Bottom` states so terminal emulators (foot) fill the exact configured size with padding instead of rounding to cell boundaries
- Window destroy (Elisp): `emthin--on-window-destroyed` does `delete-window` (if multi-window) then `kill-buffer`, then sends `set_focus` for `(window-buffer (selected-window))`. Use `window-buffer (selected-window)` not `current-buffer` after `kill-buffer` — the latter is unreliable
- IME input (two paths feeding the same winit IME chain): **text_input_v3** bridges host IME to Wayland-native clients that bind `zwp_text_input_v3` (Chrome with `--enable-wayland-ime`) — handled in `state/ime.rs::ImeBridge`. **DBus fcitx5 frontend** impersonates fcitx5 over the session bus for embedded clients using `GTK_IM_MODULE=fcitx` (WeChat, Electron, pgtk Emacs) — handled in `emthin_dbus::proxy` + `emthin-dbus/src/fcitx/`. Both drive the same `winit.set_ime_allowed(true)` + `set_ime_cursor_area` on the winit window so the host fcitx5 sees a single IC anchored to emthin's surface; the inline preedit / commit path differs by protocol. Smithay patches required: expose `WinitEvent::Ime`, remove `has_instance()` guard in text_input dispatch, add `cursor_rectangle()` accessor
- `AppWindow::wl_surface()` returns the toplevel's `WlSurface`.
- XWayland: emthin does not embed its own X server. `xwayland-satellite` runs as a dedicated external process managed by `crates/emthin/src/xwayland_satellite/` with a niri-style on-demand supervisor: emthin pre-binds `/tmp/.X11-unix/X<N>` and the Linux abstract socket, then `arm()`s calloop `Generic` sources on both fds; the first X client connect triggers `on_socket_connect()` which spawns the satellite in a dedicated thread, and `ToMain::Rearm` via a `calloop::channel` re-installs the watch after a crash. From emthin's viewpoint the satellite is just another Wayland client — X-specific focus, cursor, clipboard, and fullscreen policy all live inside satellite, not in emthin.
- Elisp: per-app buffers must use `generate-new-buffer` (not `get-buffer-create`) — two same-titled windows (two xterms, two firefox) would otherwise share a buffer and `setq-local emthin--window-id` would clobber the earlier window's id. `rename-buffer ... t` already handles title_changed collisions
- Reference compositors when unclear about focus/input semantics: anvil (`anvil/src/focus.rs` inside the smithay checkout) for the `KeyboardFocusTarget` / `PointerFocusTarget` shape; niri (`~/study/rust/source/niri/src/utils/xwayland/`) for the on-demand satellite supervisor pattern this module is ported from. Cite concrete file:line before asserting behavior.
- Elisp auto-connect: `emthin-maybe-auto-connect` must NOT be gated on `(featurep 'pgtk)` — gtk3 Emacs running under xwayland-satellite also needs IPC. Gate only on socket file existence.
- `EmthinState::output_fullscreen_geo()` — shared helper for output→mode→scale→logical fullscreen geometry, used by resize logic.
- grabs/ directory is placeholder code for future move/resize support
- Workspace model: each Emacs frame = one workspace. Active workspace's state lives in `self.workspace.active_space` + `self.emacs.surface()`; inactive workspaces stored in `self.workspace.inactive: HashMap<u64, Workspace>`. Switching = swap (std::mem::take). `sync-frame` only processes the hook-triggering frame when its workspace-id matches `active-workspace-id` — eliminates race conditions during workspace switches
- App migration: Emacs drives migration via IPC — compositor does NOT auto-migrate on workspace switch (doesn't know which apps are in which Emacs frame). `ipc_set_geometry` calls `migrate_app_to_active()` which resets geometry/pending_geometry/pending_since to None so the next set_geometry maps immediately (otherwise pending path deadlocks: app needs frame callbacks but isn't in any Space)
- Elisp workspace switch: `emthin--resync-workspace` clears change detection then delegates to `sync-frame`. `other-frame` advice sends `switch_workspace` IPC but does NOT resync — resync is handled by `on-workspace-switched` when the compositor confirms (avoids stale `active-workspace-id`). `WorkspaceSwitched` IPC is sent inside `switch_workspace()` BEFORE `keyboard.set_focus` so Emacs updates state before GTK focus-change hooks fire
- Mirror vs migration: mirrors are for simultaneous visibility (future side-by-side); migration is for workspace switching (one visible at a time). `sync-frame` only processes the active workspace's frame, so invisible workspace frames don't trigger mirror creation
- Child frame detection: pgtk child frames (posframe, company-posframe) also create xdg_toplevel from same Wayland client. Deferred to idle callback via `pending_emacs_toplevels` — check `ToplevelSurface::parent()` after dispatch_clients (GTK batches get_toplevel + set_parent in same Wayland message). Parent present = child frame (stays in current space); absent = real frame (creates workspace)
- IPC Y translation: `ipc_set_geometry` / `ipc_add_mirror` / `ipc_update_mirror_geometry` add `emacs_geometry().loc.y` (the current non-exclusive-zone origin) to incoming Emacs-relative `y`.
- ext-workspace-v1 protocol: `protocols/workspace.rs` — diff-based refresh model, action queue for client requests. Compositor is the single source of truth; IPC and protocol operate on same workspace state
- IPC extensions: Emacs→compositor: `switch_workspace`. Compositor→Emacs: `workspace_created`, `workspace_switched`, `workspace_destroyed`
- EmthinState sub-structs (all under `crates/emthin/src/state/`, re-exported at crate root for import convenience):
  - `wl: WaylandState` — 16 smithay protocol fields (compositor_state, xdg_shell_state, seat_state, …) in `state/mod.rs`
  - `workspace: WorkspaceState` — active Space + inactive HashMap<u64, Workspace> + workspace-id counters + ext-workspace-v1 protocol handle; `state/workspace.rs`
  - `apps: AppManager` — embedded-app catalog + mirror table + pending-geometry timeouts; `state/apps.rs`
  - `emacs: EmacsState` — main Emacs `surface`, spawned `child`, `title`/`app_id` forwarding, `detect`/`initial_size_settled` latches, and `pending_fullscreen`/`pending_maximize` mailboxes. Exposes composite predicates `should_claim_main()` and `main_died()` that encode the "first toplevel is Emacs" heuristic. `state/emacs.rs`
  - `xwayland: XwaylandState` — `:N` display cache, `XwlsIntegration` supervisor for on-demand `xwayland-satellite`, and the `--command` deferred-spawn mailbox; `state/xwayland.rs`
  - `cursor: CursorState` — current `CursorImageStatus` + dirty flag + last raw pointer location for zwp_relative_pointer_v1 delta synthesis; `state/cursor.rs`
  - `ime: ImeBridge` — text_input_v3 global + focused_surface + deferred `ime_enabled`; `state/ime.rs`
  - `focus: FocusState` — three saved-focus slots (`prefix_saved_focus`, `layer_saved_focus`, `host_saved_focus`). `reset_on_workspace_switch()` zeroes all three; call it alongside `ime`/`cursor`/`focus` resets on workspace swap.
  - `selection: SelectionState` — clipboard backend handle + origin tags.
  - Handlers impl on `EmthinState` and access via `self.<substruct>.<field>` (e.g. `self.wl.compositor_state`, `self.emacs.surface()`).
- ClipboardBackend trait: lives in the sibling `emthin-clipboard` crate (zero smithay deps). Three backends — `ClipboardProxy` (data-control, `data_control.rs`), `WlDataDeviceProxy` (wl_data_device fallback, `wl_data_device.rs`), `X11ClipboardProxy` (`x11.rs`) — all impl the trait. State field: `Option<Box<dyn emthin_clipboard::ClipboardBackend>>`. Constructed via `emthin_clipboard::init(&[BackendHint::DataControl, BackendHint::wl_data_device(ptr)?, BackendHint::X11])` in `main.rs`, which walks the fallback chain and returns the first backend that handshakes. `BackendHint::wl_data_device` is the only unsafe constructor (takes a foreign `*mut wl_display`, winit's in our case). Event loop registration branches on `ClipboardBackend::driver()`: `Driver::OwnedFd` backends (data-control, X11) register their fd with calloop in `main::register_clipboard_source`; `Driver::Piggyback` (wl_data_device) has no owned fd, so `tick.rs` drains via `dispatch()` every tick.
- `emthin-clipboard` uses independent `SelectionKind { Clipboard, Primary }` to stay smithay-free. The bridge layer (`crates/emthin/src/clipboard_bridge.rs`) converts to/from smithay's `SelectionTarget` via two extension traits (`SelectionTargetExt::to_kind` + `SelectionKindExt::to_target`) — orphan rules forbid `impl From` here since all three types are foreign to emthin, so extension traits (local to emthin) are the idiomatic escape hatch. Callsites in `handlers/mod.rs` `use crate::clipboard_bridge::SelectionTargetExt;` and call `target.to_kind()`.
- X11-only async completion: the `X11ClipboardProxy` backend, on host paste requests, emits `HostSendRequest { completion: Some(AsyncCompletion { id, read_fd }) }` so the bridge layer can drain the pipe via calloop and call back into `ClipboardBackend::complete_outgoing(id, data)` for the backend to send `SelectionNotify`. Wayland backends emit `completion: None` — the default `complete_outgoing` impl is a no-op. Error path in the pipe reader still calls `complete_outgoing` with whatever was drained so `outgoing_requests[id]` never leaks and the X11 requestor always gets a reply.
- IpcRect: shared `{x, y, w, h}` struct with `#[serde(flatten)]` — replaces repeated bare fields in SetGeometry, AddMirror, UpdateMirrorGeometry
- Module layout: `lib.rs` (library entry), `tick.rs` (event loop body), `ipc/connection.rs` (Content-Length framing), `ipc/jsonrpc.rs` (JSON-RPC 2.0 envelope), `ipc/dispatch.rs` (IPC message handlers → state methods), `ipc/messages.rs` (manual `from_jsonrpc`/`method_name`/`into_params_value`, no serde derives), `clipboard_bridge.rs` (smithay glue for the `emthin-clipboard` crate), `mirror_render.rs` (mirror texture rendering)

## Key Gotchas

- smithay winit backend defaults to 10-10-10-2 pixel format (2-bit alpha) — breaks GTK semi-transparent UI. Fixed by prioritizing 8-bit in smithay's `backend/winit/mod.rs`
- GPU readback on winit backend: `ExportMem::copy_framebuffer` must run inside the `backend.bind()` block while the EGL surface is still current, but `map_texture` must run AFTER `backend.submit()` — `map_texture` internally `make_current`s without a draw surface, detaching the winit EGL surface and breaking the next `eglSwapBuffers` with `BAD_SURFACE`. See `capture.rs` + `recording.rs` for the split-across-submit pattern
- For capture / mirror paths, trust `backend.window_size()` for physical framebuffer dimensions, NOT `output.current_mode().size` — on fractional-scale resizes (e.g. KDE 1.624×) winit resizes its EGL surface immediately but the output's mode is re-synced on the next render tick; reading mode.size at capture time gives a lagged size and produces stride-mismatched pixel data
- winit `scale_factor()` returns 1.0 at init time; real scale arrives later via `ScaleFactorChanged` → `WinitEvent::Resized { scale_factor }`
- Use `Scale::Fractional(scale_factor)` not `Scale::Integer(ceil)` to match host compositor's actual DPI
- `render_scale` in `render_output()` should be 1.0 (smallvil pattern); smithay handles client buffer_scale internally
- `Transform::Flipped180` is required for correct orientation with the winit EGL backend
- Use smithay's type-safe geometry: `size.to_f64().to_logical(scale).to_i32_round()` instead of manual arithmetic
- GTK3 Emacs does NOT support xdg-decoration protocol — setting `Fullscreen` state on the toplevel is what actually hides CSD titlebar/borders
- GTK4/GTK3 will send `unmaximize_request`/`unfullscreen_request` immediately on connect if those states are set in initial configure — must ignore these for single-window compositor
- Host keyboard layout: smithay winit backend does NOT expose the host's keymap. Use `wayland-client` to separately connect, receive `wl_keyboard.keymap`, then `KeyboardHandle::set_keymap_from_string()` — env vars (`XKB_DEFAULT_*`) are unreliable on KDE Wayland
- pgtk Emacs: `frame-geometry` returns 0 for `menu-bar-size` (GTK external menu-bar architectural limitation, not a bug). Compute exact bar height via compositor IPC: `offset = surface_height - frame-pixel-height`
- `window-pixel-edges` is relative to native frame (excludes external menu-bar/toolbar); `window-body-pixel-edges` bottom = top of mode-line
- embedded app windows must be mapped to space at 1×1 in `new_toplevel` (otherwise on_commit and initial configure don't fire); actual size arrives via `set_geometry` IPC
- Host resize must only resize the Emacs surface; embedded app window sizes are controlled by Emacs via IPC
- `Space::map_element` has a hidden re-stack side effect: even with `activate=false` it internally removes + re-appends to `elements.len()`, pushing the element to the top. Emacs is the fullscreen host and must stay at the bottom of `Space` so embedded app toplevels render above it — follow every `map_element(<emacs>, ...)` with `space.lower_element(&window)`. Sites: `state::resize_emacs_in_space`, `handlers::xdg_shell::new_toplevel` (both Emacs branches), `tick::process_pending_toplevels`. Symptom of regression: app goes fully white on host-window resize, recovers only after `set_visibility(false)+(true)` (Emacs covering the app from above).
- Debugging "white area" in render output: temporarily flip `clear_color` in `winit.rs::render_frame` to magenta `[1.0, 0.0, 1.0, 1.0]`. Area still white → some client (or Emacs from above, see previous bullet) drew white; area turns magenta → no surface is mapped / covered there.
- Mirror rendering: `TextureRenderElement` position is Physical coords — must use `output.current_scale().fractional_scale()` for logical→physical conversion, NOT hardcode 1.0
- Mirror rendering must walk the full `wl_subsurface` tree via `with_surface_tree_downward` — GTK/Firefox paint content onto subsurface children, so reading only the root surface yields an empty mirror
- Mirror rendering: call `import_surface_tree` once per layer, then walk each layer's subsurface tree *once* (not per mirror) and scale the collected snapshots — avoids O(mirrors × tree) traversals in the render hot path
- Mirror element Id must be `Id::from_wayland_resource(surface).namespaced(view_id as usize)` — same surface in different mirrors needs distinct Ids or the damage tracker collapses them. `render_elements_from_surface_tree` cannot replace the manual walk because its Id is hardcoded to `from_wayland_resource(surface)` with no namespace hook
- Mirror rendering must subtract `window.geometry().loc` (and `popup.geometry().loc` for popups) from the render origin — GTK/Chrome put CSD shadow padding in the buffer and use `xdg_surface.set_window_geometry` to mark where the visible window actually starts. Smithay's `Space::render_location()` does `space_loc - element.geometry().loc` automatically; custom mirror paths must match or visible content gets pushed inward by the shadow amount. Precompute this into `SurfaceLayer::render_offset` (popup offset minus geometry offset) so per-layer walks don't redo the math
- Mirror rendering: `TextureRenderElement` needs `buffer_scale`, `buffer_transform`, and viewport `src` from `RendererSurfaceState` — otherwise size is wrong under fractional scaling
- Mirror input: `surface_under()` must check mirrors BEFORE space — Emacs is fullscreen and `element_under()` always hits it first, blocking mirror detection
- Mirror input: pointer `under_position` for mirrors needs offset compensation (`pos - mapped_pos`) so smithay computes correct surface-local coords
- Mirror input: `surface_under()` for mirrors must compensate `window.geometry().loc` — same CSD shadow offset that the space path handles via `render_location = space_loc - geometry.loc`. Add `wg` to `local` point and subtract `wg` from `surface_global` in the return mapping, otherwise cursor hits shadow area instead of visible content
- Mirror scaling: aspect-fit with top-left alignment; coordinate mapping in `mirror_under` uses `rel.downscale(ratio)` to map mirror→source; `AppManager::aspect_fit_ratio()` returns None for zero-size to prevent NaN
- `render_output`'s second type param is the custom_elements type (not space element type); `render_scale` (value 1.0) is actually the `alpha` parameter
- `render_elements!` macro cannot parse associated-type bounds (`Renderer<TextureId = GlesTexture>`) — define a blanket helper trait as workaround. The `CustomElement` enum + `EmthinRenderer` trait live in `crate::element`.
- Elisp `defcustom` with `:set` that references later-defined vars: use `:initialize #'custom-initialize-default` + `bound-and-true-p` to avoid void-variable at load time
- IME: all text_input_v3 logic lives in `ime.rs::ImeBridge`. Three smithay-imposed constraints drive that design: (1) registering `TextInputManagerState` causes fcitx5-gtk to switch from DBus to text_input_v3, so `set_ime_allowed` must be toggled per-focused-client (only when the client has bound text_input_v3, probed via `with_focused_text_input`) — see `ImeBridge::on_focus_changed`; (2) smithay's keyboard.rs gates `text_input.enter()/leave()` behind `input_method.has_instance()` which is always false here, so enter/leave must be called manually with a temporary focus swap to send `leave` to the correct old client — same function; (3) `focus_changed` cannot access the winit backend, so the `set_ime_allowed` decision is stored in `ImeBridge::ime_enabled` and drained by `apply_pending_state` via `take_ime_enabled()` (same deferred pattern as `pending_fullscreen`/`pending_maximize`)
- IME via DBus fcitx5 frontend (B1, for embedded clients with `GTK_IM_MODULE=fcitx`): `DbusBridge` spawns an in-process `DbusBroker` in `state/dbus.rs`; broker binds `$XDG_RUNTIME_DIR/emthin-dbus-<pid>/bus.sock` and injects it as `DBUS_SESSION_BUS_ADDRESS` on every child spawn. The broker intercepts method_calls on `org.fcitx.Fcitx.InputMethod1` / `InputContext1` (classifier + reply synthesizer live in `emthin-dbus::fcitx`), allocating fake IC paths and emitting typed `FcitxEvent`s. Every tick, `tick::drain_fcitx_events` hands those events to `ImeBridge::on_fcitx_event`, which shares `ime_enabled` / `pending_cursor_area` with the text_input_v3 path — so the winit window ends up with one active IC regardless of which protocol drove it. When winit delivers `Ime::Preedit` / `Commit` back, `winit.rs::WinitEvent::Ime` forwards to the active IC as DBus `UpdateFormattedPreedit` / `CommitString` signals via `DbusBroker::emit_preedit` / `emit_commit_string` before also calling `on_host_ime_event` (which feeds the text_input_v3 path for any Wayland-native client). Signal `sender` is critical: DBus clients' match rules filter by the well-known's resolved unique name (`:N.M`), so the broker learns the real fcitx5 unique name from the `GetNameOwner` reply on the bus→client direction (authoritative) with a fallback that captures `destination` off intercepted method_calls; `NameOwnerChanged(sss)` signals refresh the cache when real fcitx5 restarts.
- DBus isolated mode (`--dbus-isolated`): `DbusBridge::init_isolated()` spawns a private `dbus-daemon` child as the broker's upstream instead of the host session bus. Daemon uses a minimal session.conf (no `<standard_session_servicedirs/>`) so uninhabited well-known names fail fast with `NameHasNoOwner` instead of triggering 25s timeouts via host `.service` files with `SystemdService=` directives that can't deliver into the isolated bus. `PR_SET_PDEATHSIG(SIGTERM)` in the daemon's `pre_exec` keeps it from outliving emthin on hard-kill paths.
- IME origin translation: client-reported caret rects are in the client's own surface-local frame. Emacs main surface IS the emthin winit window, so its origin is `(0, 0)`. Embedded apps use `element_location - geometry().loc` (buffer top-left in emthin-space, backing out CSD shadow padding).
- IME has ONE owner at a time: `enum ImeOwner { None, Tip { surface }, Dbus { conn, ic_path, origin } }` in `state/ime.rs::ImeBridge`. Decision is `desired_ime_allowed = !prefix_active && match owner { None => false, Tip => true, Dbus => cursor_is_real }`.
- text_input_v3 IME ordering: `set_ime_allowed(true)` MUST be called before `set_ime_cursor_area`. Per spec, `enable` resets text_input state to defaults.
- `TextInputHandle::cursor_rectangle()` is per-seat, not per-surface — value persists across client focus changes. `ImeBridge` keeps `cursor_cache: HashMap<CursorCacheKey, Rectangle>` of last-seen-fresh per owner.
- Focus override stack: `state::FocusState` uses a typed `enum FocusOverride { Prefix, Layer, Host }` + `enter`/`exit`/`is_active` API.
- Prefix chord cleanup uses TWO IPC messages: `prefix_done` (buffer-local in embedded app buffers — restores focus to the embedded app + clears IME prefix gate) and `prefix_clear` (global `post-command-hook` — clears IME prefix gate only, no focus change).
- Elisp: use `window-body-pixel-edges` for embedded app geometry (excludes fringes/margins/header-line/mode-line). Set buffer-local `left-fringe-width`, `right-fringe-width`, `left-margin-width`, `right-margin-width` to 0 and `cursor-type` to nil for embedded app buffers
- Elisp: `set-window-scroll-bars` is non-persistent across buffer switches — re-apply in `emthin--sync-frame` unconditionally for embedded app windows
- Popup input: clicking a popup surface must NOT change keyboard focus if the popup belongs to the same Wayland client as the current focus
- Popup input: browsers (Firefox, Chrome) may open menus as `xdg_popup` WITHOUT requesting `xdg_popup.grab` — the compositor must handle ungrabbed popups via the normal pointer focus path (no `PopupPointerGrab`)
- Clipboard under satellite: X clients reach emthin as ordinary Wayland clients through `xwayland-satellite`'s internal translator. Their selections arrive on `wl_data_device` just like pgtk Emacs, so `SelectionHandler::new_selection` is the single entry point.
- Cursor under satellite: `xwayland-satellite` forwards X11 cursor changes to emthin via `wp_cursor_shape_v1` / `wl_pointer.set_cursor` like any other Wayland client.
- Layer shell (wlr-layer-shell): uses smithay's `LayerMap` + `DesktopLayerSurface` (not manual Vec).
- Layer shell keyboard focus timing: `new_layer_surface` fires on `get_layer_surface` (BEFORE initial commit) — must defer focus to compositor commit handler.
- Layer shell non-exclusive zone changes trigger `EmthinState::relayout_emacs()` — only when `map.non_exclusive_zone()` differs before vs. after arrange.
- Pointer constraints activation: smithay does NOT auto-activate constraints — must call `PointerConstraintRef::activate` both in `new_constraint` AND on pointer-enter.
- Relative pointer delta synthesis: `CursorState::consume_raw_location(new_abs)` holds the last host-reported absolute and returns `new_abs - previous`.
- Pointer motion under constraint: when Locked, skip `pointer.motion()` entirely; when Confined, check position stays within region.
- Buffer-space coordinates use `Point<i32, Buffer>` / `Size<i32, Buffer>` smithay markers.
- Release workflow: `crates/emthin/Cargo.toml` version MUST equal the git tag minus the `v` prefix.
- AUR publish action pinned to `KSXGitHub/github-actions-deploy-aur@v4.1.2` or newer.
- Verifying AUR state: use `git clone https://aur.archlinux.org/<pkg>.git` to read the actual repo head.
- `SelectionOrigin` has two variants: `Wayland` / `Host`.
- External host sync goes through whichever `emthin-clipboard` backend `init()` picked.
- `EmthinState::usable_area()` returns the layer-shell non-exclusive zone.
- `emthin--last-focused-wid` must be reset to `'unset` on workspace switch.
- `other-frame` (C-x 5 o): advised `:around` to send `switch_workspace` IPC before calling original.
- New Emacs frame fullscreen: must send configure with `Fullscreen` state + output size in `new_toplevel`.
- `set-window-scroll-bars`/`set-window-fringes`/`set-window-margins` unconditionally reset to 0 in `sync-frame` for embedded app windows.
- Emacs 31+ `(defvar x)` without initvalue does NOT create a global binding (Emacs 32 `eval.c:1000-1005`: "Simple (defvar <var>) should not count as a definition at all.") — always write `(defvar x nil)` to avoid `Qunbound` from `buffer-local-value`.
- Elisp error isolation: every boundary point (IPC dispatch, frame/buffer/window hooks, post-command, kill-buffer, timers) wrapped in `condition-case` with error logging. `emthin--sync-frame` also uses `unwind-protect` to ensure `emthin--next-view-id` always saves.
- Elisp code style: direct imperative with cl-lib (no deferred thunks). All `*-thunks` factories removed in favor of straight-line blocks.
- Clipboard startup guard: use `!self.ipc.is_connected()` instead of per-target bool flags.
- Wayland child processes must have `WAYLAND_DISPLAY` explicitly set to `state.socket_name`.
- Workspace switch must reset: `focus` (via `FocusState::reset_on_workspace_switch`), `ime` (via `ImeBridge::reset_on_workspace_switch`), `cursor_status`, pointer focus.
- Wire-format quirk: `OutgoingMessage::method_name()` maps `XWaylandReady → "x_wayland_ready"` (manual snake_case; no serde derives).
- Layer shell destroy: only reclaim focus if `keyboard.current_focus() == destroyed surface`.

## Wayland Protocols Implemented

- xdg_shell (toplevel, popup)
- xdg-decoration (force ServerSide — no decorations drawn). xdg_activation_v1 is implemented **client-side only** (`main.rs::activate_main_surface_if_env_token` reads `XDG_ACTIVATION_TOKEN` / `DESKTOP_STARTUP_ID` from env and calls `activate(token, main_surface)` on the host — mirrors GNOME/KWin startup-notification). Compositor-side server for internal clients is intentionally NOT implemented; internal clients that want focus should rely on compositor auto-focus + Emacs IPC.
- wl_seat (keyboard + pointer)
- wl_data_device (DnD + selection for internal clients)
- **wlr_data_control_v1 + ext_data_control_v1** for internal clients — exposes the focus-less clipboard path to embedded apps (Firefox, Electron, wl-clipboard, screen-grabs). Mirrors what real wlroots / KDE ≥ 6.2 do.
- fractional_scale, viewporter
- text_input_v3 (IME bridge to host — see smithay fork patches)
- wp_cursor_shape_v1 (cursor shape forwarding to host — Named icons via winit, Surface falls back to default)
- linux-dmabuf (GPU buffer sharing for hardware-accelerated clients)
- wlr-layer-shell (layer surfaces for rofi/wofi launchers — uses LayerMap for layout, keyboard focus set on first commit not on surface creation)
- ext-workspace-v1 (workspace management for external bars/clients — `protocols/workspace.rs`)
- wp-pointer-constraints-v1 (locked_pointer + confined_pointer — activated in `handlers/mod.rs::PointerConstraintsHandler::new_constraint` for focused surface, and on pointer-enter in `input.rs`'s motion handler)
- wp-relative-pointer-v1 (raw mouse delta for FPS camera control; deltas synthesized from successive absolute positions since smithay's winit backend only emits `PointerMotionAbsolute`)

---

# emthin-clipboard

Self-contained host clipboard proxy for nested Wayland compositors. Zero dependency on smithay — the sibling `emthin` crate does the smithay-aware glue in `src/clipboard_bridge.rs`.

## What this crate exports

```
ClipboardBackend    trait — host-facing clipboard proxy
ClipboardEvent      enum — HostSelectionChanged / HostSendRequest / SourceCancelled
SelectionKind       enum — Clipboard / Primary (crate-independent of smithay)
Driver<'a>          enum — OwnedFd(BorrowedFd) or Piggyback
AsyncCompletion     struct — X11-only pipe-drain completion token
BackendHint         enum — DataControl / WlDataDevice{display_ptr} / X11
init(&[BackendHint])  factory that walks the fallback chain
```

## Backend fallback chain

| Variant | Transport | Needs focus? | Notes |
|---|---|---|---|
| `DataControl` | `ext_data_control_v1` or `zwlr_data_control_v1` on a fresh `$WAYLAND_DISPLAY` connection | No | Preferred path; mirrors wlroots / KDE ≥ 6.2 behavior. |
| `WlDataDevice { display_ptr }` | `wl_data_device` on a **foreign** wl_display (caller-owned, e.g. winit's) via `Backend::from_foreign_display` | Yes | Only works while the parent surface has host keyboard focus. Primary selection not implemented here. |
| `X11` | X11 selection via `$DISPLAY`, XFixes-watched | — | For X11 hosts (Xorg / Xvfb). Supports INCR for large payloads. |

`init(&hints)` tries each hint in order and returns the first backend that handshakes. Caller decides the order.

## Driving the backend

```rust
match backend.driver() {
    Driver::OwnedFd(fd) => {
        // Register fd with event loop (READ, level-triggered).
        // Call backend.dispatch() on readable.
    }
    Driver::Piggyback => {
        // No owned fd — the connection is drained elsewhere.
        // Call backend.dispatch() every tick.
    }
}
// After dispatch, drain events:
for event in backend.take_events() {
    match event { ... }
}
```

## Key principles

1. **No smithay**: this crate is reusable by any nested compositor. `SelectionKind` is our own enum; the host maps it to smithay's `SelectionTarget` at the boundary.
2. **`Driver` expresses the fd contract, not a hidden one**. `WlDataDeviceProxy` returns `Piggyback` because it genuinely has no owned fd; we don't manufacture a dummy fd to fit a unified shape.
3. **`HostSendRequest::completion` is the only X11-specific API surface in an otherwise uniform event**. Wayland backends always set it to `None`; X11 emits `Some(AsyncCompletion { id, read_fd })` and the caller must drain `read_fd` then call `ClipboardBackend::complete_outgoing(id, data)`. The default `complete_outgoing` impl is a no-op so Wayland backends stay silent.
4. **Anti-loop via suppress counters**: when we set a host selection, the host will echo back the change as `HostSelectionChanged`. Each backend has `suppress_clipboard` / `suppress_primary` counters (not booleans — Firefox sets selection twice in quick succession) that eat the echo.
5. **`BackendHint::WlDataDevice` is the unsafe surface**: it holds a raw `*mut wl_display` and the caller must guarantee lifetime via `unsafe BackendHint::wl_data_device(ptr)`. Everything else in the public API is safe.

## Deps

- `wayland-client` (+ `wayland-backend` with `client_system` feature for `Backend::from_foreign_display`)
- `wayland-protocols` + `wayland-protocols-wlr` for the data-control definitions
- `x11rb` with `xfixes` for the X11 backend
- `libc` for `pipe2` in the X11 backend's outgoing request path

No smithay, no calloop, no tokio — the crate is runtime-agnostic.

---

# emthin-dbus — DBus session-bus protocol primitives + in-process broker

Zero smithay deps. Provides the SASL handshake scanner, DBus v1 frame
parser + encoder, per-connection byte-stream state machine, fcitx5
frontend classifier / reply synthesis, **and** the full in-process
broker IO loop (listener, upstream dialing, per-connection pumps with
`SCM_RIGHTS` fd passing, fcitx5 signal emitters).

History: started out as a subprocess (`emthin-dbus-proxy` binary) +
JSON ctl socket for cursor-coord rewrite. M1 pulled the broker
in-process under `emthin/src/dbus_broker/`. M2 replaced the
cursor-rewrite hack with a full fcitx5 DBus frontend intercept (B1).
M3 added `SCM_RIGHTS` fd passing so portal.Secret / portal.FileChooser
clients work (Feishu's `RetrieveSecret` was the canary). M4 moved the
broker out of `emthin/` and into this crate's `proxy/` module, since
it has no emthin / smithay deps — just the wire primitives in this
same crate plus libc.

## Module layout

```
src/
├── lib.rs       # crate root + ergonomic re-exports
├── wire/        # DBus wire format (zero-cost over `zvariant`)
│   ├── mod.rs
│   ├── frame.rs # Frame, FrameBuilder, BodyBuilder, Headers, MessageKind,
│   │           # FieldCode, SerialCounter, FrameError
│   └── sasl.rs  # SASL handshake scanner (find_begin_end)
├── broker/      # per-connection byte-stream state machine
│   ├── mod.rs
│   └── state.rs # ConnectionState, FeedOutcome, BrokerError
├── fcitx.rs     # fcitx5 frontend: predicates + classify + IC allocator
│                # + build_reply, all in one ~700-line module since the
│                # surface is small and single-purpose.
└── proxy/       # in-process broker IO loop (listener, upstream dial,
    ├── mod.rs   # per-connection pumps, fcitx5 intercept + signal emit)
    ├── cmsg.rs  # recvmsg/sendmsg + SCM_RIGHTS fd passing
    └── signals.rs # build_preedit_chunks (UpdateFormattedPreedit chunks)
```

## Scope matrix

| Feature | Done | Future |
|---|---|---|
| SASL handshake scanner (`wire/sasl.rs`) | ✅ | |
| DBus v1 frame parser + encoder (`wire/frame.rs`) | ✅ | |
| Per-connection state machine (`broker/state.rs`) | ✅ | |
| Fcitx5 method_call classifier (`fcitx/classify.rs`) | ✅ | |
| Per-connection fcitx5 IC registry (`fcitx/ic.rs`) | ✅ | |
| Fcitx5 method_return synthesis (`fcitx/reply.rs`) | ✅ | |
| In-process broker IO loop (`proxy/mod.rs`) | ✅ | |
| `SCM_RIGHTS` fd passing (`proxy/cmsg.rs`) | ✅ | |
| `RequestName` local-own interception → closes emthin#60 | | ✅ |
| `ListNames` / `NameOwnerChanged` merging for policy | | ✅ |

## Architecture

```
embedded app (WeChat / Emacs pgtk / Electron / Feishu)
       │
       │ DBus (bus.sock injected via DBUS_SESSION_BUS_ADDRESS)
       ▼
┌──────────── emthin-dbus::proxy ─────────────┐
│  DbusBroker (recvmsg/sendmsg + SCM_RIGHTS)  │
│    ├─ ConnectionState (wire/sasl + frames)  │
│    ├─ fcitx::classify (InputMethod1 /       │
│    │                   InputContext1)       │
│    ├─ fcitx::build_reply (method_return)    │
│    └─ FrameBuilder::signal                  │
│        (CommitString / UpdateFormattedPreedit)│
└──────┬──────────────────────────────────────┘
       │ non-fcitx5 methods pass through, fds round-trip
       ▼
  upstream host session bus (real fcitx5 stays untouched)
                  ↑
       OR a private `dbus-daemon` child when emthin is run with
       `--dbus-isolated` — the upstream socket is whatever path the
       consumer hands to `DbusBroker::bind`. From this crate's
       perspective there's no difference: it's still a Unix socket
       speaking DBus. The fcitx5 path keeps working because emthin's
       host fcitx5 reaches the winit window over Wayland (text_input_v3),
       not over this DBus bridge.
```

The consumer crate (e.g. `emthin`) wires the broker's listener fd and
each accepted connection's two fds (`client`, `upstream`) into its
event loop. From this crate's perspective those fds are just data —
calloop / mio / tokio all work the same. Tests use plain
`std::os::unix::net::socketpair` and step the pumps manually.

## Invariants

- **Parser is append-only.** `ConnectionState::feed_from_client(chunk)`
  must be called with successive socket reads; internally buffers
  partial messages. The returned `FeedOutcome.outbound` is the *exact*
  byte sequence to write to the other side — intercept sites filter
  it, not mutate it in place.
- **Encoder is little-endian only.** The parser still accepts
  big-endian input for messages the broker forwards verbatim; anything
  the broker synthesizes itself is LE because every modern Linux DBus
  client is LE and there's no value in the extra path.
- **Signals need a unique-name sender.** `fcitx::build_reply` does not
  set sender — the broker owns that (the caller tracks the real
  fcitx5 unique name via GetNameOwner-reply parsing +
  NameOwnerChanged refresh) and stamps it on the signal frame before
  encoding.
- **IC paths are opaque, not state.** `InputContextAllocator::allocate`
  hands out `(path, uuid)` for the `CreateInputContext` reply and
  forgets immediately — no per-IC state lives in the broker. emthin's
  IME state lives in `winit` + `ImeBridge`, driven by the FcitxEvent
  stream from `dbus_broker::emit_fcitx_event`. Ids are per-connection
  and monotonic so client-side stale references can't collide.
- **Serials are non-zero.** `SerialCounter::bump` skips zero on wrap;
  `next_serial == 0` violates the DBus spec and lockstep clients
  reject the frame.
- **Preedit format flags** (per fcitx5's `FcitxTextFormatFlag`,
  `fcitx-utils/textformatflags.h`): `Underline = 1 << 3`,
  `HighLight = 1 << 4`. `UpdateFormattedPreedit` chunks MUST include
  `Underline` or GTK fcitx-gtk renders the preedit as plain inline
  text (no visual distinction from committed content). The active
  segment (from winit's `(begin, end)` cursor range) gets
  `Underline | HighLight` for the inverted-color "currently composing"
  rendering — see `proxy::signals::build_preedit_chunks`.
- **`BareSignature`, not `Value::Signature`, encodes the SIGNATURE
  header.** zvariant 5 wraps multi-element signatures in `()` (it
  models them as an implicit struct); GDBus / fcitx5 reject signal
  bodies whose declared SIGNATURE includes those parens — IM signals
  silently drop. Regression test:
  `wire::frame::tests::signature_field_does_not_wrap_in_parens`.
- **`SCM_RIGHTS` rides one packet at a time.** The proxy's IO uses
  `recvmsg(MSG_CMSG_CLOEXEC)` / `sendmsg`; outbound queues are
  `VecDeque<OutPacket>` where one packet = one DBus message
  (post-SASL) and its declared `unix_fds` ride alongside that
  packet's first byte. On partial write the fds are gone — they were
  delivered with the first byte — so retry sends the remaining bytes
  with no ancillary. Pre-SASL bytes go through as one fd-less packet.

## Non-goals

- No high-level `Proxy` / `ObjectServer` API. This is raw-byte
  primitives for a broker, not a DBus service library.
- No activation fork-exec logic — all activation stays on the host bus.
- No policy / sandbox filtering. xdg-dbus-proxy's security model is
  out of scope; we use the same DBus-parsing techniques but the
  "what's allowed" question is fully answered by "emthin only
  intercepts fcitx5 interfaces, forwards everything else verbatim".

---

# emthin developer patterns

Patterns derived from git history. Use these as defaults when
working in this repo; override only with explicit reason.

## Commit conventions

This repo uses **Conventional Commits**, filtered by `cliff.toml` into the
changelog.

**Scopes observed:** `release`, `ci`, `focus`, `elisp`, `cli`,
`readme`, `emthin`. Scopes are optional but preferred when
the change is localized — e.g. `refactor(focus): …`.

`chore:`, `style:`, merge, and revert commits are stripped by `cliff.toml`.
Pick a different type if the change deserves a changelog line.

## Co-change patterns

These files tend to move together. When you touch one, check the others:

### IPC change → three sides in lockstep

A protocol change touches **all three**:

1. `crates/emthin/src/ipc/messages.rs` — add enum variant
2. `crates/emthin/src/ipc/dispatch.rs` — handle the variant
3. `elisp/emthin*.el` — send/receive on the elisp side

`OutgoingMessage::method_name()` in `messages.rs` maps `XWaylandReady` →
`"x_wayland_ready"` (manual snake_case; no serde derives).

## Versioning & release

- Workspace version lives in `[workspace.package]` in root `Cargo.toml`.
- `crates/emthin/Cargo.toml` **also** keeps a literal `version = "x.y.z"`
  because cargo-aur 0.x doesn't support `version.workspace = true`. Both
  sites must stay in sync — `cargo release` bumps them together via
  `release.toml` pre-release-replacements anchored by
  `# x-release-please-version`.
- Release is `cargo release patch --execute` (or `minor` / explicit).
  That runs `git-cliff` → updates `CHANGELOG.md` → single `chore: release`
  commit → tag → tag push.
- Historical note: the repo migrated from release-please to
  cargo-release + git-cliff; don't re-introduce release-please config.
  `.github/workflows/release.yml` was removed when the fork moved
  to a separate upstream; release automation is manual from here on.

## Local verification

Before pushing, run:

```
cargo fmt --all --check
cargo clippy --workspace -- -D warnings
cargo build --workspace
```

If the change affects xwayland-satellite:

```
cargo test -p emthin
```

## Code-style defaults

- **Comments / logs / docs in Rust source: English only.** No Chinese in
  `.rs` files.
- **Never `git push` without explicit user approval.** `git commit` does not
  include push. Same for creating releases.

## smithay is a fork

When reading smithay source to trace behavior, use the vendored checkout at
`~/.cargo/git/checkouts/smithay-*/<commit>/` — that revision carries the
emthin patches (`backend/winit/mod.rs`, `text_input/text_input_handle.rs`,
`selection/seat_data.rs`). A clean upstream clone won't match what the
compositor actually links against.
