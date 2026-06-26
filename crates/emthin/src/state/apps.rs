use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use smithay::{
    desktop::{PopupManager, Window},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{IsAlive, Logical, Point, Rectangle},
    wayland::seat::WaylandFocus,
};

/// An embedded application window.
pub struct AppWindow {
    pub window_id: u64,
    pub window: Window,
    /// Which workspace this app's source surface belongs to.
    pub workspace_id: u64,
    /// Committed geometry (logical px) — currently used for rendering.
    pub geometry: Option<Rectangle<i32, Logical>>,
    /// Pending geometry awaiting the client's next buffer commit.
    pub pending_geometry: Option<Rectangle<i32, Logical>>,
    /// When `pending_geometry` was set (for timeout-based force-commit).
    pub pending_since: Option<Instant>,
    pub visible: bool,
}

/// A renderable surface layer — toplevel or popup.
pub struct SurfaceLayer {
    pub surface: WlSurface,
    /// Where to render this layer's buffer origin, relative to the toplevel.
    /// Matches smithay's `Space::render_location()`: popup position minus the
    /// `xdg_surface.set_window_geometry` offset. The subtraction cancels GTK
    /// CSD shadow padding baked into the buffer so the visible window lands
    /// at the intended top-left.
    pub render_offset: Point<i32, Logical>,
}

impl AppWindow {
    /// Get the primary WlSurface. X clients reach us through
    /// xwayland-satellite as ordinary Wayland toplevels, so a single
    /// lookup suffices.
    pub fn wl_surface(&self) -> Option<WlSurface> {
        self.window.toplevel().map(|t| t.wl_surface().clone())
    }

    /// Collect the full surface stack: toplevel (offset=0,0) + all popups (recursive).
    pub fn surface_layers(&self) -> Vec<SurfaceLayer> {
        let Some(toplevel) = self.window.toplevel() else {
            return Vec::new();
        };
        let wl = toplevel.wl_surface();
        let wg = self.window.geometry().loc;
        let mut layers = vec![SurfaceLayer {
            surface: wl.clone(),
            render_offset: (-wg.x, -wg.y).into(),
        }];
        for (popup, offset) in PopupManager::popups_for_surface(wl) {
            let pg = popup.geometry().loc;
            layers.push(SurfaceLayer {
                surface: popup.wl_surface().clone(),
                render_offset: (offset.x - pg.x, offset.y - pg.y).into(),
            });
        }
        layers
    }
}

/// Tracks all live embedded application windows.
#[derive(Default)]
pub struct AppManager {
    windows: HashMap<u64, AppWindow>,
    next_id: u64,
}

impl AppManager {
    pub fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn insert(&mut self, app: AppWindow) {
        self.windows.insert(app.window_id, app);
    }

    pub fn remove(&mut self, window_id: u64) -> Option<AppWindow> {
        self.windows.remove(&window_id)
    }

    pub fn get(&self, window_id: u64) -> Option<&AppWindow> {
        self.windows.get(&window_id)
    }

    pub fn get_mut(&mut self, window_id: u64) -> Option<&mut AppWindow> {
        self.windows.get_mut(&window_id)
    }

    pub fn windows(&self) -> impl Iterator<Item = &AppWindow> {
        self.windows.values()
    }

    pub fn windows_mut(&mut self) -> impl Iterator<Item = &mut AppWindow> {
        self.windows.values_mut()
    }

    /// Find the window_id for a given Wayland surface (works for both Wayland and X11 windows).
    pub fn id_for_surface(&self, wl: &WlSurface) -> Option<u64> {
        self.windows
            .values()
            .find(|w| w.window.wl_surface().map(|s| &*s == wl).unwrap_or(false))
            .map(|w| w.window_id)
    }

    /// Find a mutable reference to the AppWindow for a given Wayland surface.
    pub fn get_mut_by_surface(&mut self, wl: &WlSurface) -> Option<&mut AppWindow> {
        self.windows
            .values_mut()
            .find(|w| w.window.wl_surface().map(|s| &*s == wl).unwrap_or(false))
    }

    /// Geometry of the embedded app owning `wl`, if any. Collapses
    /// `id_for_surface` + `get` + `.geometry` into one lookup.
    pub fn surface_geometry(&self, wl: &WlSurface) -> Option<Rectangle<i32, Logical>> {
        self.windows
            .values()
            .find(|w| w.window.wl_surface().map(|s| &*s == wl).unwrap_or(false))
            .and_then(|w| w.geometry)
    }

    /// Collect embedded app windows whose pending geometry has timed out.
    /// Returns (window_id, window, geo) for each; caller must `map_element`.
    pub fn collect_timed_out(
        &mut self,
        timeout: Duration,
    ) -> Vec<(u64, Window, Rectangle<i32, Logical>)> {
        let mut result = Vec::new();
        for app in self.windows.values_mut() {
            if let (Some(since), Some(pending)) = (app.pending_since, app.pending_geometry) {
                if since.elapsed() > timeout {
                    app.geometry = Some(pending);
                    app.pending_geometry = None;
                    app.pending_since = None;
                    result.push((app.window_id, app.window.clone(), pending));
                }
            }
        }
        result
    }

    /// Remove and return all windows whose Wayland surface has been destroyed.
    pub fn drain_dead(&mut self) -> Vec<AppWindow> {
        let dead_ids: Vec<u64> = self
            .windows
            .iter()
            .filter(|(_, w)| !w.window.alive())
            .map(|(id, _)| *id)
            .collect();
        dead_ids
            .into_iter()
            .filter_map(|id| self.windows.remove(&id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_id_returns_sequential_ids() {
        let mut mgr = AppManager::default();
        let id1 = mgr.alloc_id();
        let id2 = mgr.alloc_id();
        let id3 = mgr.alloc_id();
        assert_eq!(id2, id1 + 1);
        assert_eq!(id3, id2 + 1);
    }
}
