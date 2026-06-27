# emthin Elisp Architecture Optimization — Step 1 (立项)

**Date:** 2026-06-26  
**Status:** Draft

## Motivation

The current Elisp side grew organically: `emthin-app.el` (418 lines) mixes geometry
math, IPC dispatch, lifecycle management, frame sync, and focus sync in one file.
The workspace and manage modules depend on `emthin-app` for functions that should
be their own concerns.

This refactoring is step 1 of a multi-step architecture optimization, referencing
CLFSWM v1212's patterns (float coordinate system, module separation, tree-walk
data flow) while adapting to Emacs Lisp conventions and the emthin design constraint
of `emthin-ipc.el` / `emthin-launch.el` being frozen.

## Design Constraints

- **`emthin-ipc.el`** — frozen, no modifications
- **`emthin-launch.el`** — frozen, no modifications
- **`emthin-connect.el`** — frozen, no modifications
- **No forward declarations** (`declare-function`)
- **No compiler magic** (`eval-when-compile`, compiler-macros)
- **Hook-based communication** between modules — dispatch defines hooks,
  consumers register handlers
- **Minimal behavior change** — this is a structural refactoring; IPC protocol
  and observable behavior remain identical

## Architecture

```
╔═══════════════════════════════════════════════════╗
║              Layer 1: Pure Data                   ║
║  emthin-geom.el                                   ║
║  ─ cl-defstruct emthin--rect                      ║
║  ─ px↔fl conversion, window-geometry collection   ║
║  ─ 0 deps, 0 hooks, 0 side effects                ║
╚═══════════════════════════════════════════════════╝
                      │
╔═══════════════════════════════════════════════════╗
║              Layer 2: Hook Routing                ║
║  emthin-dispatch.el                               ║
║  ─ Per-message hook variables (defvar)            ║
║  ─ emthin--dispatch: pcase → run-hook-with-args   ║
║  ─ 0 knowledge of app/sync/workspace modules      ║
╚═══════════════════════════════════════════════════╝
                      │
╔═══════════════════════════════════════════════════╗
║              Layer 3: EIEIO Lifecycle             ║
║  emthin-app.el                                    ║
║  ─ defclass emthin--app (window-id buffer         ║
║                         last-geometry)            ║
║  ─ window-created / destroyed / title-changed     ║
║  ─ Registered on dispatch.el hooks                ║
╚═══════════════════════════════════════════════════╝
                      │
╔═══════════════════════════════════════════════════╗
║              Layer 4: Side-effect Sync            ║
║  emthin-sync.el                                   ║
║  ─ sync-frame, sync-focus                        ║
║  ─ apply-geometry, apply-visible                 ║
║  ─ Emacs hooks (window-size-change-functions etc) ║
╚═══════════════════════════════════════════════════╝
                      │
╔═══════════════════════════════════════════════════╗
║  emthin-workspace.el   (adapted)                  ║
║  emthin-manage.el      (adapted)                  ║
║  emthin.el             (requires updated)         ║
╚═══════════════════════════════════════════════════╝
```

## Module Specifications

### 1. `emthin-geom.el` — Pure Geometry

**Depends on:** nothing (0 deps)  
**Exports:** struct, pure functions

```
cl-defstruct emthin--rect (x y w h :type float)
  Float rectangle relative to Emacs frame (0..1).

emthin--px->fl (px dim) → float
emthin--fl->px (fl dim) → integer
emthin--window-geometry (window &optional header-offset) → emthin--rect
  When HEADER-OFFSET is non-nil, adds it to the y-coordinate to
  compensate for GTK external bars (menu-bar/tool-bar).
emthin--rect-scale (rect fw fh) → (x y w h integer list)
emthin--rect-center (rect) → (cx . cy)
```

The conversion naming follows CLFSWM convention: `fl` = float, `px` = pixel.

### 2. `emthin-dispatch.el` — Hook Router

**Depends on:** `emthin-ipc` (for `emthin--message-hook`)  
**Exports:** per-message hook variables, dispatch function

Defines one `defvar` per IPC message type:

| Hook variable | Args |
|---|---|
| `emthin--connected-hook` | (version) |
| `emthin--error-hook` | (msg) |
| `emthin--window-created-hook` | (window-id title) |
| `emthin--window-destroyed-hook` | (window-id) |
| `emthin--title-changed-hook` | (window-id title) |
| `emthin--focus-view-hook` | (window-id) |
| `emthin--window-resized-hook` | (window-id x y w h) |
| `emthin--surface-size-hook` | (width height) |
| `emthin--workspace-created-hook` | (workspace-id) |
| `emthin--workspace-switched-hook` | (workspace-id) |
| `emthin--workspace-destroyed-hook` | (workspace-id) |

`emthin--dispatch` runs via `emthin--message-hook` and fans out to specific hooks.
No module beyond dispatch.el ever reads `emthin--message-hook` directly.

### 3. `emthin-app.el` — EIEIO Lifecycle

**Depends on:** `eieio`, `emthin-ipc`, `emthin-geom`  
**Exports:** `defclass emthin--app`, registry functions

