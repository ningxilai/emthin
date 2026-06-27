# emthin Elisp Layout Generic Functions — Step 2

**Date:** 2026-06-26  
**Status:** Draft

## Motivation

After the structural refactoring (Step 1), layout computation is still implicit:
`emthin--window-geometry` always fills the Emacs window. Adding pluggable layout
policies requires a dispatch mechanism — `cl-defgeneric` specialized on layout
type objects, following CLFSWM's pattern.

## Design Constraints

- **No global state mutation during layout** — layout functions are pure:
  input (layout, window, app) → output emthin--rect
- **Per-app layout policy** — each `emthin--app` has a `layout` slot holding
  an instance of an `emthin-layout` subclass
- **Minimal code change** — only modify `emthin-sync.el`'s `apply-geometry`
  to call the generic function; keep `emthin--window-geometry` as the
  underlying window-edge → fraction collector
- **Follow CLFSWM** `defgeneric`/`defmethod` pattern

## Architecture

```
emthin-geom.el         ── window-edge → fraction (unchanged)
emthin-layout.el (NEW) ── defclass emthin-layout + subclasses
                          cl-defgeneric emthin--compute-layout
emthin-app.el    (EDIT) ── add `layout` slot to emthin--app
emthin-sync.el   (EDIT) ── apply-geometry calls compute-layout
```

## Module Specification

### `emthin-layout.el` (NEW)

```
(defclass emthin-layout () () :abstract t)
(defclass emthin-layout-fill (emthin-layout) ())
(defclass emthin-layout-float (emthin-layout)
  ((saved-rect :initform nil :type (or null emthin--rect))))

cl-defgeneric emthin--compute-layout (layout window app) → emthin--rect

cl-defmethod emthin--compute-layout ((layout emthin-layout-fill) window _app)
  → (emthin--window-geometry window (emthin--frame-header-offset))

cl-defmethod emthin--compute-layout ((layout emthin-layout-float) window _app)
  → saved-rect or fallback to fill

cl-defmethod emthin--compute-layout ((layout (eql nil)) window app)
  → fallback for nil layout (fill)
```

**Deps:** `emthin-geom` (for `emthin--window-geometry`, `emthin--rect`)

### `emthin-app.el` (EDIT)

Add `layout` slot to `emthin--app`:

```elisp
(defclass emthin--app ()
  (...existing slots...
   (layout :initform (make-instance 'emthin-layout-fill)
           :type emthin-layout))
  ...)
```

### `emthin-sync.el` (EDIT)

Change `emthin--apply-geometry` to take app object and call generic:

```elisp
;; was: (defun emthin--apply-geometry (window-id window) ...)
(defun emthin--apply-geometry (app window)
  (let* ((geo (emthin--compute-layout (oref app layout) window app))
         (old (oref app last-geometry)))
    (unless (equal geo old)
      (oset app last-geometry geo)
      (emthin--send 'set-geometry
        `(:window_id ,(oref app window-id)
          :x ,(emthin--rect-x geo)
          :y ,(emthin--rect-y geo)
          :w ,(emthin--rect-w geo)
          :h ,(emthin--rect-h geo))))))
```

Update callers in `emthin--sync-frame`:

```elisp
;; was: (emthin--apply-geometry wid (car wins))
;; new:
(emthin--apply-geometry app (car wins))
```

### `emthin.el` (EDIT)

Add `(require 'emthin-layout)` between geom and dispatch.

## Data Flow

```
sync-frame → wid-wins-data
  → for each (wid . wins)
    → find-app(wid) → app
    → apply-geometry(app, car(wins))
      → compute-layout(layout-slot, window, app)
        [generic dispatch on layout class]
        → emthin-layout-fill: window-geometry(window, header-offset)
        → emthin-layout-float: saved-rect or window-geometry
      → compare geo vs last-geometry (slot)
      → if changed: send set-geometry IPC
```

## Files Changed

| File | Action |
|---|---|
| `emthin-layout.el` | **NEW** (~40 lines) |
| `emthin-app.el` | **EDIT** — add `layout` slot (~5 lines) |
| `emthin-sync.el` | **EDIT** — change `apply-geometry` signature + callers (~10 lines) |
| `emthin.el` | **EDIT** — add require (~1 line) |
| `emthin-manage.el` | **EDIT** — update `emthin-manage-toggle-fullscreen` to use layout object (~5 lines) |
