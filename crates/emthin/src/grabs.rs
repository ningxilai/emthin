//! Pointer grabs — `MoveDialogGrab` for dragging floating dialogs
//! and `ResizeGrab` for resizing embedded apps by their edges.

use crate::ipc::OutgoingMessage;
use crate::EmthinState;
use smithay::{
    desktop::Window,
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
        GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab,
        PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point, Size},
};

pub struct MoveDialogGrab {
    pub start_data: GrabStartData<EmthinState>,
    pub window: Window,
    pub initial_window_location: Point<i32, Logical>,
}

impl PointerGrab<EmthinState> for MoveDialogGrab {
    fn motion(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        // Suppress pointer focus while dragging — the dialog tracks
        // the pointer wholesale, no other surface should think it's
        // hovered.
        handle.motion(data, None, event);

        let delta = event.location - self.start_data.location;
        let new_location = self.initial_window_location.to_f64() + delta;
        data.workspace.active_space.map_element(
            self.window.clone(),
            new_location.to_i32_round::<i32>(),
            true,
        );
    }

    fn relative_motion(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut EmthinState, handle: &mut PointerInnerHandle<'_, EmthinState>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &GrabStartData<EmthinState> {
        &self.start_data
    }

    fn unset(&mut self, _data: &mut EmthinState) {}
}

/// Pointer grab for resizing an embedded app window via its edges.
/// Activated when the client sends an `xdg_toplevel.resize_request`.
/// On completion (button release), sends `WindowResized` IPC so Emacs
/// can update its layout tracking.
pub struct ResizeGrab {
    pub start_data: GrabStartData<EmthinState>,
    pub window: Window,
    pub window_id: u64,
    pub initial_location: Point<i32, Logical>,
    pub initial_size: Size<i32, Logical>,
    /// Tracks the last computed location during drag — updated on every
    /// motion event so that final IPC reflects the actual final geometry.
    pub current_location: Point<i32, Logical>,
    /// Tracks the last computed size during drag.
    pub current_size: Size<i32, Logical>,
    pub edges: ResizeEdge,
}

impl ResizeGrab {
    fn compute_geometry(&mut self, delta: Point<i32, Logical>) {
        self.current_location = self.initial_location;
        self.current_size = self.initial_size;

        // Determine which axes are being resized based on the edge.
        // ResizeEdge is a plain enum (not bitflags), so we match each
        // variant individually. Corner variants affect two axes.
        let (resize_left, resize_right, resize_top, resize_bottom) = match self.edges {
            ResizeEdge::Top => (false, false, true, false),
            ResizeEdge::Bottom => (false, false, false, true),
            ResizeEdge::Left => (true, false, false, false),
            ResizeEdge::Right => (false, true, false, false),
            ResizeEdge::TopLeft => (true, false, true, false),
            ResizeEdge::TopRight => (false, true, true, false),
            ResizeEdge::BottomLeft => (true, false, false, true),
            ResizeEdge::BottomRight => (false, true, false, true),
            ResizeEdge::None => (false, false, false, false),
            _ => (false, false, false, false),
        };

        if resize_left {
            let dx = delta.x.min(self.initial_size.w - 50);
            self.current_location.x = self.initial_location.x + dx;
            self.current_size.w = self.initial_size.w - dx;
        }
        if resize_top {
            let dy = delta.y.min(self.initial_size.h - 50);
            self.current_location.y = self.initial_location.y + dy;
            self.current_size.h = self.initial_size.h - dy;
        }
        if resize_right {
            self.current_size.w = (self.initial_size.w + delta.x).max(50);
        }
        if resize_bottom {
            self.current_size.h = (self.initial_size.h + delta.y).max(50);
        }
    }

    fn send_window_resized(&self, data: &mut EmthinState) {
        let rect = data.canvas_to_fraction(self.current_location, self.current_size);
        data.ipc.send(OutgoingMessage::WindowResized {
            window_id: self.window_id,
            rect,
        });
    }
}

impl PointerGrab<EmthinState> for ResizeGrab {
    fn motion(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);

        let delta = (event.location - self.start_data.location).to_i32_round();
        self.compute_geometry(delta);

        if let Some(toplevel) = self.window.toplevel() {
            toplevel.with_pending_state(|s| {
                s.size = Some(self.current_size);
                s.states.set(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::TiledLeft,
                );
                s.states.set(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::TiledRight,
                );
                s.states.set(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::TiledTop,
                );
                s.states.set(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::TiledBottom,
                );
            });
            toplevel.send_pending_configure();
        }
        data.workspace
            .active_space
            .map_element(self.window.clone(), self.current_location, true);
    }

    fn relative_motion(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);
        if handle.current_pressed().is_empty() {
            self.send_window_resized(data);
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        details: AxisFrame,
    ) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut EmthinState, handle: &mut PointerInnerHandle<'_, EmthinState>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut EmthinState,
        handle: &mut PointerInnerHandle<'_, EmthinState>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &GrabStartData<EmthinState> {
        &self.start_data
    }

    fn unset(&mut self, data: &mut EmthinState) {
        self.send_window_resized(data);
    }
}
