use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, MouseButton, PointerAxisEvent, PointerButtonEvent,
    },
    input::{
        keyboard::keysyms,
        pointer::{AxisFrame, ButtonEvent, MotionEvent, RelativeMotionEvent},
    },
    reexports::wayland_server::Resource,
    utils::SERIAL_COUNTER,
    wayland::{
        pointer_constraints::{with_pointer_constraint, PointerConstraint},
        seat::WaylandFocus,
    },
};

use crate::state::EmthinState;

// XKB keycodes (evdev + 8, matching winit backend convention).
const KEYCODE_LCTRL: u32 = 37;
const KEYCODE_LALT: u32 = 64;
const KEYCODE_C: u32 = 54;
const KEYCODE_X: u32 = 53;
const KEYCODE_V: u32 = 55;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranslateOp {
    Copy,
    Cut,
    Paste,
}

impl EmthinState {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let Some(keyboard) = self.seat.get_keyboard() else {
                    return;
                };
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);

                let focus_on_emacs =
                    keyboard.current_focus() == self.emacs_focus_target();

                let mut translate_op = None;
                let (is_prefix, mods_changed) = keyboard.input_intercept(
                    self,
                    event.key_code(),
                    event.state(),
                    |_state, modifiers, keysym_handle| {
                        let Some(sym) = keysym_handle.raw_latin_sym_or_raw_current_sym() else {
                            return false;
                        };
                        let key = sym.raw();

                        if focus_on_emacs {
                            // Prefix keys (C-x, C-c, M-x) when Emacs
                            // has keyboard focus.
                            (modifiers.ctrl
                                && matches!(key, keysyms::KEY_x | keysyms::KEY_c))
                                || (modifiers.alt && key == keysyms::KEY_x)
                        } else {
                            // Emacs clipboard keybindings when an
                            // embedded app has keyboard focus.
                            if modifiers.alt && key == keysyms::KEY_w {
                                translate_op = Some(TranslateOp::Copy);
                                return true;
                            }
                            if modifiers.ctrl {
                                match key {
                                    keysyms::KEY_w => {
                                        translate_op = Some(TranslateOp::Cut);
                                        return true;
                                    }
                                    keysyms::KEY_y => {
                                        translate_op = Some(TranslateOp::Paste);
                                        return true;
                                    }
                                    _ => {}
                                }
                            }
                            false
                        }
                    },
                );

                // Clipboard injection — suppress original, synthesise
                // translated shortcut (press only).
                if is_prefix && !focus_on_emacs {
                    if event.state() == KeyState::Pressed {
                        match translate_op {
                            Some(TranslateOp::Copy) => {
                                keyboard.input_forward(
                                    self, KEYCODE_LALT.into(), KeyState::Released,
                                    serial, time, true,
                                );
                                keyboard.input_forward(
                                    self, KEYCODE_LCTRL.into(), KeyState::Pressed,
                                    serial, time, true,
                                );
                                keyboard.input_forward(
                                    self, KEYCODE_C.into(), KeyState::Pressed,
                                    serial, time, true,
                                );
                                keyboard.input_forward(
                                    self, KEYCODE_C.into(), KeyState::Released,
                                    serial, time, true,
                                );
                                keyboard.input_forward(
                                    self, KEYCODE_LCTRL.into(), KeyState::Released,
                                    serial, time, true,
                                );
                            }
                            Some(TranslateOp::Cut) => {
                                keyboard.input_forward(
                                    self, KEYCODE_X.into(), KeyState::Pressed,
                                    serial, time, true,
                                );
                                keyboard.input_forward(
                                    self, KEYCODE_X.into(), KeyState::Released,
                                    serial, time, true,
                                );
                            }
                            Some(TranslateOp::Paste) => {
                                keyboard.input_forward(
                                    self, KEYCODE_V.into(), KeyState::Pressed,
                                    serial, time, true,
                                );
                                keyboard.input_forward(
                                    self, KEYCODE_V.into(), KeyState::Released,
                                    serial, time, true,
                                );
                            }
                            None => {}
                        }
                    }
                    return;
                }

                // Emacs prefix chord.
                if is_prefix {
                    if !self.focus.is_active(crate::state::FocusOverride::Prefix) {
                        self.focus.enter(
                            crate::state::FocusOverride::Prefix,
                            keyboard.current_focus(),
                        );
                    }
                    self.ime.set_prefix_active(true);
                    if let Some(emacs) = self.emacs_focus_target() {
                        if keyboard.current_focus().as_ref() != Some(&emacs) {
                            keyboard.set_focus(self, Some(emacs), SERIAL_COUNTER.next_serial());
                        }
                    }
                }

                keyboard.input_forward(
                    self,
                    event.key_code(),
                    event.state(),
                    serial,
                    time,
                    mods_changed,
                );
            }

            // Smithay's winit backend never emits relative motion; we
            // synthesize a delta from successive absolutes in the
            // `PointerMotionAbsolute` arm below.
            InputEvent::PointerMotion { .. } => {}

            InputEvent::PointerMotionAbsolute { event, .. } => {
                let Some(output) = self.workspace.active_space.outputs().next() else {
                    return;
                };
                let Some(output_geo) = self.workspace.active_space.output_geometry(output) else {
                    return;
                };
                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };
                let new_abs = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
                // Diff against last raw host position to feed
                // zwp_relative_pointer_v1 — independent of
                // `pointer.current_location()`, which freezes under a
                // pointer lock while clients still need raw delta.
                let delta = self.cursor.consume_raw_location(new_abs);
                let time_msec = event.time_msec();
                let new_under = self.surface_under(new_abs);

                // Constraints attach per (surface, pointer) and only apply
                // while the pointer is focused on that surface — query the
                // focus directly instead of an extra surface_under(tracked)
                // walk.
                let constrained_surface = pointer.current_focus();
                let mut pointer_locked = false;
                let mut pointer_confined = false;
                let mut active_region = None;
                if let Some(surface) = constrained_surface.as_ref() {
                    with_pointer_constraint(surface, &pointer, |constraint| match constraint {
                        Some(c) if c.is_active() => match &*c {
                            PointerConstraint::Locked(l) => {
                                pointer_locked = true;
                                active_region = l.region().cloned();
                            }
                            PointerConstraint::Confined(c) => {
                                pointer_confined = true;
                                active_region = c.region().cloned();
                            }
                        },
                        _ => {}
                    });
                }

                // Rare: constraint restricts which sub-region of the surface
                // it applies to. Reuse `new_under`'s surface_loc when it
                // matches, otherwise pay for a second lookup.
                if let (Some(region), Some(surface)) =
                    (active_region.as_ref(), constrained_surface.as_ref())
                {
                    let tracked_loc = pointer.current_location();
                    let surface_loc = new_under
                        .as_ref()
                        .filter(|(s, _)| s == surface)
                        .map(|(_, loc)| *loc)
                        .or_else(|| {
                            self.surface_under(tracked_loc)
                                .filter(|(s, _)| s == surface)
                                .map(|(_, loc)| loc)
                        });
                    let in_region = surface_loc
                        .is_some_and(|loc| region.contains((tracked_loc - loc).to_i32_round()));
                    if !in_region {
                        pointer_locked = false;
                        pointer_confined = false;
                    }
                }

                // Always emit relative motion — no-op for clients that
                // haven't bound zwp_relative_pointer_v1, and the only signal
                // for clients that locked the pointer.
                pointer.relative_motion(
                    self,
                    new_under.clone(),
                    &RelativeMotionEvent {
                        delta,
                        delta_unaccel: delta,
                        utime: time_msec as u64 * 1000,
                    },
                );

                if pointer_locked {
                    pointer.frame(self);
                    return;
                }

                let serial = SERIAL_COUNTER.next_serial();

                if pointer_confined {
                    if let Some(surface) = constrained_surface.as_ref() {
                        let leaves_surface = new_under
                            .as_ref()
                            .map(|(s, _)| s != surface)
                            .unwrap_or(true);
                        let leaves_region = active_region.as_ref().is_some_and(|r| {
                            new_under
                                .as_ref()
                                .filter(|(s, _)| s == surface)
                                .is_some_and(|(_, loc)| {
                                    !r.contains((new_abs - *loc).to_i32_round())
                                })
                        });
                        if leaves_surface || leaves_region {
                            pointer.frame(self);
                            return;
                        }
                    }
                }

                if tracing::enabled!(tracing::Level::DEBUG) {
                    let new_id = new_under.as_ref().map(|(s, _)| s.id());
                    let old_id = pointer.current_focus().map(|s| s.id());
                    if new_id != old_id {
                        let loc = new_under.as_ref().map(|(_, p)| *p);
                        tracing::debug!(
                            "pointer focus change: {:?} -> {:?} pos=({:.0},{:.0}) loc={:?}",
                            old_id,
                            new_id,
                            new_abs.x,
                            new_abs.y,
                            loc,
                        );
                    }
                }

                pointer.motion(
                    self,
                    new_under.clone(),
                    &MotionEvent {
                        location: new_abs,
                        serial,
                        time: time_msec,
                    },
                );
                pointer.frame(self);

                // Smithay doesn't auto-activate — every surface enter must
                // be checked.
                if let Some((surface, surface_loc)) = new_under {
                    with_pointer_constraint(&surface, &pointer, |constraint| match constraint {
                        Some(c) if !c.is_active() => {
                            let point = (new_abs - surface_loc).to_i32_round();
                            if c.region().is_none_or(|r| r.contains(point)) {
                                c.activate();
                            }
                        }
                        _ => {}
                    });
                }
            }

            InputEvent::PointerButton { event, .. } => {
                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };
                let Some(keyboard) = self.seat.get_keyboard() else {
                    return;
                };

                let serial = SERIAL_COUNTER.next_serial();
                let button = event.button_code();
                let button_state = event.state();

                if ButtonState::Pressed == button_state && !pointer.is_grabbed() {
                    let pos = pointer.current_location();
                    let under = self.surface_under(pos);
                    tracing::debug!(
                        "button press: pos=({:.0},{:.0}) under={:?} ptr_focus={:?}",
                        pos.x,
                        pos.y,
                        under.as_ref().map(|(s, _)| s.id()),
                        pointer.current_focus().map(|s| s.id()),
                    );
                    let under_surface = under.map(|(s, _)| s);

                    // Left-click on an embedded app → tell Emacs to select that window.
                    if event.button() == Some(MouseButton::Left) {
                        if let Some((window_id, view_id, _)) =
                            self.apps.mirror_under(pos, self.workspace.active_id)
                        {
                            self.ipc.send(crate::ipc::OutgoingMessage::FocusView {
                                window_id,
                                view_id,
                            });
                        } else if let Some(window_id) = under_surface
                            .as_ref()
                            .and_then(|s| self.apps.id_for_surface(s))
                        {
                            self.ipc.send(crate::ipc::OutgoingMessage::FocusView {
                                window_id,
                                view_id: 0,
                            });
                        }
                    }

                    let focus = under_surface
                        .as_ref()
                        .and_then(|s| self.focus_target_for_surface(s))
                        .or_else(|| self.emacs_focus_target());

                    // Only change keyboard focus when clicking a different client.
                    // Clicking a popup surface from the same client (e.g. Firefox
                    // menu) must NOT send wl_keyboard.leave to the toplevel —
                    // otherwise the client dismisses the popup before processing
                    // the button event.
                    let same_client = focus.as_ref().is_some_and(|new| {
                        keyboard.current_focus().is_some_and(|old| {
                            new.wl_surface()
                                .is_some_and(|s| old.same_client_as(&s.id()))
                        })
                    });
                    if !same_client {
                        keyboard.set_focus(self, focus, serial);
                        self.focus.exit(crate::state::FocusOverride::Prefix);
                        // Mouse-click cancels any in-flight prefix
                        // chord — must also re-enable host IME.
                        self.ime.set_prefix_active(false);
                    }
                }

                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time: event.time_msec(),
                    },
                );
                pointer.frame(self);
            }

            InputEvent::PointerAxis { event, .. } => {
                let Some(pointer) = self.seat.get_pointer() else {
                    return;
                };
                let source = event.source();

                let horizontal_amount = event.amount(Axis::Horizontal).unwrap_or_else(|| {
                    event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.
                });
                let vertical_amount = event.amount(Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.
                });
                let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
                let vertical_amount_discrete = event.amount_v120(Axis::Vertical);

                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                if horizontal_amount != 0.0 {
                    frame = frame.value(Axis::Horizontal, horizontal_amount);
                    if let Some(discrete) = horizontal_amount_discrete {
                        frame = frame.v120(Axis::Horizontal, discrete as i32);
                    }
                }
                if vertical_amount != 0.0 {
                    frame = frame.value(Axis::Vertical, vertical_amount);
                    if let Some(discrete) = vertical_amount_discrete {
                        frame = frame.v120(Axis::Vertical, discrete as i32);
                    }
                }

                if source == AxisSource::Finger {
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                }

                pointer.axis(self, frame);
                pointer.frame(self);
            }

            _ => {}
        }
    }

    /// emthin's winit window lost focus (Alt+Tab away, minimize, etc.).
    /// Save the current keyboard focus and clear it so embedded clients
    /// stop thinking they still have focus. `focus_changed` cascades the
    /// clear to IME, data_device, and primary_selection. Pointer focus
    /// is released here too (bundled with keyboard until winit gives us
    /// separate `CursorLeft` events — YAGNI for now).
    pub fn on_focus_leave(&mut self) {
        let serial = SERIAL_COUNTER.next_serial();
        if let Some(keyboard) = self.seat.get_keyboard() {
            self.focus
                .enter(crate::state::FocusOverride::Host, keyboard.current_focus());
            keyboard.set_focus(self, None, serial);
        }
        if let Some(pointer) = self.seat.get_pointer() {
            pointer.motion(
                self,
                None,
                &MotionEvent {
                    location: pointer.current_location(),
                    serial,
                    time: 0,
                },
            );
            pointer.frame(self);
        }
    }

    /// emthin's winit window regained focus. Restore the keyboard focus
    /// saved by `on_focus_leave`; the `focus_changed` cascade re-enables
    /// IME (if the restored client has text_input_v3 bound) and rewires
    /// data_device / primary_selection.
    pub fn on_focus_enter(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let Some(Some(saved)) = self.focus.exit(crate::state::FocusOverride::Host) else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        keyboard.set_focus(self, Some(saved), serial);
    }
}
