//! Event loop tick — the per-frame idle callback for the compositor.

use smithay::utils::IsAlive;

use crate::state::EmthinState;

/// Called once per event loop iteration. Handles workspace lifecycle,
/// IPC dispatch, clipboard events, and pending geometry timeouts.
pub fn event_loop_tick(state: &mut EmthinState) {
    // --- Check if Emacs child process has exited ---
    if let Some(child) = state.emacs.child_mut() {
        if let Ok(Some(status)) = child.try_wait() {
            tracing::info!("Emacs exited with {status}, stopping compositor");
            state.loop_signal.stop();
        }
    }

    // --- Workspace: process deferred Emacs toplevels ---
    // After dispatch_clients, set_parent has been processed for same-batch
    // toplevels, so surface.parent() is now accurate.
    crate::state::workspace::process_pending_toplevels(state);

    // --- Embedded toplevels: classify dialog vs app, then route ---
    // Same one-tick defer rationale (parent + min/max are cleared up only
    // after the client's initial dispatch burst completes).
    process_pending_app_toplevels(state);

    // --- Workspace: process ext-workspace-v1 client actions ---
    crate::state::workspace::process_workspace_actions(state);

    // --- Workspace: detect dead Emacs frames ---
    crate::state::workspace::detect_dead_workspaces(state);

    // --- Workspace: refresh ext-workspace-v1 protocol + bar ---
    crate::state::workspace::refresh_workspace_state(state);

    // --- Clean up destroyed embedded app windows ---
    crate::handlers::apps::cleanup_dead_apps(state);
    // --- Clean up destroyed floating dialogs ---
    crate::handlers::dialogs::cleanup_dead_dialogs(state);

    // --- Dispatch incoming IPC messages from Emacs ---
    if let Some(msgs) = state.ipc.recv_all() {
        for msg in msgs {
            crate::ipc::dispatch::handle_ipc_message(state, msg);
        }
        state.needs_redraw = true;
    }

    state.ipc.flush();

    // --- Process clipboard events from host compositor ---
    // `OwnedFd` backends (data-control, X11) are driven by their calloop
    // fd source in `main::register_clipboard_source`, so we just drain
    // events here. `Piggyback` backends (wl_data_device on winit's
    // shared connection) have no owned fd — this tick is the only point
    // at which we can collect buffered events from libwayland's per-queue
    // ring before draining them.
    let clipboard_events = state
        .selection
        .clipboard
        .as_mut()
        .map(|c| {
            if matches!(c.driver(), emthin_clipboard::Driver::Piggyback) {
                c.dispatch();
            }
            c.take_events()
        })
        .unwrap_or_default();
    let has_clipboard_events = !clipboard_events.is_empty();
    for event in clipboard_events {
        crate::clipboard_bridge::handle_clipboard_event(state, event);
    }
    // Flush immediately so Wayland clients see selection changes / send
    // requests without waiting for the next render frame.
    if has_clipboard_events {
        let _ = state.display_handle.flush_clients();
        state.needs_redraw = true;
    }

    // --- Force-commit pending geometries that have timed out (100ms) ---
    let timed_out = state
        .apps
        .collect_timed_out(std::time::Duration::from_millis(100));
    if !timed_out.is_empty() {
        state.needs_redraw = true;
    }
    for (window_id, window, geo) in timed_out {
        let ws_id = state
            .apps
            .get(window_id)
            .map(|a| a.workspace_id)
            .unwrap_or(state.workspace.active_id);
        if let Some(space) = state.workspace.space_for_mut(ws_id) {
            space.map_element(window, geo.loc, false);
        }
        tracing::debug!("embedded app window_id={window_id} geometry force-committed (timeout)");
    }

    // Drain broker-observed fcitx5 events and drive winit IME in
    // emthin-winit-local coords.
    crate::state::ime::drain_fcitx_events(state);

    // Poll text_input_v3 cursor freshness — picks up the focused
    // client's `set_cursor_rectangle` request the moment it lands,
    // without waiting for a host IME event. Fixes "popup at fallback
    // (surface origin) until first preedit" for tip-only clients
    // (Alacritty etc.) whose cursor_rectangle arrives async after
    // focus.
    state.ime.poll_tip_freshness(&state.seat, &state.apps);

    // --- Forward DBus router notifications to Emacs ---
    forward_router_notifications(state);
}

