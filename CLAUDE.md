# emskin workspace

Cargo workspace, three crates:

```
crates/
├── emskin/            # compositor binary, IPC, handlers/, tests/
├── emskin-clipboard/  # smithay-free host clipboard proxy (data-control / wl_data_device / X11)
└── emskin-dbus/       # DBus fcitx5 frontend for IME
elisp/                 # Emacs-side client, embedded via include_dir!
```

```
emskin      ──→  emskin-clipboard
       └──→  emskin-dbus
```

- `emskin-clipboard` **cannot** `use smithay` — it's a self-contained
  host clipboard proxy usable by any nested Wayland compositor. The
  smithay-aware glue (SelectionTarget ↔ SelectionKind mapping, XWM
  replay, async pipe drain for X11) lives in `emskin/src/clipboard_bridge.rs`.

Deeper per-crate notes live in each `crates/*/CLAUDE.md`.

## Invariants (every session)

1. **Compositor is self-adaptive via layer-shell.** Emacs's geometry is
   `EmskinState::usable_area() = LayerMap::non_exclusive_zone()`. Any
   layer-shell client declaring `exclusive_zone` shrinks it and
   `relayout_emacs()` pushes the new size.
2. **`crates/emskin/Cargo.toml` keeps literal `version`/`edition`/… values**
   because cargo-aur 0.x doesn't support `version.workspace = true`. Both
   this and root `[workspace.package].version` must bump together
   (`cargo release` handles both via `release.toml`).

## Testing

E2E tests each spawn their own private host compositor. Invoke directly
with cargo:

```
cargo test -p emskin
```

## See also

- `.claude/skills/emskin-patterns/SKILL.md` — commit conventions, co-change
  patterns, release flow, `chain_position` table, "when to look where"
  navigation. Loaded on demand when writing commits, adding plugins, or
  cutting releases.
- `CONTRIBUTING.md` — setup, local checks, PR flow for outside contributors.
