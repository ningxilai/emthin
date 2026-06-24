use include_dir::{include_dir, Dir};

use crate::EmthinState;

static ELISP_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../elisp");

pub fn runtime_dir() -> String {
    std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string())
}

pub fn default_ipc_path() -> std::path::PathBuf {
    let pid = std::process::id();
    std::path::PathBuf::from(format!("{}/emthin-{pid}.ipc", runtime_dir()))
}

pub fn extract_embedded(src: &Dir<'_>, subdir: &str) -> Option<std::path::PathBuf> {
    let dest = std::path::PathBuf::from(format!(
        "{}/emthin-{}/{subdir}",
        runtime_dir(),
        std::process::id(),
    ));
    if let Err(e) = std::fs::create_dir_all(&dest) {
        tracing::error!("Failed to create {subdir} dir {}: {e}", dest.display());
        return None;
    }
    for file in src.files() {
        let out = dest.join(file.path());
        if let Err(e) = std::fs::write(&out, file.contents()) {
            tracing::error!("Failed to write {}: {e}", out.display());
            return None;
        }
    }
    tracing::info!("Extracted embedded {subdir} to {}", dest.display());
    Some(dest)
}

pub fn init_logging(log_file: Option<&std::path::Path>) {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    match log_file {
        Some(path) => match std::fs::File::create(path) {
            Ok(file) => tracing_subscriber::fmt()
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file))
                .with_env_filter(env_filter)
                .init(),
            Err(e) => eprintln!("failed to open --log-file {}: {e}", path.display()),
        },
        None => tracing_subscriber::fmt().with_env_filter(env_filter).init(),
    }
}

pub fn host_wl_display_ptr(state: &EmthinState) -> Option<*mut std::ffi::c_void> {
    use winit_crate::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    let backend = state.backend.as_ref()?;
    let handle = backend.window().display_handle().ok()?;
    match handle.as_raw() {
        RawDisplayHandle::Wayland(wl) => Some(wl.display.as_ptr()),
        _ => None,
    }
}

pub fn host_wl_surface_ptr(state: &EmthinState) -> Option<*mut std::ffi::c_void> {
    use winit_crate::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let backend = state.backend.as_ref()?;
    let handle = backend.window().window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Wayland(wl) => Some(wl.surface.as_ptr()),
        _ => None,
    }
}

pub fn spawn_child(
    command: &str,
    args: &[String],
    x_display: Option<u32>,
    standalone: bool,
    state: &mut EmthinState,
) {
    let Some(socket_name) = state.socket_name.to_str() else {
        tracing::error!("Wayland socket name is not valid UTF-8, cannot spawn child");
        return;
    };

    let mut full_args: Vec<String> = Vec::new();

    if standalone {
        if let Some(elisp_dir) = extract_embedded(&ELISP_DIR, "elisp") {
            full_args.push("--directory".to_string());
            full_args.push(elisp_dir.to_string_lossy().into_owned());
            full_args.push("-l".to_string());
            full_args.push("emthin".to_string());
            state.elisp_dir = Some(elisp_dir);
        }
    }

    full_args.extend_from_slice(args);

    let display_log = match x_display {
        Some(d) => format!(":{d}"),
        None => std::env::var("DISPLAY").unwrap_or_else(|_| "<unset>".to_string()),
    };
    tracing::info!(
        "Spawning: {command} {full_args:?} (WAYLAND_DISPLAY={socket_name} DISPLAY={display_log})"
    );
    let mut cmd = std::process::Command::new(command);
    cmd.args(&full_args)
        .env("WAYLAND_DISPLAY", socket_name)
        .env("XDG_SESSION_TYPE", "wayland")
        .env("XDG_SESSION_DESKTOP", "emthin");
    if let Some(d) = x_display {
        cmd.env("DISPLAY", format!(":{d}"));
    }
    state.dbus.inject_env(&mut cmd);
    match cmd.spawn() {
        Ok(child) => state.emacs.set_child(child),
        Err(e) => tracing::error!("Failed to spawn '{command}': {e}"),
    }
}
