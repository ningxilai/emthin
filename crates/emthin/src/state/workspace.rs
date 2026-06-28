//! Workspace model: each Emacs frame = one workspace. The active
//! workspace's `Space<Window>` lives inline; inactive workspaces are
//! swapped out into `inactive`.
//!
//! Cross-subsystem operations (`switch_workspace`, `destroy_workspace`,
//! `migrate_app_to_active`) stay on `EmthinState` because they touch
//! seat, IME, focus, apps, and IPC. Only pure workspace-local
//! operations live as methods here.

use std::collections::HashMap;

use smithay::{
    desktop::{Space, Window},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::shell::xdg::ToplevelSurface,
};

/// State for an inactive workspace (swapped out when another is active).
pub struct Workspace {
    pub space: Space<Window>,
    pub emacs_surface: Option<WlSurface>,
    /// Display name for the bar (extracted from Emacs frame title).
    pub name: String,
}

/// Workspace-related fields grouped together. Replaces seven loose
/// fields on `EmthinState`.
pub struct WorkspaceState {
    /// The active workspace's space (swapped in/out on switch).
    pub active_space: Space<Window>,
    /// Inactive workspaces, keyed by workspace id.
    pub inactive: HashMap<u64, Workspace>,
    /// Id of the currently active workspace.
    pub active_id: u64,
    /// Display name of the active workspace.
    pub active_name: String,
    /// Next workspace id to allocate.
    pub next_id: u64,
    /// Emacs toplevels awaiting parent() check (child frame detection).
    pub pending_emacs_toplevels: Vec<(ToplevelSurface, Window)>,
    /// Embedded-app toplevels awaiting dialog-vs-app classification.
    /// Same one-tick-defer pattern as `pending_emacs_toplevels`: the
    /// dispatch_clients pass that fires `new_toplevel` may not yet have
    /// processed the client's `set_parent` / `set_min_size` /
    /// `set_max_size` requests (cf. sway/desktop/xdg_shell.c:228
    /// `wants_floating`). Drained in `tick::process_pending_toplevels`.
    pub pending_app_toplevels: Vec<(ToplevelSurface, Window)>,
    /// ext-workspace-v1 protocol state.
    pub protocol: crate::protocols::workspace::WorkspaceProtocolState,
}

impl WorkspaceState {
    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Total number of workspaces (active + inactive).
    pub fn count(&self) -> usize {
        1 + self.inactive.len()
    }

    /// Mutable reference to the `Space<Window>` for a given workspace
    /// id. Returns the active space if `ws_id` matches, otherwise looks
    /// up inactive.
    pub fn space_for_mut(&mut self, ws_id: u64) -> Option<&mut Space<Window>> {
        if ws_id == self.active_id {
            Some(&mut self.active_space)
        } else {
            self.inactive.get_mut(&ws_id).map(|ws| &mut ws.space)
        }
    }

    /// Sorted list of all workspace ids.
    pub fn all_ids(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = std::iter::once(self.active_id)
            .chain(self.inactive.keys().copied())
            .collect();
        ids.sort_unstable();
        ids
    }
}

use crate::ipc::OutgoingMessage;
use smithay::reexports::wayland_server::Resource;

/// Process deferred Emacs toplevels from `pending_emacs_toplevels`.
/// Called once per tick after `dispatch_clients` has resolved
/// `surface.parent()` for same-batch toplevels.
pub(crate) fn process_pending_toplevels(state: &mut crate::EmthinState) {
    let pending = std::mem::take(&mut state.workspace.pending_emacs_toplevels);
    if pending.is_empty() {
        return;
    }
    state.needs_redraw = true;
    for (surface, window) in pending {
        if surface.parent().is_some() {
            tracing::info!(
                "Emacs child frame confirmed (has parent), workspace {}",
                state.workspace.active_id
            );
        } else {
            state.workspace.active_space.unmap_elem(&window);
            let ws_id = state.workspace.alloc_id();
            tracing::info!("new Emacs frame → workspace {ws_id}");

            let emacs_wl = surface.wl_surface().clone();
            let mut new_space = smithay::desktop::Space::default();
            if let Some(output) = state.workspace.active_space.outputs().next().cloned() {
                new_space.map_output(&output, (0, 0));
            }

            state.workspace.inactive.insert(
                ws_id,
                Workspace {
                    space: new_space,
                    emacs_surface: Some(emacs_wl),
                    name: String::new(),
                },
            );

            if let Some(geo) = state.emacs_geometry() {
                surface.with_pending_state(|s| {
                    s.size = Some(geo.size);
                    s.states.set(
                        smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Fullscreen,
                    );
                });
                surface.send_pending_configure();

                if let Some(ws) = state.workspace.inactive.get_mut(&ws_id) {
                    ws.space.map_element(window.clone(), geo.loc, false);
                    ws.space.lower_element(&window);
                }
            }

            state.ipc.send(OutgoingMessage::WorkspaceCreated {
                workspace_id: ws_id,
            });

            state.switch_workspace(ws_id);
        }
    }
}

