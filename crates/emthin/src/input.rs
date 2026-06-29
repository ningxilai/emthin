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
    utils::{IsAlive, SERIAL_COUNTER},
    wayland::seat::WaylandFocus,
};

use crate::state::EmthinState;

impl EmthinState {
    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let Some(keyboard) = self.seat.get_keyboard() else {
                    return;
                };
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);

                let (_is_wakeup, mods_changed) = keyboard.input_intercept(
                    self,
                    event.key_code(),
                    event.state(),
                    |_state, _modifiers, keysym_handle| {
                        let Some(sym) = keysym_handle.raw_latin_sym_or_raw_current_sym() else {
                            return false;
                        };
                        // Only intercept WakeUp for focus toggle.
                        sym.raw() == keysyms::KEY_XF86WakeUp
                    },
                );

                if _is_wakeup && event.state() == KeyState::Pressed {
                    self.handle_wakeup();
                    return;
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
                let delta = self.cursor.consume_raw_location(new_abs);
                let time_msec = event.time_msec();
                let new_under = self.surface_under(new_abs);

                let serial = SERIAL_COUNTER.next_serial();

                // Always emit relative motion — no-op for clients that
                // haven't bound zwp_relative_pointer_v1.
                pointer.relative_motion(
                    self,
                    new_under.clone(),
                    &RelativeMotionEvent {
                        delta,
                        delta_unaccel: delta,
                        utime: time_msec as u64 * 1000,
                    },
                );

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

                    // Left-click: check mirrors first, then embedded app surfaces.
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

    /// Toggle keyboard focus between Emacs and the last-focused embedded app.
    /// Called on WakeUp key press.
    fn handle_wakeup(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let serial = SERIAL_COUNTER.next_serial();
        let current = keyboard.current_focus();
        let focus_on_emacs = current == self.emacs_focus_target();

        if focus_on_emacs {
            if let Some(saved) = self.focus.last_app_focus.take() {
                if saved.alive() {
                    keyboard.set_focus(self, Some(saved), serial);
                }
            }
        } else {
            if let Some(ref cur) = current {
                self.focus.last_app_focus = Some(cur.clone());
            }
            if let Some(emacs) = self.emacs_focus_target() {
                keyboard.set_focus(self, Some(emacs), serial);
            }
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
