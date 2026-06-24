use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

use crate::ipc::OutgoingMessage;
use crate::EmskinState;

/// Register an embedded toplevel with `AppManager`, send `WindowCreated`
/// IPC, and apply the auto-focus policy. This is the original
/// synchronous path from `xdg_shell::new_toplevel`, now run from the
/// drain pass.
pub fn register_embedded_app(
    state: &mut EmskinState,
    surface: smithay::wayland::shell::xdg::ToplevelSurface,
    window: smithay::desktop::Window,
) {
    let window_id = state.apps.alloc_id();
    let title = with_states(surface.wl_surface(), |s| {
        s.data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|d| d.lock().ok())
            .and_then(|d| d.title.clone())
    })
    .unwrap_or_default();

    tracing::info!("embedded app toplevel connected: window_id={window_id} title={title:?}");

    surface.with_pending_state(|s| {
        s.size = Some((1, 1).into());
    });
    surface.send_pending_configure();

    state.apps.insert(crate::apps::AppWindow {
        window_id,
        window: window.clone(),
        workspace_id: state.workspace.active_id,
        geometry: None,
        pending_geometry: None,
        pending_since: None,
        visible: false,
        mirrors: std::collections::HashMap::new(),
    });

    state
        .ipc
        .send(OutgoingMessage::WindowCreated { window_id, title });

    state.auto_focus_new_window(window, window_id);
}

pub fn cleanup_dead_apps(state: &mut EmskinState) {
    let dead = state.apps.drain_dead();
    if dead.is_empty() {
        return;
    }
    state.needs_redraw = true;
    for app in &dead {
        if let Some(space) = state.workspace.space_for_mut(app.workspace_id) {
            space.unmap_elem(&app.window);
        }
        state.ipc.send(OutgoingMessage::WindowDestroyed {
            window_id: app.window_id,
        });
        tracing::info!("embedded app window_id={} destroyed", app.window_id);
    }
    if let Some(keyboard) = state.seat.get_keyboard() {
        if keyboard.current_focus().is_none() {
            let target = state.emacs_focus_target();
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            keyboard.set_focus(state, target, serial);
            tracing::debug!("focus returned to Emacs after window destroy");
        }
    }
}
