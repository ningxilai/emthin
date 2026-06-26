use std::time::Duration;
use winit_crate::platform::wayland::WindowAttributesExtWayland;

use smithay::{
    backend::{
        input::KeyState,
        renderer::{
            damage::OutputDamageTracker,
            element::{texture::TextureRenderElement, Id, Kind},
            gles::{GlesRenderer, GlesTexture},
            utils::{import_surface_tree, RendererSurfaceStateUserData},
            Renderer,
        },
        winit::{self, WinitEvent, WinitGraphicsBackend},
    },
    desktop::{space::render_output, Window},
    input::{
        keyboard::FilterResult,
        pointer::{CursorImageStatus, CursorImageSurfaceData},
    },
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::calloop::EventLoop,
    utils::{Physical, Rectangle, Size, Transform, SERIAL_COUNTER},
    wayland::compositor::with_states,
};

use crate::element::CustomElement;

use crate::EmthinState;

const REFRESH_RATE: i32 = 60_000;

fn make_mode(size: Size<i32, Physical>) -> Mode {
    Mode {
        size,
        refresh: REFRESH_RATE,
    }
}

fn apply_pending_state(state: &mut EmthinState, backend: &mut WinitGraphicsBackend<GlesRenderer>) {
    if let Some(title) = state.emacs.take_title() {
        backend.window().set_title(&title);
    }

    if let Some(fullscreen) = state.emacs.take_pending_fullscreen() {
        if fullscreen {
            backend
                .window()
                .set_fullscreen(Some(winit_crate::window::Fullscreen::Borderless(None)));
        } else {
            backend.window().set_fullscreen(None);
        }
    }

    if let Some(maximize) = state.emacs.take_pending_maximize() {
        backend.window().set_maximized(maximize);
    }

    // Sync the IME bridge's current state to winit. Computes
    // desired (cursor_area, ime_allowed) from `active_fcitx_ic` +
    // `tip_wants_ime` and diffs against what it last told winit —
    // see `ImeBridge::sync_to_winit` for the ordering-matters
    // reasoning.
    state.ime.sync_to_winit(backend.window());

    if let Some(status) = state.cursor.take_changed() {
        let window = backend.window();
        match status {
            CursorImageStatus::Named(icon) => {
                window.set_cursor_visible(true);
                window.set_cursor(winit_crate::window::Cursor::Icon(*icon));
            }
            // Surface cursors are software-rendered in render_frame();
            // hide the host cursor so it doesn't overlap.
            CursorImageStatus::Hidden | CursorImageStatus::Surface(_) => {
                window.set_cursor_visible(false);
            }
        }
    }
}

fn render_frame(
    state: &mut EmthinState,
    backend: &mut WinitGraphicsBackend<GlesRenderer>,
    output: &Output,
    damage_tracker: &mut OutputDamageTracker,
) {
    let size = backend.window_size();

    if output.current_mode().map(|m| m.size) != Some(size) {
        output.change_current_state(Some(make_mode(size)), None, None, None);
    }

    let damage = Rectangle::from_size(size);

    {
        let Ok((renderer, mut framebuffer)) = backend.bind() else {
            tracing::error!("Failed to bind rendering backend, skipping frame");
            return;
        };

        let scale = output.current_scale().fractional_scale();
        let mut extras: Vec<CustomElement<GlesRenderer>> = Vec::new();

        // Software cursor (topmost of extras): used for Surface cursors
        // (GTK3/Emacs) that can't be forwarded via winit's CursorIcon API.
        state.cursor.ensure_alive();
        if let CursorImageStatus::Surface(surface) = state.cursor.status() {
            if let Some(pointer) = state.seat.get_pointer() {
                if let Err(e) = import_surface_tree(renderer, surface) {
                    tracing::warn!("cursor import_surface_tree failed: {e:?}");
                } else {
                    let cursor_pos = pointer.current_location();
                    let ctx = renderer.context_id();
                    with_states(surface, |data| {
                        let hotspot = data
                            .data_map
                            .get::<CursorImageSurfaceData>()
                            .map(|d| d.lock().unwrap().hotspot)
                            .unwrap_or_default();
                        let Some(rss) = data.data_map.get::<RendererSurfaceStateUserData>() else {
                            return;
                        };
                        let rss = rss.lock().unwrap();
                        let Some(texture) = rss.texture::<GlesTexture>(ctx.clone()).cloned() else {
                            return;
                        };
                        let view = rss.view();
                        let pos = (cursor_pos - hotspot.to_f64()).to_physical(scale);
                        extras.push(
                            TextureRenderElement::from_static_texture(
                                Id::from_wayland_resource(surface),
                                ctx.clone(),
                                pos,
                                texture,
                                rss.buffer_scale(),
                                rss.buffer_transform(),
                                None, // alpha
                                view.map(|v| v.src),
                                view.map(|v| v.dst),
                                None, // damage
                                Kind::Cursor,
                            )
                            .into(),
                        );
                    });
                }
            }
        }

        if let Err(e) = render_output::<GlesRenderer, CustomElement<GlesRenderer>, Window, _>(
            output,
            renderer,
            &mut framebuffer,
            1.0,
            0,
            [&state.workspace.active_space],
            &extras,
            damage_tracker,
            [0.0, 0.0, 0.0, 0.0],
        ) {
            tracing::error!("render_output failed: {e}");
            return;
        }
    }

    if let Err(e) = backend.submit(Some(&[damage])) {
        tracing::error!("frame submit failed: {e}");
    }
}