/// Process ext-workspace-v1 client actions (activate, remove, etc.).
pub(crate) fn process_workspace_actions(state: &mut crate::EmthinState) {
    let actions = state.workspace.protocol.take_pending_actions();
    if actions.is_empty() {
        return;
    }
    state.needs_redraw = true;
    for action in actions {
        use crate::protocols::workspace::WorkspaceAction;
        match action {
            WorkspaceAction::Activate(id) => {
                state.switch_workspace(id);
            }
            WorkspaceAction::Remove(id) => {
                if id != state.workspace.active_id {
                    state.destroy_workspace(id);
                    state
                        .ipc
                        .send(OutgoingMessage::WorkspaceDestroyed { workspace_id: id });
                }
            }
            other => tracing::warn!("ext-workspace: unhandled action {other:?}"),
        }
    }
}

/// Detect dead Emacs frames (inactive workspaces whose surface died,
/// or the active frame itself) and clean up.
pub(crate) fn detect_dead_workspaces(state: &mut crate::EmthinState) {
    let dead_ws: Vec<u64> = state
        .workspace
        .inactive
        .iter()
        .filter(|(_, ws)| ws.emacs_surface.as_ref().is_none_or(|s| !s.is_alive()))
        .map(|(id, _)| *id)
        .collect();
    let had_dead = !dead_ws.is_empty();
    for ws_id in dead_ws {
        state.destroy_workspace(ws_id);
        state.ipc.send(OutgoingMessage::WorkspaceDestroyed {
            workspace_id: ws_id,
        });
        tracing::info!("workspace {ws_id} destroyed (Emacs frame died)");
    }
    if had_dead {
        state.needs_redraw = true;
    }

    if state.emacs.main_died() {
        if let Some(&fallback_id) = state.workspace.inactive.keys().next() {
            tracing::info!("active Emacs died, switching to workspace {fallback_id}");
            state.switch_workspace(fallback_id);
            state.needs_redraw = true;
        } else {
            tracing::info!("last Emacs frame died, stopping");
            state.loop_signal.stop();
        }
    }
}

/// Send ext-workspace-v1 protocol events reflecting current workspace
/// state, then clean up dead protocol handles.
pub(crate) fn refresh_workspace_state(state: &mut crate::EmthinState) {
    let ws_ids = state.workspace.all_ids();

    let ws_named: Vec<(u64, &str)> = ws_ids
        .iter()
        .map(|&id| {
            let name: &str = if id == state.workspace.active_id {
                &state.workspace.active_name
            } else {
                state
                    .workspace
                    .inactive
                    .get(&id)
                    .map(|ws| ws.name.as_str())
                    .unwrap_or("")
            };
            (id, name)
        })
        .collect();

    let ws_infos: Vec<crate::protocols::workspace::WorkspaceInfo> = ws_named
        .iter()
        .map(|&(id, name)| {
            let display_name = if name.is_empty() {
                format!("Workspace {id}")
            } else {
                name.to_string()
            };
            crate::protocols::workspace::WorkspaceInfo {
                id,
                name: display_name,
                active: id == state.workspace.active_id,
            }
        })
        .collect();
    if let Some(output) = state.workspace.active_space.outputs().next().cloned() {
        let dh = state.display_handle.clone();
        state.workspace.protocol.refresh(&dh, &ws_infos, &output);
    }
    state.workspace.protocol.cleanup_dead();

    let _ = ws_named;
}
