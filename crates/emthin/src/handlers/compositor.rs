use std::time::Duration;

use crate::{state::ClientState, EmthinState};
use smithay::wayland::seat::WaylandFocus;
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_shm,
    desktop::utils::send_frames_surface_tree,
    reexports::wayland_server::{
        protocol::{wl_buffer, wl_surface::WlSurface},
        Client,
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            get_parent, is_sync_subsurface, CompositorClientState, CompositorHandler,
            CompositorState,
        },
        shm::{ShmHandler, ShmState},
    },
};

use super::xdg_shell;

impl CompositorHandler for EmthinState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.wl.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("ClientState missing — client was not inserted via our listener")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            let committed_window = self
                .workspace
                .active_space
                .elements()
                .find(|w| w.wl_surface().map(|s| *s == root).unwrap_or(false))
                .cloned();
            if let Some(ref window) = committed_window {
                window.on_commit();
            }
            // Floating dialog: re-center on every commit until the
            // client's natural size lands (`promote_floating_dialog`
            // configures 0×0, the buffer that finally arrives sets the
            // real size, and we want it visually centered). Idempotent
            // — `re_center_dialog` is just `map_element` with a fresh
            // location.
            if let Some(window) = committed_window {
                if window
                    .user_data()
                    .get::<crate::handlers::dialogs::FloatingDialogTag>()
                    .is_some()
                    && window.geometry().size.w > 0
                    && window.geometry().size.h > 0
                {
                    crate::handlers::dialogs::re_center_dialog(self, &window);
                }
            }

            // Pending → committed geometry transition for embedded app windows.
            // When an embedded app commits a new buffer after a configure, atomically
            // switch its geometry so the new buffer and new position appear together.
            let commit_info = self.apps.get_mut_by_surface(&root).and_then(|app| {
                app.pending_geometry.take().map(|pending| {
                    app.geometry = Some(pending);
                    app.pending_since = None;
                    (app.window.clone(), app.window_id, pending)
                })
            });
            if let Some((window, window_id, geo)) = commit_info {
                self.workspace
                    .active_space
                    .map_element(window, geo.loc, false);
                tracing::debug!("embedded app window_id={window_id} geometry committed: {geo:?}");
            }
        };

        xdg_shell::handle_surface_commit(
            &mut self.wl.popups,
            &self.workspace.active_space,
            surface,
        );

        // Fire frame callbacks for surfaces not tracked in space
        // (e.g., temporary Vulkan test surfaces created during GPU init).
        // Without this, Vulkan WSI's vkQueuePresentKHR stalls waiting for
        // wl_surface.frame callbacks that never arrive.
        let is_space_element = self
            .workspace
            .active_space
            .elements()
            .any(|w| w.wl_surface().is_some_and(|s| *s == *surface));
        if !is_space_element {
            tracing::trace!("untracked surface commit: {surface:?}");
            if let Some(output) = self.workspace.active_space.outputs().next() {
                send_frames_surface_tree(
                    surface,
                    output,
                    self.start_time.elapsed(),
                    Some(Duration::ZERO),
                    |_, _| None,
                );
            }
        }
    }
}

impl BufferHandler for EmthinState {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for EmthinState {
    fn shm_state(&self) -> &ShmState {
        &self.wl.shm_state
    }
}

delegate_compositor!(EmthinState);
delegate_shm!(EmthinState);

smithay::delegate_viewporter!(EmthinState);
impl smithay::wayland::fractional_scale::FractionalScaleHandler for EmthinState {
    fn new_fractional_scale(
        &mut self,
        _surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
    }
}
smithay::delegate_fractional_scale!(EmthinState);