fn post_render(state: &mut EmthinState, output: &Output) {
    state.workspace.active_space.elements().for_each(|window| {
        window.send_frame(
            output,
            state.start_time.elapsed(),
            Some(Duration::ZERO),
            |_, _| Some(output.clone()),
        )
    });

    state.workspace.active_space.refresh();
    state.wl.popups.cleanup();

    if let Err(e) = state.display_handle.flush_clients() {
        tracing::warn!("flush_clients failed: {}", e);
    }
}

pub fn init_winit(
    event_loop: &mut EventLoop<EmthinState>,
    state: &mut EmthinState,
    fullscreen: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let attributes = winit_crate::window::Window::default_attributes()
        .with_inner_size(winit_crate::dpi::LogicalSize::new(1280.0, 800.0))
        .with_title("Emacs")
        .with_visible(true)
        .with_name("emthin", "emthin");
    let (mut backend, winit) = winit::init_from_attributes(attributes)?;
    if fullscreen {
        backend
            .window()
            .set_fullscreen(Some(winit_crate::window::Fullscreen::Borderless(None)));
        state.emacs.request_fullscreen(true);
    } else {
        backend.window().set_maximized(true);
    }

    let mode = make_mode(backend.window_size());

    let output = Output::new(
        "winit".to_string(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Smithay".into(),
            model: "Winit".into(),
            serial_number: "Unknown".into(),
        },
    );
    let _global = output.create_global::<EmthinState>(&state.display_handle);
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(mode);

    state.workspace.active_space.map_output(&output, (0, 0));

    init_dmabuf(&mut backend, state);

    state.backend = Some(backend);

    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| {
            // Temporarily take backend out of state to avoid double &mut borrow
            // (the event handlers need &mut state AND &mut backend simultaneously).
            let mut backend = state
                .backend
                .take()
                .expect("backend missing in winit callback");
            match event {
                WinitEvent::Resized { size, scale_factor } => {
                    let int_scale = scale_factor.ceil() as i32;
                    tracing::info!(
                        "Host resized: {}x{} scale={} (int={})",
                        size.w,
                        size.h,
                        scale_factor,
                        int_scale
                    );
                    output.change_current_state(
                        Some(make_mode(size)),
                        None,
                        Some(Scale::Fractional(scale_factor)),
                        None,
                    );
                    if state.emacs.size_settled() {
                        // Re-lays out every Emacs frame against the fresh
                        // output size and broadcasts SurfaceSize — also
                        // sets needs_redraw.
                        state.relayout_emacs();
                    } else {
                        state.needs_redraw = true;
                    }
                }

                WinitEvent::Input(event) => {
                    state.process_input_event(event);
                    state.needs_redraw = true;
                }

                WinitEvent::Redraw => {
                    apply_pending_state(state, &mut backend);
                    // Clear needs_redraw BEFORE render_frame so `Effect::post_paint`
                    // (splash animation) can re-arm it for the next frame.
                    if state.needs_redraw {
                        state.needs_redraw = false;
                        render_frame(state, &mut backend, &output, &mut damage_tracker);
                    }
                    post_render(state, &output);
                    backend.window().request_redraw();
                }

                WinitEvent::CloseRequested => {
                    state.loop_signal.stop();
                }

                WinitEvent::Ime(event) => {
                    tracing::debug!("winit Ime event: {event:?}");

                    // Relay to the DBus fcitx5 active IC via router IPC so
                    // embedded clients (WeChat / Electron via
                    // GTK_IM_MODULE=fcitx) receive inline preedit + commit.
                    if let Some(ic_path) = state.ime.active_dbus_ic().map(|p| p.to_string()) {
                        match &event {
                            winit_crate::event::Ime::Commit(text) => {
                                state.dbus.send_rpc(&emthin_dbus::RouterRequest::ImeCommit {
                                    ic_path: ic_path.clone(),
                                    text: text.clone(),
                                });
                            }
                            winit_crate::event::Ime::Preedit(text, cursor) => {
                                let (cursor_begin, cursor_end) = cursor
                                    .map(|(b, e)| (b as i32, e as i32))
                                    .unwrap_or((-1, -1));
                                state
                                    .dbus
                                    .send_rpc(&emthin_dbus::RouterRequest::ImePreedit {
                                        ic_path,
                                        text: text.clone(),
                                        cursor_begin,
                                        cursor_end,
                                    });
                            }
                            _ => {}
                        }
                    }

                    // Then forward to the text_input_v3 path (for
                    // Wayland-native clients like Chromium with
                    // `--enable-wayland-ime` that bypass DBus).
                    state
                        .ime
                        .on_host_ime_event(event, &state.seat, &state.apps, backend.window());
                    state.needs_redraw = true;
                }

                WinitEvent::Focus(focused) => {
                    if focused {
                        state.on_focus_enter();
                        // Release all stuck keys to prevent phantom modifiers
                        // after Alt+Tab round-trip (the host eats the release).
                        let Some(keyboard) = state.seat.get_keyboard() else {
                            state.backend = Some(backend);
                            return;
                        };
                        let pressed = keyboard.pressed_keys();
                        if !pressed.is_empty() {
                            tracing::debug!(
                                "Window regained focus, releasing {} stuck keys",
                                pressed.len()
                            );
                            let time = state.start_time.elapsed().as_millis() as u32;
                            for keycode in pressed {
                                let serial = SERIAL_COUNTER.next_serial();
                                keyboard.input::<(), _>(
                                    state,
                                    keycode,
                                    KeyState::Released,
                                    serial,
                                    time,
                                    |_, _, _| FilterResult::Forward,
                                );
                            }
                        }
                    } else {
                        state.on_focus_leave();
                    }
                    state.needs_redraw = true;
                }
            };
            state.backend = Some(backend);
        })?;

    Ok(())
}

