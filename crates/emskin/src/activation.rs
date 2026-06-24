use crate::util;
use crate::EmskinState;

/// If `XDG_ACTIVATION_TOKEN` (or `DESKTOP_STARTUP_ID`) is present in the
/// environment and the host compositor advertises
/// `xdg_activation_v1`, call `activate(token, main_surface)` to move
/// keyboard focus to emskin's main window. This is the protocol-legal
/// startup-notification path real Mutter / KWin both honour.
///
/// Does nothing on X11, when the token is absent, or when the host
/// lacks `xdg_activation_v1`. Runs as a short self-contained event
/// loop (bind → activate → roundtrip → drop).
pub fn activate_main_surface_if_env_token(state: &EmskinState) {
    use wayland_client::backend::{Backend, ObjectId};
    use wayland_client::protocol::wl_registry;
    use wayland_client::protocol::wl_surface::WlSurface;
    use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
    use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::{
        self, XdgActivationV1,
    };

    let Some(token) = std::env::var("XDG_ACTIVATION_TOKEN")
        .ok()
        .or_else(|| std::env::var("DESKTOP_STARTUP_ID").ok())
    else {
        return;
    };
    let Some(display_ptr) = util::host_wl_display_ptr(state) else {
        return;
    };
    let Some(surface_ptr) = util::host_wl_surface_ptr(state) else {
        return;
    };

    // SAFETY: display_ptr + surface_ptr come from winit's raw-window-handle,
    // both valid for at least the duration of this short sync.
    let backend = unsafe { Backend::from_foreign_display(display_ptr.cast()) };
    let connection = Connection::from_backend(backend);

    struct ActivationState {
        activation: Option<XdgActivationV1>,
    }
    impl Dispatch<wl_registry::WlRegistry, ()> for ActivationState {
        fn event(
            state: &mut Self,
            registry: &wl_registry::WlRegistry,
            event: wl_registry::Event,
            _: &(),
            _: &Connection,
            qh: &QueueHandle<Self>,
        ) {
            if let wl_registry::Event::Global {
                name,
                interface,
                version,
            } = event
            {
                if interface == "xdg_activation_v1" && state.activation.is_none() {
                    state.activation = Some(registry.bind(name, version.min(1), qh, ()));
                }
            }
        }
    }
    impl Dispatch<XdgActivationV1, ()> for ActivationState {
        fn event(
            _: &mut Self,
            _: &XdgActivationV1,
            _: xdg_activation_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    let mut queue = connection.new_event_queue::<ActivationState>();
    let qh = queue.handle();
    let _registry = connection.display().get_registry(&qh, ());
    let mut st = ActivationState { activation: None };

    if let Err(e) = queue.roundtrip(&mut st) {
        tracing::debug!("xdg_activation roundtrip failed: {e}");
        return;
    }

    let Some(activation) = st.activation.as_ref() else {
        tracing::debug!("host does not advertise xdg_activation_v1; skip self-activate");
        return;
    };

    // Wrap the raw wl_surface pointer from winit into a proxy on this
    // connection. SAFETY: surface_ptr is a live wl_surface proxy from
    // winit, and we only use this wrapped handle to issue one request
    // (`activate`) that doesn't destroy or mutate its state.
    let Ok(surface_id) =
        (unsafe { ObjectId::from_ptr(WlSurface::interface(), surface_ptr.cast()) })
    else {
        tracing::debug!("failed to wrap wl_surface ptr into proxy id");
        return;
    };
    let Ok(surface) = WlSurface::from_id(&connection, surface_id) else {
        tracing::debug!("failed to construct WlSurface proxy from id");
        return;
    };

    activation.activate(token.clone(), &surface);
    if let Err(e) = connection.flush() {
        tracing::warn!("xdg_activation flush failed: {e}");
    }
    let _ = queue.roundtrip(&mut st);
    tracing::info!(
        "requested self-activation via xdg_activation_v1 (token bytes={})",
        token.len()
    );
}
