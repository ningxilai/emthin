use crate::ipc::IncomingMessage;
use crate::EmthinState;

pub fn handle_ipc_message(state: &mut EmthinState, msg: IncomingMessage) {
    match msg {
        IncomingMessage::SetGeometry { window_id, rect } => {
            ipc_set_geometry(state, window_id, rect);
        }
        IncomingMessage::Close { window_id } => {
            ipc_close(state, window_id);
        }
        IncomingMessage::SetVisibility { window_id, visible } => {
            ipc_set_visibility(state, window_id, visible);
        }
        IncomingMessage::PrefixDone => {
            ipc_prefix_done(state);
        }
        IncomingMessage::PrefixClear => {
            // IME-only cleanup, no focus restoration.
            state.ime.set_prefix_active(false);
        }
        IncomingMessage::AddMirror {
            window_id,
            view_id,
            rect,
        } => {
            ipc_add_mirror(state, window_id, view_id, rect);
        }
        IncomingMessage::UpdateMirrorGeometry {
            window_id,
            view_id,
            rect,
        } => {
            ipc_update_mirror_geometry(state, window_id, view_id, rect);
        }
        IncomingMessage::RemoveMirror { window_id, view_id } => {
            ipc_remove_mirror(state, window_id, view_id);
        }
        IncomingMessage::PromoteMirror { window_id, view_id } => {
            ipc_promote_mirror(state, window_id, view_id);
        }
        IncomingMessage::SetFocus { window_id } => {
            ipc_set_focus(state, window_id);
        }
        IncomingMessage::SwitchWorkspace { workspace_id } => {
            tracing::debug!("IPC switch_workspace {workspace_id}");
            // switch_workspace sends WorkspaceSwitched IPC internally
            // (before keyboard.set_focus to avoid race conditions).
            state.switch_workspace(workspace_id);
        }
        IncomingMessage::DbusRouterAddRule { rule } => {
            tracing::debug!("IPC dbus_router_add_rule id={}", rule.id);
            state
                .dbus
                .send_rpc(&emthin_dbus::RouterRequest::AddRule { rule });
        }
        IncomingMessage::DbusRouterRemoveRule { id } => {
            tracing::debug!("IPC dbus_router_remove_rule id={id}");
            state
                .dbus
                .send_rpc(&emthin_dbus::RouterRequest::RemoveRule { id });
        }
        IncomingMessage::DbusRouterListRules => {
            tracing::debug!("IPC dbus_router_list_rules");
            state.dbus.send_rpc(&emthin_dbus::RouterRequest::ListRules);
        }
    }
}

fn ipc_set_geometry(state: &mut EmthinState, window_id: u64, rect: crate::ipc::IpcRect) {
    let crate::ipc::IpcRect { x, y, w, h } = rect;
    tracing::debug!("IPC set_geometry window={window_id} ({x:.3},{y:.3} {w:.3}x{h:.3})");
    if w <= 0.0 || h <= 0.0 {
        tracing::warn!("IPC set_geometry: invalid size ({w}x{h}), ignoring");
        return;
    }
    let new_geo = state.fraction_to_canvas(rect);
    let area = state.usable_area();
    let px_w = (w * area.size.w as f64).round().max(1.0) as i32;
    let px_h = (h * area.size.h as f64).round().max(1.0) as i32;

    let Some(app) = state.apps.get_mut(window_id) else {
        return;
    };
    app.visible = true;

    state.migrate_app_to_active(window_id);

    let Some(app) = state.apps.get_mut(window_id) else {
        return;
    };

    let Some(toplevel) = app.window.toplevel() else {
        return;
    };
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
    toplevel.with_pending_state(|s| {
        s.size = Some((px_w, px_h).into());
        s.states.set(xdg_toplevel::State::TiledLeft);
        s.states.set(xdg_toplevel::State::TiledRight);
        s.states.set(xdg_toplevel::State::TiledTop);
        s.states.set(xdg_toplevel::State::TiledBottom);
    });
    toplevel.send_pending_configure();

    if app.geometry.is_none() {
        app.geometry = Some(new_geo);
        let window = app.window.clone();
        state
            .workspace
            .active_space
            .map_element(window, new_geo.loc, false);
        tracing::info!(
            "app {window_id} mapped immediately at ({},{}) ws={}",
            new_geo.loc.x,
            new_geo.loc.y,
            state.workspace.active_id
        );
    } else {
        app.pending_geometry = Some(new_geo);
        app.pending_since = Some(std::time::Instant::now());
        tracing::debug!(
            "app {window_id} pending geometry ({},{}) ws={}",
            new_geo.loc.x,
            new_geo.loc.y,
            state.workspace.active_id
        );
    }
}

fn ipc_close(state: &mut EmthinState, window_id: u64) {
    tracing::debug!("IPC close window={window_id}");
    if let Some(app) = state.apps.get_mut(window_id) {
        if let Some(toplevel) = app.window.toplevel() {
            toplevel.send_close();
        }
    }
}