fn init_dmabuf(backend: &mut WinitGraphicsBackend<GlesRenderer>, state: &mut EmthinState) {
    use smithay::backend::{egl::EGLDevice, renderer::ImportDma};
    use smithay::wayland::dmabuf::DmabufFeedbackBuilder;

    let dmabuf_formats = backend.renderer().dmabuf_formats();

    let render_node = EGLDevice::device_for_display(backend.renderer().egl_context().display())
        .and_then(|device| device.try_get_render_node())
        .ok()
        .flatten();

    let global = match render_node.and_then(|node| {
        DmabufFeedbackBuilder::new(node.dev_id(), dmabuf_formats.clone())
            .build()
            .ok()
            .map(|fb| (node, fb))
    }) {
        Some((node, feedback)) => {
            tracing::info!("DMA-BUF v4 initialized (render node: {node:?})");
            state
                .wl
                .dmabuf_state
                .create_global_with_default_feedback::<EmthinState>(
                    &state.display_handle,
                    &feedback,
                )
        }
        None => {
            tracing::info!("DMA-BUF v3 initialized (no render node or feedback build failed)");
            state
                .wl
                .dmabuf_state
                .create_global::<EmthinState>(&state.display_handle, dmabuf_formats)
        }
    };
    state.wl.dmabuf_global = Some(global);
}
