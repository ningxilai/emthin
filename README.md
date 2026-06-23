# emskin

[中文文档](README_cn.md)

emskin wraps Emacs inside a nested Wayland compositor so that Wayland applications can be embedded into Emacs windows.

## Features

- **Embed Wayland apps** — browsers, terminals, etc. inside Emacs windows
- **Window mirroring** — display the same app in multiple Emacs windows
- **Input method support** — shares the host IM with precise cursor positioning
- **Clipboard sync** — bidirectional between host and embedded apps
- **Launcher support** — rofi / wofi / zofi work out of the box
- **Automatic focus management** — new windows auto-focus; focus falls back on close

## Compatibility

Hosts we've actually tested emskin under. The columns indicate which
kind of desktop session you launched emskin from, **not** which kinds
of clients it can embed — emskin always embeds both Wayland and X11
clients (X11 via the external [`xwayland-satellite`] process, spawned
on demand when an X client connects), regardless of host. `n/a` just
means that compositor or window manager doesn't have a session of
that type to nest into.

[`xwayland-satellite`]: https://github.com/Supreeeme/xwayland-satellite

| Host    | Wayland session | X11 session |
|---------|-----------------|-------------|
| GNOME   | ✓               | ✓           |
| KDE     | ✓               | ✓           |
| Sway    | ✓               | n/a         |
| COSMIC  | ✓               | n/a         |
| niri    | ✓               | n/a         |
| i3wm    | n/a             | ✓           |

