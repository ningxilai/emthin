use clap::Parser;
use smithay::reexports::wayland_server::Display;

use emskin::{activation, cli::Cli, ipc, state, util, EmskinState};
use emskin_clipboard::{BackendHint, ClipboardBackend};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    util::init_logging(cli.log_file.as_deref());

    // --wayland-socket is plumbed through an env var so state.rs's
    // `init_wayland_listener` can stay signature-stable; CLI flag takes
    // precedence over a pre-set env var by overwriting it here.
    if let Some(ref name) = cli.wayland_socket {
        std::env::set_var("EMSKIN_WAYLAND_SOCKET_NAME", name);
    }

    let mut event_loop: smithay::reexports::calloop::EventLoop<'static, EmskinState> =
        smithay::reexports::calloop::EventLoop::try_new()?;

    let display: Display<EmskinState> = Display::new()?;

    let ipc_path = cli.ipc_path.clone().unwrap_or_else(util::default_ipc_path);
    tracing::info!("IPC socket path: {}", ipc_path.display());

    // xkbcommon treats "" as invalid (not "use default"), so when variant is
    // set but layout is empty we must supply a base layout explicitly.
    let xkb_layout = if cli.xkb_layout.is_empty() && !cli.xkb_variant.is_empty() {
        "us".to_string()
    } else {
        cli.xkb_layout.clone()
    };
    let xkb_config = smithay::input::keyboard::XkbConfig {
        layout: &xkb_layout,
        model: &cli.xkb_model,
        variant: &cli.xkb_variant,
        options: cli.xkb_options.clone(),
        ..Default::default()
    };

    let ipc = emskin::ipc::IpcServer::bind(ipc_path)?;
    let loop_handle = event_loop.handle();
    let mut state = EmskinState::new(&mut event_loop, loop_handle, display, ipc, xkb_config)?;

    register_ipc_source(&mut event_loop, &state)?;

    // Open a Wayland/X11 window for our nested compositor. Must happen
    // before clipboard init — the wl_data_device fallback piggybacks on
    // winit's host Wayland connection to get focused-client selection
    // events without needing our own host surface.
    emskin::winit::init_winit(&mut event_loop, &mut state, cli.fullscreen)?;

    // Claim keyboard focus on the host via xdg_activation_v1 if we
    // inherited an XDG_ACTIVATION_TOKEN / DESKTOP_STARTUP_ID — real
    // GNOME/KWin startup-notification path, and the only way to get
    // focus on hosts that don't auto-focus new toplevels (Mutter).
    // No-op if env is empty or host lacks xdg_activation_v1.
    activation::activate_main_surface_if_env_token(&state);

    // Initialize clipboard synchronization with host compositor.
    // Fallback chain: Wayland data-control (no-focus, preferred) →
    // wl_data_device via winit's shared connection (focus-gated) →
    // X11 selection (if host is Xorg).
    //
    // Test hook: `EMSKIN_DISABLE_HOST_CLIPBOARD=1` disables host clipboard
    // sync entirely. Kept as a safety valve for debugging; the E2E
    // harness doesn't need it anymore because each test gets its own
    // private host compositor (see `tests/common/mod.rs::NestedHost`).
    if std::env::var_os("EMSKIN_DISABLE_HOST_CLIPBOARD").is_none() {
        // Fallback chain: data-control (owns its own host connection, focus-free)
        // → wl_data_device on winit's shared connection (focus-gated) → X11
        // selection (only meaningful when the host is Xorg). See
        // emskin_clipboard::BackendHint for per-variant semantics.
        let mut hints: Vec<BackendHint> = vec![BackendHint::DataControl];
        if let Some(ptr) = util::host_wl_display_ptr(&state) {
            // SAFETY: the wl_display is owned by winit's backend, which
            // lives in `state.backend` for the entire compositor run. The
            // returned clipboard backend sits in `state.selection.clipboard`
            // on the same struct, so default field-drop order guarantees
            // the backend drops before the wl_display.
            hints.push(unsafe { BackendHint::wl_data_device(ptr) });
        }
        hints.push(BackendHint::X11);

        state.selection.clipboard = emskin_clipboard::init(&hints);
        if let Some(ref clipboard) = state.selection.clipboard {
            register_clipboard_source(&mut event_loop, clipboard.as_ref())?;
        }
    } else {
        tracing::info!("EMSKIN_DISABLE_HOST_CLIPBOARD set; host clipboard sync disabled");
    }

    // Bind the in-process DBus broker before any child processes; its
    // listen socket must exist by the time `inject_env` stamps
    // `DBUS_SESSION_BUS_ADDRESS` on `spawn_child`. A
    // missing or unparseable upstream bus downgrades the bridge to an
    // inert state — embedded IME popups then land wherever they always
    // did (no regression vs. pre-broker behavior).
    state.dbus = if cli.dbus_isolated {
        state::dbus::DbusBridge::init_isolated()
    } else {
        state::dbus::DbusBridge::init()
    };
    if state.dbus.broker.is_some() {
        register_dbus_listen_source(&event_loop, &state)?;
    }

    if !cli.no_spawn {
        state.xwayland.set_pending_command(state::PendingCommand {
            command: cli.command.clone(),
            args: cli.command_args.clone(),
            standalone: cli.standalone,
        });
    }

    start_xwayland_satellite(
        event_loop.handle(),
        &mut state,
        cli.xwayland_display.unwrap_or(0),
        &cli.xwayland_satellite_bin,
    );

    // Spawn the parked child (Emacs by default) regardless of whether
    // XWayland came up. Three buckets:
    //
    //   - satellite up → child sees `DISPLAY=:N` (emskin's nested X)
    //   - satellite missing, host has Xwayland → child inherits the
    //     host `DISPLAY` and X11-only programs (gtk3 Emacs on UOS
    //     etc.) fall back to the host X server. Windows render
    //     outside emskin, but at least the child has a GUI instead
    //     of dropping to TUI.
    //   - satellite missing AND host has no DISPLAY → child runs
    //     headless / TUI; nothing we can do without an X server.
    //
    // Previously this spawn lived inside `start_xwayland_satellite`'s
    // success path, which silently dropped the parked command on
    // systems missing `xwayland-satellite` and left the splash
    // spinning forever.
    if let Some(pc) = state.xwayland.take_pending_command() {
        let display = state.xwayland.display();
        if display.is_none() {
            if let Ok(host) = std::env::var("DISPLAY") {
                tracing::warn!(
                    "xwayland-satellite unavailable; child will inherit host \
                     DISPLAY={host}. X11 windows will render on the host X \
                     server (outside emskin)."
                );
            } else {
                tracing::warn!(
                    "xwayland-satellite unavailable and host has no DISPLAY; \
                     X11-only children will fall back to TUI / headless."
                );
            }
        }
        util::spawn_child(&pc.command, &pc.args, display, pc.standalone, &mut state);
    }

    // Launch the external workspace bar (if configured). Done after the
    // Wayland socket exists so the child inherits WAYLAND_DISPLAY via the
    // parent environment.
    event_loop.run(None, &mut state, emskin::tick::event_loop_tick)?;

    // Clean up Emacs child process
    if let Some(mut child) = state.emacs.take_child() {
        let _ = child.kill();
        let _ = child.wait();
    }
    // emskin-dbus-proxy: send Shutdown then reap so the bus socket gets
    // unlinked and the session dir is removed.
    state.dbus.shutdown();

    // Clean up extracted elisp files
    if let Some(ref dir) = state.elisp_dir {
        let _ = std::fs::remove_dir_all(dir);
    }

    Ok(())
}

