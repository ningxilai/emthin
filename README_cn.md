# emskin

> 让 Emacs 内能够使用 Wayland 应用

[English](README.md)

emskin 将 Emacs 包裹在一个嵌套的 Wayland 合成器中，让 Wayland 应用可以嵌入到 Emacs 窗口中。

## 特性

- **嵌入 Wayland 应用** — 浏览器、终端等直接显示在 Emacs 窗口内
- **窗口镜像** — 同一应用显示在多个 Emacs 窗口
- **输入法支持** — 共用宿主输入法，精确定位
- **剪贴板同步** — 主机与嵌入程序双向同步
- **启动器支持** — rofi / wofi / zofi 可直接使用
- **自动焦点管理** — 新窗口自动获焦，关闭后自动回退

## 兼容性

下表是我们实际测试通过的宿主环境。列指 emskin 是从哪种桌面会话启动的，
**不是**它能嵌入的客户端类型——emskin 始终同时支持嵌入 Wayland 和
X11 客户端（X11 经外部 [`xwayland-satellite`] 进程，在 X 客户端首次
连接时按需拉起），与宿主无关。`n/a` 表示该合成器或窗口管理器自身
没有这种类型的会话可供 emskin 嵌套。

[`xwayland-satellite`]: https://github.com/Supreeeme/xwayland-satellite

| 宿主    | Wayland 会话 | X11 会话 |
|---------|--------------|----------|
| GNOME   | ✓            | ✓        |
| KDE     | ✓            | ✓        |
| Sway    | ✓            | n/a      |
| COSMIC  | ✓            | n/a      |
| niri    | ✓            | n/a      |
| i3wm    | n/a          | ✓        |

推荐 pgtk Emacs（`--with-pgtk`）；安装了 `xwayland-satellite` 后 GTK3 X11 版 Emacs 也可以跑（satellite 在首个 X 客户端连接时按需拉起）。

## 安装

