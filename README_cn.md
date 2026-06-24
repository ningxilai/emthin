# emthin

[English](README.md)

emthin 是一个嵌套式 Wayland 合成器，将嵌入式应用托管在 Emacs 窗口中。
从 [emskin](https://github.com/emskin/emskin) 分支而来。

## 特性

- **嵌入 Wayland 应用** — 在 Emacs 窗口中运行浏览器、终端等
- **窗口镜像** — 同一应用显示在多个 Emacs 窗口
- **输入法** — 共享宿主输入法，精确定位光标
- **剪贴板** — 主机与嵌入程序双向同步
- **工作区** — 每个 Emacs frame 对应一个工作区

## 快速开始

```bash
emthin --standalone
```

自动加载内置 elisp，不影响你的 `~/.emacs.d/init.el`。

## 安装

```bash
cargo install --git https://github.com/ningxilai/emthin.git
```

需要 Rust ≥ 1.89。

## 使用

`M-x emthin-open-app` 启动嵌入式应用。

- `C-x o` — 切换窗口
- `C-x 5 2` — 新建工作区
- `C-x 5 o` — 切换工作区

## CLI 参数

```
emthin [OPTIONS]

  --standalone            独立模式，自动加载内置 elisp
  --fullscreen            启动时请求全屏
  --no-spawn              不启动 Emacs，等待外部连接
  --command <CMD>         启动命令 (默认: "emacs")
  --arg <ARG>             命令参数
  --ipc-path <PATH>       IPC socket 路径
  --wayland-socket <NAME> Wayland display socket 名
  --xkb-layout <LAYOUT>   键盘布局
  --xkb-model <MODEL>     键盘型号
  --xkb-variant <VAR>     布局变体
  --xkb-options <OPTS>    XKB 选项
  --log-file <PATH>       日志写入文件而非 stderr
  --dbus-isolated         私有 dbus-daemon
```

## 兼容性

| 宿主    | Wayland | X11 |
|---------|---------|-----|
| GNOME   | ✓       | ✓   |
| KDE     | ✓       | ✓   |
| Sway    | ✓       | n/a |
| COSMIC  | ✓       | n/a |
| niri    | ✓       | n/a |
| i3wm    | n/a     | ✓   |

## License

GPL-3.0