fn register_ipc_source(
    event_loop: &mut smithay::reexports::calloop::EventLoop<EmskinState>,
    state: &EmskinState,
) -> Result<(), Box<dyn std::error::Error>> {
    use smithay::reexports::calloop::{generic::Generic, Interest, Mode, PostAction};
    use std::os::unix::io::FromRawFd;
    let listener_fd = state.ipc.listener_fd();
    // SAFETY: We duplicate the fd so the Generic source owns its own copy.
    // The original fd remains valid inside IpcServer for the lifetime of state.
    let dup_fd = unsafe { libc::dup(listener_fd) };
    if dup_fd < 0 {
        return Err("dup(ipc listener fd) failed".into());
    }
    // SAFETY: dup_fd is a valid open fd (dup succeeded above, dup_fd >= 0).
    // Ownership transfers to File; the original listener_fd stays open in IpcServer.
    let file = unsafe { std::fs::File::from_raw_fd(dup_fd) };
    event_loop
        .handle()
        .insert_source(
            Generic::new(file, Interest::READ, Mode::Level),
            |_, _, state| {
                state.ipc.accept();
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| format!("failed to register IPC listener: {e}"))?;
    Ok(())
}

/// Register the DBus broker's listen socket with calloop. Each accepted
/// connection fans out to two more Generic sources (`client → upstream`
/// and `upstream → client` pumps) via [`handle_dbus_accept`]. Assumes
/// `state.dbus.broker.is_some()` — caller is expected to guard.
fn register_dbus_listen_source(
    event_loop: &smithay::reexports::calloop::EventLoop<EmskinState>,
    state: &EmskinState,
) -> Result<(), Box<dyn std::error::Error>> {
    use smithay::reexports::calloop::{generic::Generic, Interest, Mode, PostAction};
    use std::os::unix::io::FromRawFd;

    let Some(broker) = state.dbus.broker.as_ref() else {
        return Ok(());
    };
    let listener_fd = broker.listener_fd();
    // SAFETY: we dup the listener fd so the Generic source owns its own
    // copy. The original lives inside `DbusBroker::listener` for the
    // entire compositor run.
    let dup_fd = unsafe { libc::dup(listener_fd) };
    if dup_fd < 0 {
        return Err("dup(dbus listener fd) failed".into());
    }
    // SAFETY: dup_fd was just produced by libc::dup, so it's open and
    // owned by us. Passing to File::from_raw_fd transfers ownership; the
    // original listener_fd stays open in DbusBroker.
    let file = unsafe { std::fs::File::from_raw_fd(dup_fd) };
    event_loop
        .handle()
        .insert_source(
            Generic::new(file, Interest::READ, Mode::Level),
            |_, _, state| {
                handle_dbus_accept(state);
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| format!("failed to register dbus listener: {e}"))?;
    Ok(())
}

/// Drain every pending accept on the broker's listen socket, and for
/// each accepted pair register one calloop source per fd. Level-
/// triggered so we technically only need to accept once, but a loop is
/// cheap and cuts wakeup count when multiple clients dial in one tick.
fn handle_dbus_accept(state: &mut EmskinState) {
    loop {
        let Some(broker) = state.dbus.broker.as_mut() else {
            return;
        };
        let accepted = match broker.accept_one() {
            Ok(Some(a)) => a,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(error = %e, "dbus broker: accept failed");
                return;
            }
        };
        register_dbus_connection(state, accepted);
    }
}

/// Register the two per-connection calloop sources for one accepted
/// `(client, upstream)` pair. Each source pumps one direction; on EOF
/// or I/O error, both sources are torn down and the broker's connection
/// state is freed.
fn register_dbus_connection(state: &mut EmskinState, accepted: emskin_dbus::ConnAccepted) {
    use emskin_dbus::PumpOutcome;
    use smithay::reexports::calloop::{generic::Generic, Interest, Mode, PostAction};
    use std::os::unix::io::FromRawFd;

    let id = accepted.id;
    let client_dup = unsafe { libc::dup(accepted.client_fd) };
    let upstream_dup = unsafe { libc::dup(accepted.upstream_fd) };
    if client_dup < 0 || upstream_dup < 0 {
        tracing::warn!(?id, "dbus broker: fd dup failed; dropping connection");
        if let Some(broker) = state.dbus.broker.as_mut() {
            broker.remove_connection(id);
        }
        if client_dup >= 0 {
            unsafe { libc::close(client_dup) };
        }
        if upstream_dup >= 0 {
            unsafe { libc::close(upstream_dup) };
        }
        return;
    }
    // SAFETY: both dup fds are open and owned by us. File::from_raw_fd
    // takes ownership; closing is handled when the Generic source is
    // removed and its File drops.
    let client_file = unsafe { std::fs::File::from_raw_fd(client_dup) };
    let upstream_file = unsafe { std::fs::File::from_raw_fd(upstream_dup) };

    let loop_handle = state.loop_handle.clone();

    let client_token = match loop_handle.insert_source(
        Generic::new(client_file, Interest::READ, Mode::Level),
        move |_, _, state| {
            let Some(broker) = state.dbus.broker.as_mut() else {
                return Ok(PostAction::Remove);
            };
            match broker.pump_client_to_upstream(id) {
                Ok(PumpOutcome::Active) => Ok(PostAction::Continue),
                Ok(PumpOutcome::PeerClosed) => {
                    drop_dbus_connection(state, id, DropSide::Client);
                    Ok(PostAction::Remove)
                }
                Err(e) => {
                    tracing::warn!(?id, error = %e, "dbus broker: c2u pump failed");
                    drop_dbus_connection(state, id, DropSide::Client);
                    Ok(PostAction::Remove)
                }
            }
        },
    ) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(?id, error = %e, "failed to register dbus client source");
            if let Some(broker) = state.dbus.broker.as_mut() {
                broker.remove_connection(id);
            }
            return;
        }
    };

    let upstream_token = match loop_handle.insert_source(
        Generic::new(upstream_file, Interest::READ, Mode::Level),
        move |_, _, state| {
            let Some(broker) = state.dbus.broker.as_mut() else {
                return Ok(PostAction::Remove);
            };
            match broker.pump_upstream_to_client(id) {
                Ok(PumpOutcome::Active) => Ok(PostAction::Continue),
                Ok(PumpOutcome::PeerClosed) => {
                    drop_dbus_connection(state, id, DropSide::Upstream);
                    Ok(PostAction::Remove)
                }
                Err(e) => {
                    tracing::warn!(?id, error = %e, "dbus broker: u2c pump failed");
                    drop_dbus_connection(state, id, DropSide::Upstream);
                    Ok(PostAction::Remove)
                }
            }
        },
    ) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(?id, error = %e, "failed to register dbus upstream source");
            state.loop_handle.remove(client_token);
            if let Some(broker) = state.dbus.broker.as_mut() {
                broker.remove_connection(id);
            }
            return;
        }
    };

    state
        .dbus
        .connection_tokens
        .insert(id, (client_token, upstream_token));
}

