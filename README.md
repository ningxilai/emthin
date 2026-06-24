# emthin

[中文文档](README_cn.md)

emthin is a [nested Wayland compositor](https://github.com/ningxilai/emthin) that hosts embedded applications inside Emacs windows. Forked from [emskin](https://github.com/emskin/emskin) with the upstream dependency on the host compositor for child window management removed.

## Features

- **Embed Wayland apps** — browsers, terminals, etc. inside Emacs windows
- **Window mirroring** — display the same app in multiple Emacs windows
- **Input method support** — shares the host IM with precise cursor positioning
- **Clipboard sync** — bidirectional between host and embedded apps
- **Workspaces** — each Emacs frame maps to a workspace

## Quick Start

```bash
emthin --standalone
```

Loads bundled elisp automatically; your `~/.emacs.d/init.el` still loads as usual.

## Install

```bash
cargo install --git https://github.com/ningxilai/emthin.git
```

Requires Rust ≥ 1.89.

## Usage

`M-x emthin-open-app` to launch an embedded app.

- `C-x o` — switch windows
- `C-x 5 2` — create workspace
- `C-x 5 o` — switch workspace

## CLI Options

```
emthin [OPTIONS]

  --standalone            Auto-load built-in elisp
  --fullscreen            Request fullscreen on startup
  --no-spawn              Don't start Emacs; wait for external connection
  --command <CMD>         Program to launch (default: "emacs")
  --arg <ARG>             Arguments for --command
  --ipc-path <PATH>       IPC socket path
  --wayland-socket <NAME> Pin Wayland display socket name
  --xkb-layout <LAYOUT>   Keyboard layout (e.g. "us", "cn")
  --xkb-model <MODEL>     Keyboard model (e.g. "pc105")
  --xkb-variant <VAR>     Layout variant
  --xkb-options <OPTS>    XKB options
  --log-file <PATH>       Write logs to file instead of stderr
  --dbus-isolated         Private dbus-daemon for embedded apps
```

## Compatibility

| Host    | Wayland session | X11 session |
|---------|-----------------|-------------|
| GNOME   | ✓               | ✓           |
| KDE     | ✓               | ✓           |
| Sway    | ✓               | n/a         |
| COSMIC  | ✓               | n/a         |
| niri    | ✓               | n/a         |
| i3wm    | n/a             | ✓           |

## License

GPL-3.0
