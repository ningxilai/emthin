use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "emskin",
    version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("EMSKIN_GIT_SHA"), ")")
)]
pub struct Cli {
    /// Do not spawn a child process; wait for an external connection.
    #[arg(long)]
    pub no_spawn: bool,

    /// Program to launch (default: "emacs").
    #[arg(long, default_value = "emacs")]
    pub command: String,

    /// Arguments for --command.
    #[arg(long = "arg", num_args = 1)]
    pub command_args: Vec<String>,

    /// Explicit IPC socket path (default: $XDG_RUNTIME_DIR/emskin-<pid>.ipc).
    #[arg(long)]
    pub ipc_path: Option<std::path::PathBuf>,

    /// Pin the Wayland display socket name (default: auto-chosen wayland-N
    /// by smithay).
    #[arg(long)]
    pub wayland_socket: Option<String>,

    /// XKB keyboard layout (e.g. "us", "de", "cn").
    #[arg(long, default_value = "")]
    pub xkb_layout: String,

    /// XKB keyboard model (e.g. "pc105").
    #[arg(long, default_value = "")]
    pub xkb_model: String,

    /// XKB layout variant (e.g. "nodeadkeys").
    #[arg(long, default_value = "")]
    pub xkb_variant: String,

    /// XKB options (e.g. "ctrl:nocaps").
    #[arg(long)]
    pub xkb_options: Option<String>,

    /// Standalone mode: auto-load built-in elisp without user config.
    #[arg(long)]
    pub standalone: bool,

    /// Request fullscreen for the host compositor window on startup.
    #[arg(long)]
    pub fullscreen: bool,

    /// Write tracing logs to this file instead of stderr.
    #[arg(long)]
    pub log_file: Option<std::path::PathBuf>,

    /// Pin the XWayland DISPLAY number that emskin asks
    /// xwayland-satellite to claim.
    #[arg(long)]
    pub xwayland_display: Option<u32>,

    /// Path to the `xwayland-satellite` binary. Defaults to the binary
    /// found on `$PATH`.
    #[arg(long, default_value = "xwayland-satellite")]
    pub xwayland_satellite_bin: std::path::PathBuf,

    /// Spawn a private `dbus-daemon` for embedded apps and route the
    /// broker's upstream to it instead of the host session bus.
    #[arg(long)]
    pub dbus_isolated: bool,
}