/// Identifies which side triggered the teardown. We're called from
/// inside one of the two per-connection callbacks — that callback will
/// return `PostAction::Remove` itself. This function removes the
/// *other* source explicitly, plus the broker's connection state.
#[derive(Debug, Clone, Copy)]
enum DropSide {
    Client,
    Upstream,
}

fn drop_dbus_connection(state: &mut EmskinState, id: emskin_dbus::ConnId, side: DropSide) {
    if let Some((client_token, upstream_token)) = state.dbus.connection_tokens.remove(&id) {
        match side {
            DropSide::Client => state.loop_handle.remove(upstream_token),
            DropSide::Upstream => state.loop_handle.remove(client_token),
        }
    }
    if let Some(broker) = state.dbus.broker.as_mut() {
        broker.remove_connection(id);
    }
}

/// niri-style xwayland-satellite integration.
///
/// emskin pre-binds the X11 display sockets and only spawns the external
/// `xwayland-satellite` process when an X11 client first connects.
/// satellite crashes are handled transparently: the spawner thread
/// observes the exit, sends `ToMain::Rearm` through a calloop channel,
/// and the main loop re-installs the socket watch.
fn start_xwayland_satellite(
    handle: smithay::reexports::calloop::LoopHandle<'static, EmskinState>,
    state: &mut EmskinState,
    display_start: u32,
    binary: &std::path::Path,
) {
    use emskin::xwayland_satellite::{
        setup_connection, test_ondemand, SpawnConfig, ToMain, XwlsIntegration,
    };
    use smithay::reexports::calloop::channel;

    // Niri pattern: probe the binary first. A missing / incompatible
    // satellite disables the XWayland integration rather than crashing
    // the compositor.
    if !test_ondemand(binary) {
        tracing::warn!(
            "xwayland-satellite at {} not available or lacks --test-listenfd-support; \
             XWayland disabled",
            binary.display()
        );
        return;
    }

    let sockets = match setup_connection(display_start) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("xwayland-satellite: failed to bind X11 sockets: {e}");
            return;
        }
    };
    let display = sockets.display;
    let display_name = sockets.display_name.clone();

    let Some(socket_name) = state.socket_name.to_str() else {
        tracing::error!(
            "xwayland-satellite: wayland socket name is not valid UTF-8; aborting setup"
        );
        return;
    };
    let spawn_cfg = SpawnConfig {
        binary: binary.to_path_buf(),
        wayland_socket: std::path::PathBuf::from(socket_name),
        xdg_runtime_dir: std::path::PathBuf::from(util::runtime_dir()),
    };

    let (tx, rx) = channel::channel::<ToMain>();
    state
        .xwayland
        .set_integration(XwlsIntegration::new(sockets, spawn_cfg, tx));

    // Rearm handler: when the spawner thread reports child exit, drain
    // pending connections and re-install the socket watch.
    let rearm_handle = handle.clone();
    if let Err(e) = handle.insert_source(rx, move |event, _, st| {
        if let channel::Event::Msg(ToMain::Rearm) = event {
            if let Some(x) = st.xwayland.integration_mut() {
                if let Err(e) = x.on_rearm(&rearm_handle) {
                    tracing::warn!("xwayland-satellite rearm failed: {e}");
                }
            }
        }
    }) {
        tracing::error!("xwayland-satellite: failed to install rearm channel: {e}");
        state.xwayland.clear_integration();
        return;
    }

    if let Err(e) = state
        .xwayland
        .integration_mut()
        .expect("set_integration above guarantees Some")
        .arm(&handle)
    {
        tracing::error!("xwayland-satellite: arm() failed: {e}");
        state.xwayland.clear_integration();
        return;
    }

    // Socket is ready — export DISPLAY and notify Emacs. First X client
    // connect will trigger the on-demand satellite spawn automatically.
    std::env::set_var("DISPLAY", &display_name);
    state.xwayland.set_display(display);
    state
        .ipc
        .send(ipc::OutgoingMessage::XWaylandReady { display });
    tracing::info!("xwayland-satellite: socket ready on {display_name}");

    // Pending child is spawned by `main` after this fn returns, so the
    // satellite-missing path (early return above) can still launch it
    // without DISPLAY.
}