pgtk Emacs (`--with-pgtk`) is recommended. GTK3 X11 Emacs also works when `xwayland-satellite` is installed (it's spawned lazily the moment an X client first connects).

## Install

**Requires Rust ≥ 1.89** (`rust-toolchain.toml` pins 1.92.0). If your distro ships an older rustc, install via [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### Arch Linux (AUR)

```bash
yay -S emskin-bin
```

### From source

```bash
# Dependencies (Arch Linux)
sudo pacman -S wayland libxkbcommon mesa

# Optional: embed X11 applications via xwayland-satellite.
# Without it, X clients (gtk3 Emacs, xterm, …) can't be embedded
# inside emskin — they fall back to the host X server if one is
# running, otherwise to TUI. Everything else (pgtk Emacs, Wayland
# apps, clipboard, IME) works regardless.
#
# On Arch:
yay -S xwayland-satellite

# On distros that don't package it yet (UOS, older Debian, RHEL …),
# build from upstream — Rust toolchain only:
cargo install --git https://github.com/Supreeeme/xwayland-satellite.git

# Option 1: cargo install
cargo install --git https://github.com/emskin/emskin.git

# Option 2: build from source
git clone https://github.com/emskin/emskin.git
cd emskin && cargo build --release
```

## Quick Start

**Recommended.** `--standalone` is non-invasive:

```bash
emskin --standalone
```

What it does:

- Pre-loads the bundled `emskin.el` from `$XDG_RUNTIME_DIR/emskin-<pid>/elisp/` so you don't need a `(require 'emskin)` of your own.
- Your **`~/.emacs.d/init.el` still loads as usual** — no `-Q`, no `--no-init-file`.
- Cleans up the extracted elisp dir on exit.

The only edge case: if you separately cloned emskin and added `(require 'emskin')` from your own `load-path`, the bundled copy will have already been loaded and your `require` becomes a no-op. Developers working on emskin itself should skip `--standalone` and load the elisp manually (see [Emacs Configuration](#emacs-configuration)).

## Usage

### Open embedded apps

Inside Emacs running in emskin:

```
M-x emskin-open-app RET
```

The app embeds into the current Emacs window and receives keyboard focus.

### Keyboard interaction

When an embedded app has focus, keystrokes go directly to it. Emacs prefix keys (`C-x`, `C-c`, `M-x`) are intercepted and sent back to Emacs; focus restores automatically after the key sequence completes.

- `C-x o` — switch Emacs windows (embedded apps follow buffer switches)
- `C-x 1` / `C-x 2` / `C-x 3` — normal window operations; embedded apps resize automatically

### Workspaces

Each Emacs frame maps to a workspace:

- `C-x 5 2` — create workspace
- `C-x 5 o` — switch workspace
- `C-x 5 0` — close current workspace

### Launchers

Bind a key to launch rofi / zofi:

```elisp
;; zofi — a launcher designed for emskin, see https://github.com/emskin/zskins
(defun my/emskin-zofi ()
  (interactive)
  (start-process "zofi" nil "setsid" "zofi"))
(global-set-key (kbd "C-c z") #'my/emskin-zofi)

;; rofi
(defun my/emskin-rofi ()
  (interactive)
  (start-process "rofi" nil
                 "setsid" "rofi"
                 "-show" "combi"
                 "-combi-modi" "drun,ssh"
                 "-terminal" "foot"
                 "-show-icons" "-i"))
(global-set-key (kbd "C-c r") #'my/emskin-rofi)
```

## Emacs Configuration

Without `--standalone`, load the elisp manually:

```elisp
(add-to-list 'load-path "/path/to/emskin/elisp")
(require 'emskin)
```

## CLI Options

```
emskin [OPTIONS]

  --standalone            Standalone mode: auto-load built-in elisp
  --fullscreen            Request fullscreen for the host compositor window on startup
  --no-spawn              Don't start Emacs; wait for external connection
  --command <CMD>         Program to launch (default: "emacs")
  --arg <ARG>             Arguments for --command (repeatable)
  --ipc-path <PATH>       IPC socket path (default: $XDG_RUNTIME_DIR/emskin-<pid>.ipc)
  --wayland-socket <NAME> Pin Wayland display socket name (default: wayland-N, auto)
  --xkb-layout <LAYOUT>   Keyboard layout (e.g. "us", "cn")
  --xkb-model <MODEL>     Keyboard model (e.g. "pc105")
  --xkb-variant <VAR>     Layout variant (e.g. "nodeadkeys")
  --xkb-options <OPTS>    XKB options (e.g. "ctrl:nocaps")
  --log-file <PATH>       Write tracing logs to this file instead of stderr
  --dbus-isolated         Spawn a private dbus-daemon for embedded apps so portal
                          activations and GApplication single-instance stay inside
                          emskin (experimental; host notifications/tray/secrets
                          unreachable in this mode)
```

## FAQ

### `--dbus-isolated` notes for GNOME / KDE

`--dbus-isolated` spawns a private `dbus-daemon` so embedded apps stop
leaking out to the host compositor on portal / GApplication
activation. The mechanics are identical on GNOME and KDE (both speak
the same session bus), but the **trade-offs land differently**:

- **GNOME** — heaviest payoff: portal-mediated launches (`xdg-open`,
  GTK file dialogs) and GApplication single-instance apps now stay in
  emskin instead of attaching to the host shell. Loss: notifications
  fired from emskin children don't reach the GNOME notification panel,
  saved passwords in `gnome-keyring` are unreachable, and tray icons
  via `org.kde.StatusNotifierWatcher` won't appear in any extension
  that proxies host tray.
- **KDE Plasma** — same payoff for portal/GApplication. KDE users feel
  the tray loss more (more apps surface via `KStatusNotifierItem`),
  and `kwallet` becomes unreachable instead of `gnome-keyring`. KDE's
  own portal backend (`xdg-desktop-portal-kde`) activates locally
  inside emskin, so file dialogs match Plasma styling without leaking
  to host.

If you find a host service whose loss is unacceptable in your
workflow, the next step is per-name bridging or a local
activation-`.service` shim — file an issue.

### Crash on startup in a VM

emskin supports software rendering (llvmpipe), but older Mesa (< 21.0) may crash at high resolutions:

```bash
# Check renderer
glxinfo | grep "OpenGL renderer"

# If llvmpipe at high resolution, reduce it
xrandr --output Virtual-1 --mode 1920x1080
```

Make sure mesa is installed: `sudo pacman -S mesa mesa-utils` (Arch) or `sudo apt install mesa-utils` (Debian/Ubuntu).

## License

GPL-3.0