/// Forward non-fcitx router notifications (rule add/remove/list) to Emacs
/// via IPC.
fn forward_router_notifications(state: &mut EmthinState) {
    let notifications = state.dbus.take_router_notifications();
    for n in notifications {
        let msg = match n {
            emthin_dbus::router::RouterNotification::FcitxEvent(_) => continue,
            emthin_dbus::router::RouterNotification::RuleAdded { id, rule } => {
                crate::ipc::OutgoingMessage::DbusRouterRuleAdded { id, rule }
            }
            emthin_dbus::router::RouterNotification::RuleRemoved { id } => {
                crate::ipc::OutgoingMessage::DbusRouterRuleRemoved { id }
            }
            emthin_dbus::router::RouterNotification::RuleList { rules } => {
                crate::ipc::OutgoingMessage::DbusRouterRules { rules }
            }
        };
        state.ipc.send(msg);
    }
}

/// Drain `pending_app_toplevels`, classify each as either a floating
/// dialog (login boxes, file pickers, …) or an embedded app, and route
/// it accordingly. Same one-tick-defer rationale as
/// `process_pending_toplevels`: by now the client's
/// `set_parent` / `set_min_size` / `set_max_size` from the same dispatch
/// burst have been processed, so `wants_floating` returns a stable
/// answer.
fn process_pending_app_toplevels(state: &mut EmthinState) {
    let pending = std::mem::take(&mut state.workspace.pending_app_toplevels);
    if pending.is_empty() {
        return;
    }
    state.needs_redraw = true;
    for (surface, window) in pending {
        if !window.alive() {
            continue;
        }
        let floating = crate::handlers::xdg_shell::wants_floating(state, &surface);
        // Log the full set of inputs the heuristic actually sees, so a
        // misclassified login window (wechat / feishu / file pickers)
        // is debuggable from the journal without rebuilding emthin.
        let log = inspect_toplevel_for_log(&surface);
        tracing::info!(
            target: "emthin::dialog",
            floating,
            has_parent = surface.parent().is_some(),
            title = ?log.title,
            app_id = ?log.app_id,
            min_w = log.min.w,
            min_h = log.min.h,
            max_w = log.max.w,
            max_h = log.max.h,
            "embedded toplevel classification",
        );
        if floating {
            crate::handlers::dialogs::promote_floating_dialog(state, surface, window);
        } else {
            crate::handlers::apps::register_embedded_app(state, surface, window);
        }
    }
}

/// Inputs to `wants_floating`, captured for diagnostic logging.
struct ToplevelLogInputs {
    title: Option<String>,
    app_id: Option<String>,
    min: smithay::utils::Size<i32, smithay::utils::Logical>,
    max: smithay::utils::Size<i32, smithay::utils::Logical>,
}

/// Read the inputs that `wants_floating` consults, for diagnostic
/// logging. Pure side-effect-free read of the toplevel's cached state.
fn inspect_toplevel_for_log(
    surface: &smithay::wayland::shell::xdg::ToplevelSurface,
) -> ToplevelLogInputs {
    use smithay::wayland::compositor::with_states;
    use smithay::wayland::shell::xdg::{SurfaceCachedState, XdgToplevelSurfaceData};

    let (title, app_id) = with_states(surface.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|d| d.lock().ok())
            .map(|d| (d.title.clone(), d.app_id.clone()))
            .unwrap_or((None, None))
    });
    let (min, max) = with_states(surface.wl_surface(), |states| {
        let mut cached = states.cached_state.get::<SurfaceCachedState>();
        let current = cached.current();
        (current.min_size, current.max_size)
    });
    ToplevelLogInputs {
        title,
        app_id,
        min,
        max,
    }
}