fn register_clipboard_source(
    event_loop: &mut smithay::reexports::calloop::EventLoop<EmskinState>,
    clipboard: &dyn ClipboardBackend,
) -> Result<(), Box<dyn std::error::Error>> {
    use emskin_clipboard::Driver;
    use smithay::reexports::calloop::{generic::Generic, Interest, Mode, PostAction};
    use std::os::unix::io::{AsRawFd, FromRawFd};

    // Piggyback backends (wl_data_device on a foreign wl_display) are drained
    // every tick from tick.rs — no owned fd to register here.
    let raw_fd = match clipboard.driver() {
        Driver::OwnedFd(fd) => fd.as_raw_fd(),
        Driver::Piggyback => return Ok(()),
    };

    // SAFETY: dup() returns a valid fd that we transfer ownership to File.
    let dup_fd = unsafe { libc::dup(raw_fd) };
    if dup_fd < 0 {
        return Err("dup(clipboard connection fd) failed".into());
    }
    let file = unsafe { std::fs::File::from_raw_fd(dup_fd) };

    event_loop
        .handle()
        .insert_source(
            Generic::new(file, Interest::READ, Mode::Level),
            |_, _, state| {
                if let Some(ref mut clipboard) = state.selection.clipboard {
                    clipboard.dispatch();
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|e| format!("failed to register clipboard source: {e}"))?;
    Ok(())
}