```
defclass emthin--app
  slots: window-id, buffer, last-geometry, saved-geometry

defvar emthin--app-table         hash-table: window-id → app
emthin--find-app (window-id)     → app or nil
emthin--register-app (app)
emthin--unregister-app (window-id)

emthin--on-window-created (window-id title)       [hook handler]
emthin--on-window-destroyed (window-id)           [hook handler]
emthin--on-title-changed (window-id title)        [hook handler]
emthin--on-window-resized (window-id x y w h)     [hook handler]
  Updates the app's last-geometry slot — prevents
  stale geometry on next sync-frame.
emthin--kill-buffer-hook ()                       [buffer-local hook]

**Global state:** `emthin--header-offset`, `emthin--app-table`

**Registry access** — `emthin--find-app` is a public function callable
by sync.el (which depends on app.el) and manage.el (which depends on
app.el). This is NOT a hook violation: dispatch.el calls nothing,
sync.el reads state via direct function call.

Buffer-local variables currently in use (`emthin--window-id`, `emthin--visible`,
`emthin--last-geometry`) are **kept** for backward compatibility during transition —
new code should prefer `(oref app ...)` but existing consumers (manage.el, sync.el)
still read buffer-locals via `buffer-local-value`.

### 4. `emthin-sync.el` — Frame Sync

**Depends on:** `emthin-ipc`, `emthin-geom`, `emthin-app`  
**Exports:** sync functions

```
emthin--sync-frame (frame)                  [Emacs hook handler]
emthin--sync-focus (&optional frame)        [Emacs hook handler]
emthin--apply-geometry (app window)          [side-effect: send IPC]
emthin--apply-visible (wid now was)          [side-effect: send IPC]
emthin--wid-wins-data (frame)                [pure: wid→list-of-wins]
emthin--find-buffer (wid)                    [convenience]
```

**Registeres on:**
- `window-size-change-functions`
- `window-buffer-change-functions`
- `window-selection-change-functions`

**Global state:** `emthin--last-focused-wid`

### 5. `emthin-workspace.el` — Adapted

- Removes `emthin--handle-workspace-message` and its `add-hook` to
  `emthin--message-hook` — replaces with direct registration on
  `emthin--workspace-created/switched/destroyed-hook`
- Changes `(require 'emthin-app)` → `(require 'emthin-sync)`

### 6. `emthin-manage.el` — Adapted

- `make-emthin--rect` replaces `(emthin--rect 0.0 0.0 1.0 1.0)`
- Uses `emthin--apply-geometry` from sync.el instead of `emthin--report-geometry`
- Uses `emthin--rect-x/y/w/h` accessors on `emthin--rect` struct

### 7. `emthin.el` — Entry

Updated requires in execution order:

```elisp
(require 'emthin-connect)
(require 'emthin-ipc)
(require 'emthin-geom)
(require 'emthin-dispatch)
(require 'emthin-app)
(require 'emthin-sync)
(require 'emthin-workspace)
(require 'emthin-launch)
(require 'emthin-manage)
```

## Data Flow

```
Compositor IPC → emthin--message-hook
  → emthin--dispatch
    → emthin--window-created-hook  → emthin--on-window-created (app.el)
    → emthin--window-resized-hook  → emthin--on-window-resized (app.el)
    → emthin--workspace-switched-hook → emthin--on-workspace-switched (workspace.el)
    → ...

Emacs hook window-size-change-functions
  → emthin--sync-frame (sync.el)
    → emthin--wid-wins-data (pure collection)
    → emthin--apply-geometry → emthin--send
    → emthin--apply-visible  → emthin--send
    → emthin--sync-focus     → emthin--send
```

The hook-only rule applies only to `emthin-dispatch.el`: dispatch defines hooks
and calls nothing from other modules. Downstream modules (sync, workspace,
manage) may `require` and call each other's functions directly — this is normal
Elisp and avoids unnecessary indirection. The key constraint is that IPC message
routing is always hook-mediated, never hardcoded.

## Files Changed

| File | Action | Lines (est) |
|---|---|---|
| `emthin-geom.el` | **NEW** | ~80 |
| `emthin-dispatch.el` | **NEW** | ~100 |
| `emthin-sync.el` | **NEW** | ~120 |
| `emthin-app.el` | **REWRITE** | ~418 → ~200 |
| `emthin-workspace.el` | **EDIT** | ~20 changed |
| `emthin-manage.el` | **EDIT** | ~20 changed |
| `emthin.el` | **EDIT** | requires added |
| `emthin-ipc.el` | untouched | frozen |
| `emthin-connect.el` | untouched | frozen |
| `emthin-launch.el` | untouched | frozen |

## Testing

- Manual smoke test: launch emthin, verify app lifecycle (open/close/resize
  embedded apps), workspace switch, geometry sync all work identically
- No automated tests for Elisp currently exist; no test infrastructure change
  in this step

## Future Steps

After this structural refactoring, subsequent steps can:
- Introduce a proper frame-tree abstraction (CLFSWM-style tree-walk)
- Implement generic layout dispatch via `cl-defgeneric`
- Port workspace management to EIEIO `defclass`
- Add layout policies as separate modules