fn ipc_set_visibility(state: &mut EmthinState, window_id: u64, visible: bool) {
    tracing::debug!("IPC set_visibility window={window_id} visible={visible}");
    let Some(app) = state.apps.get_mut(window_id) else {
        return;
    };
    app.visible = visible;
    let win = app.window.clone();
    let geo = app.geometry;
    if !visible {
        // Unmap from whichever space it's in.
        let ws_id = app.workspace_id;
        if let Some(space) = state.workspace.space_for_mut(ws_id) {
            space.unmap_elem(&win);
        }
    } else if let Some(geo) = geo {
        state.migrate_app_to_active(window_id);
        // Write back geometry (migrate resets it to None).
        if let Some(app) = state.apps.get_mut(window_id) {
            app.geometry = Some(geo);
        }
        state
            .workspace
            .active_space
            .map_element(win, geo.loc, false);
    }
}

fn ipc_prefix_done(state: &mut EmthinState) {
    // Always re-enable host IME at chord end, even if no prefix override
    // was active (so Emacs's prefix_done IPC remains functional after a
    // dropped intermediate IPC).
    state.ime.set_prefix_active(false);
    let Some(saved) = state.focus.exit(crate::state::FocusOverride::Prefix) else {
        return;
    };
    let Some(keyboard) = state.seat.get_keyboard() else {
        return;
    };
    tracing::debug!("IPC prefix_done: restoring focus");
    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
    keyboard.set_focus(state, saved, serial);
}

fn ipc_add_mirror(
    state: &mut EmthinState,
    window_id: u64,
    view_id: u64,
    rect: crate::ipc::IpcRect,
) {
    let ws_id = state.workspace.active_id;
    tracing::debug!(
        "IPC add_mirror window={window_id} view={view_id} rect=({:.3},{:.3} {:.3}x{:.3}) ws={ws_id}",
        rect.x, rect.y, rect.w, rect.h,
    );
    if rect.w <= 0.0 || rect.h <= 0.0 {
        tracing::warn!("IPC add_mirror: invalid size, ignoring");
        return;
    }
    let geo = state.fraction_to_canvas(rect);
    let Some(app) = state.apps.get_mut(window_id) else {
        tracing::warn!("add_mirror: unknown window_id={window_id}");
        return;
    };
    app.mirrors.insert(
        view_id,
        crate::apps::MirrorView {
            geometry: geo,
            workspace_id: ws_id,
        },
    );
}

fn ipc_update_mirror_geometry(
    state: &mut EmthinState,
    window_id: u64,
    view_id: u64,
    rect: crate::ipc::IpcRect,
) {
    tracing::debug!(
        "IPC update_mirror_geometry window={window_id} view={view_id} rect=({:.3},{:.3} {:.3}x{:.3})",
        rect.x, rect.y, rect.w, rect.h,
    );
    if rect.w <= 0.0 || rect.h <= 0.0 {
        tracing::warn!("IPC update_mirror_geometry: invalid size, ignoring");
        return;
    }
    let geo = state.fraction_to_canvas(rect);
    let Some(app) = state.apps.get_mut(window_id) else {
        return;
    };
    if let Some(mirror) = app.mirrors.get_mut(&view_id) {
        mirror.geometry = geo;
    }
}

fn ipc_remove_mirror(state: &mut EmthinState, window_id: u64, view_id: u64) {
    tracing::debug!("IPC remove_mirror window={window_id} view={view_id}");
    if let Some(app) = state.apps.get_mut(window_id) {
        app.mirrors.remove(&view_id);
    }
}

fn ipc_promote_mirror(state: &mut EmthinState, window_id: u64, view_id: u64) {
    tracing::debug!("IPC promote_mirror window={window_id} view={view_id}");
    let Some(app) = state.apps.get_mut(window_id) else {
        return;
    };
    if let Some(mirror) = app.mirrors.remove(&view_id) {
        let old_ws = app.workspace_id;
        let new_ws = mirror.workspace_id;
        app.geometry = Some(mirror.geometry);
        let window = app.window.clone();

        if old_ws != new_ws {
            app.workspace_id = new_ws;
            if let Some(space) = state.workspace.space_for_mut(old_ws) {
                space.unmap_elem(&window);
            }
            let app_geo = state.apps.get(window_id).and_then(|a| a.geometry);
            if let Some(geo) = app_geo {
                if let Some(space) = state.workspace.space_for_mut(new_ws) {
                    space.map_element(window, geo.loc, false);
                }
            }
        } else {
            state
                .workspace
                .active_space
                .map_element(window, mirror.geometry.loc, false);
        }
    }
}

fn ipc_set_focus(state: &mut EmthinState, window_id: Option<u64>) {
    let Some(keyboard) = state.seat.get_keyboard() else {
        return;
    };
    let target = match window_id {
        Some(id) => state
            .apps
            .get(id)
            .map(|app| crate::KeyboardFocusTarget::from(app.window.clone()))
            .or_else(|| state.emacs_focus_target()),
        None => state.emacs_focus_target(),
    };
    tracing::debug!("IPC set_focus window_id={window_id:?}");
    state.focus.exit(crate::state::FocusOverride::Prefix);
    state.ime.set_prefix_active(false);
    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
    keyboard.set_focus(state, target, serial);
}
