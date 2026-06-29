use crate::EmthinState;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

/// Marker stored in a floating dialog's `Window::user_data`. The
/// compositor commit handler looks for this tag and re-centers the
/// window on every commit until the client's natural size stops
/// changing — the tag itself stays for the dialog's lifetime so a
/// configure-driven resize (e.g. content swap inside a wechat login →
/// captcha → 2FA flow) still pulls the window back to center.
#[derive(Clone, Copy, Default)]
pub struct FloatingDialogTag;

/// Marker on `Window::user_data` indicating an embedded toplevel that
/// hasn't been classified yet (still in `pending_app_toplevels`).
/// Removed in `process_pending_app_toplevels` once we know whether to
/// route to dialog (configure 0, 0) or app (configure 1, 1).
#[derive(Clone, Copy, Default)]
pub struct PendingClassificationTag;

/// Map a toplevel as a centered, AppManager-less dialog. The client's
/// natural size drives geometry — we configure 0×0 so the toplevel
/// commits its preferred size. Re-centering happens on every commit
/// in `handlers::compositor::commit` via the `FloatingDialogTag`
/// marker, because at this point the buffer hasn't been attached yet
/// and `window.geometry().size` is still (0, 0) / (1, 1).
pub fn promote_floating_dialog(
    state: &mut EmthinState,
    surface: smithay::wayland::shell::xdg::ToplevelSurface,
    window: smithay::desktop::Window,
) {
    let title = with_states(surface.wl_surface(), |s| {
        s.data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|d| d.lock().ok())
            .and_then(|d| d.title.clone())
    });
    tracing::info!("embedded toplevel routed to floating dialog: title={title:?}");

    window
        .user_data()
        .insert_if_missing(FloatingDialogTag::default);

    surface.with_pending_state(|s| {
        s.size = Some((0, 0).into());
        s.states
            .set(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Activated);
    });
    surface.send_pending_configure();

    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
    if let Some(keyboard) = state.seat.get_keyboard() {
        keyboard.set_focus(state, Some(window.into()), serial);
    }
}

/// Center a window inside the active output. Mirrors
/// sway/tree/container.c:1219 `container_floating_move_to_center`:
///   new_lx = output.x + (output.w - win.w) / 2
///   new_ly = output.y + (output.h - win.h) / 2
pub fn re_center_dialog(state: &mut EmthinState, window: &smithay::desktop::Window) {
    let Some(output) = state.workspace.active_space.outputs().next().cloned() else {
        return;
    };
    let Some(output_geo) = state.workspace.active_space.output_geometry(&output) else {
        return;
    };
    let win_size = window.geometry().size;
    let new_x = output_geo.loc.x + (output_geo.size.w - win_size.w) / 2;
    let new_y = output_geo.loc.y + (output_geo.size.h - win_size.h) / 2;
    state
        .workspace
        .active_space
        .map_element(window.clone(), (new_x, new_y), false);
}

/// Reap dead floating-dialog Windows. Dialogs aren't tracked in
/// `AppManager`, so `cleanup_dead_apps` doesn't see them — we walk
/// every Space and unmap any element whose underlying surface has
/// died. Runs *after* `cleanup_dead_apps` so any AppWindow's space
/// element has already been removed by that pass; everything left
/// dead in a Space is therefore a dialog (Emacs' Window stays alive
/// for the compositor's lifetime).
pub fn cleanup_dead_dialogs(state: &mut EmthinState) {
    use smithay::utils::IsAlive;

    let mut had_dead = false;

    let dead: Vec<smithay::desktop::Window> = state
        .workspace
        .active_space
        .elements()
        .filter(|w| !w.alive())
        .cloned()
        .collect();
    for window in &dead {
        state.workspace.active_space.unmap_elem(window);
    }
    had_dead |= !dead.is_empty();

    for ws in state.workspace.inactive.values_mut() {
        let dead: Vec<smithay::desktop::Window> = ws
            .space
            .elements()
            .filter(|w| !w.alive())
            .cloned()
            .collect();
        for window in &dead {
            ws.space.unmap_elem(window);
        }
        had_dead |= !dead.is_empty();
    }

    if !had_dead {
        return;
    }
    state.needs_redraw = true;

    if let Some(keyboard) = state.seat.get_keyboard() {
        let needs_fallback = match keyboard.current_focus() {
            None => true,
            Some(crate::state::KeyboardFocusTarget::Window(ref w)) => !w.alive(),
            _ => false,
        };
        if needs_fallback {
            let target = state.emacs_focus_target();
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            keyboard.set_focus(state, target, serial);
            tracing::debug!("focus returned to Emacs after dialog destroy");
        }
    }
}