**需要 Rust ≥ 1.89**（`rust-toolchain.toml` 固定为 1.92.0）。如果发行版自带的 rustc 版本较旧，请通过 [rustup](https://rustup.rs/) 安装：

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 从源码构建

```bash
# 安装依赖（Arch Linux）
sudo pacman -S wayland libxkbcommon mesa

# 可选：通过 xwayland-satellite 嵌入 X11 应用。
# 不装也能跑——pgtk Emacs、Wayland 应用、剪切板、IME 全部正常；
# X 客户端（gtk3 Emacs、xterm 等）无法嵌入，会 fallback 到宿主
# X server（窗口画在 emskin 外面）或 TUI 模式。

# 没打包这个的发行版（UOS、老 Debian、RHEL 等），用 Rust 工具链
# 直接从上游编：
cargo install --git https://github.com/Supreeeme/xwayland-satellite.git

# 方式一：cargo install
cargo install --git https://github.com/emskin/emskin.git

# 方式二：手动编译
git clone https://github.com/emskin/emskin.git
cd emskin && cargo build --release
```

## 快速开始

**推荐方式。** `--standalone` 不侵入你现有的 Emacs 配置：

```bash
emskin --standalone
```

它做了什么：

- 从 `$XDG_RUNTIME_DIR/emskin-<pid>/elisp/` 预加载内置的 `emskin.el`，你不必自己写 `(require 'emskin)`。
- **`~/.emacs.d/init.el` 仍然按平常方式加载**——没有 `-Q`、没有 `--no-init-file`。
- 退出时清理临时 elisp 目录。

唯一的边界情况：如果你单独 clone 了 emskin 并在 init.el 里 `(require 'emskin)`，内置版本已经被预加载，你的 `require` 会变成 no-op；正在开发 emskin 本身的人请跳过 `--standalone`，改用手动加载（见 [Emacs 配置](#emacs-配置)）。

## 使用

### 打开嵌入程序

在 emskin 内的 Emacs 中：

`M-x emskin-open-app RET`

选择后自动嵌入当前 Emacs 窗口，并获得键盘焦点。

### 键盘交互

嵌入程序获焦时，键盘输入直接发送给它。Emacs 前缀键（`C-x`、`C-c`、`M-x`）会被自动拦截并送回 Emacs，完成组合键后焦点自动恢复。

- `C-x o` — 切换 Emacs 窗口（嵌入程序随 buffer 切换自动获焦）
- `C-x 1` / `C-x 2` / `C-x 3` — 正常的窗口操作，嵌入程序自动调整大小

### 工作区

每个 Emacs frame 对应一个工作区：

- `C-x 5 2` — 新建工作区
- `C-x 5 o` — 切换工作区
- `C-x 5 0` — 关闭当前工作区

### 使用启动器

`M-x emskin-open-app`

## Emacs 配置

不使用 `--standalone` 时，需要手动加载 elisp：

```elisp
(add-to-list 'load-path "/path/to/emskin/elisp")
(require 'emskin)
```

## CLI 参数

```
emskin [OPTIONS]

  --standalone            独立模式，自动加载内置 elisp（推荐初次体验）
  --fullscreen            启动时请求宿主 compositor 窗口全屏
  --no-spawn              不启动 Emacs，等待外部连接
  --command <CMD>         启动命令 (默认: "emacs")
  --arg <ARG>             命令参数 (可多次指定)
  --ipc-path <PATH>       IPC socket 路径 (默认: $XDG_RUNTIME_DIR/emskin-<pid>.ipc)
  --wayland-socket <NAME> 固定 Wayland display socket 名字 (默认: wayland-N, 自动)
  --xkb-layout <LAYOUT>   键盘布局 (例: "us", "cn")
  --xkb-model <MODEL>     键盘型号 (例: "pc105")
  --xkb-variant <VAR>     布局变体 (例: "nodeadkeys")
  --xkb-options <OPTS>    XKB 选项 (例: "ctrl:nocaps")
  --log-file <PATH>       将 tracing 日志写入文件而非 stderr
  --dbus-isolated         为嵌入应用启动私有 dbus-daemon，让 portal 激活与
                          GApplication 单实例都留在 emskin 内（实验性；该模式
                          下宿主通知 / 托盘 / 密钥环不可达）
```

## FAQ

### `--dbus-isolated` 在 GNOME / KDE 下的注意事项

`--dbus-isolated` 启动一个私有 `dbus-daemon`，让嵌入应用在 portal /
GApplication 激活时不再"漏"到宿主合成器。机制在 GNOME 和 KDE 上完全
相同（同一套 session bus、同一组协议），但**用户体感取舍不一样**：

- **GNOME** —— 收益最明显：portal 触发的启动（`xdg-open`、GTK 文件对
  话框）和 GApplication 单实例应用都留在 emskin 里，不再被宿主的
  shell 接走。代价：emskin 里应用发出的通知到不了 GNOME 通知面板，
  `gnome-keyring` 里保存的密码读不到，依赖
  `org.kde.StatusNotifierWatcher` 代理的托盘图标也不会出现。
- **KDE Plasma** —— portal/GApplication 收益相同。KDE 用户更容易感
  受到托盘缺失（更多应用通过 `KStatusNotifierItem` 暴露），密钥环失
  去的是 `kwallet` 而不是 `gnome-keyring`。KDE 自家的
  `xdg-desktop-portal-kde` 后端会在 emskin 内部本地激活，文件对话框
  仍是 Plasma 风格、不漏到宿主。

如果遇到某个宿主服务不能没有，下一步可以做按 well-known name 桥接，
或者写一个本地 activation `.service` shim ——欢迎开 issue。

### 虚拟机里启动后闪退

emskin 支持软件渲染（llvmpipe），但旧版本 Mesa（< 21.0）在高分辨率下可能崩溃。解决方法：

```bash
# 检查当前渲染器
glxinfo | grep "OpenGL renderer"

# 如果显示 llvmpipe 且分辨率过高，降低分辨率
xrandr --output Virtual-1 --mode 1920x1080
```

确保安装了 mesa：`sudo pacman -S mesa mesa-utils`（Arch）或 `sudo apt install mesa-utils`（Debian/Ubuntu）。

## License

GPL-3.0
