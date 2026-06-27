# Elisp Architecture Restructuring

> **For agentic workers:** REQUIRED SUB-SKILL: Use subagent-driven-development to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Restructure emthin's Elisp side by decomposing `emthin-app.el` (~418 lines) into smaller, focused modules following CLFSWM's pattern of one concern per file, without modifying `emthin-ipc.el` or `emthin-launch.el`.

**Architecture:** Extract 5 new modules from `emthin-app.el`:
- `emthin-core.el` — state vars, IPC helpers, rect accessors
- `emthin-geom.el` — geometry calculation (px→frac, window geometry)
- `emthin-life.el` — window lifecycle (pure, no sync calls)
- `emthin-sync.el` — frame/focus sync (depends on core + geom only)
- `emthin-dispatch.el` — IPC dispatch pcase table (orchestrates life + sync)

`emthin-app.el` becomes a thin loader that requires all new modules and provides `emthin-app` for backwards compatibility.

**Tech Stack:** Emacs Lisp, `cl-lib`, `pcase`, `defsubst`

---

### Task 1: Create `emthin-core.el`

**Files:**
- Create: `elisp/emthin-core.el`

**Responsibility:** Core state variables, workspace table helpers, rect accessors, px→frac conversion, IPC call helpers (`emthin--call`/`emthin--call*`), pending app target queue.

This file has zero dependencies on other emthin modules (only `emthin-ipc`).

- [ ] **Step 1: Write `emthin-core.el`**

Content is extracted verbatim from `emthin-app.el` lines 1-113 (state vars, workspace tables, pending targets, accessors, IPC helpers). No `require` beyond `cl-lib`, `subr-x`, `emthin-ipc`.

```elisp
;;; emthin-core.el --- Core state and IPC helpers for emthin  -*- lexical-binding: t; -*-

(require 'cl-lib)
(require 'subr-x)
(require 'emthin-ipc)

;; ---------------------------------------------------------------------------
;; Application state
;; ---------------------------------------------------------------------------

(defvar emthin--header-offset nil
  "Pixel height of external GTK bars (menu-bar + tool-bar).
Seeded once on the first compositor SurfaceSize event and kept
constant thereafter.")

(defvar-local emthin--window-id nil
  "emthin window_id for the embedded app in this buffer.")

(defvar-local emthin--visible nil
  "Whether this embedded app buffer is currently displayed in an Emacs window.")

(defvar-local emthin--last-geometry nil
  "Last geometry sent for this buffer's embedded app window, to skip no-op updates.")

(defvar emthin--last-focused-wid 'unset
  "Last window-id sent via set_focus IPC.  Used as change-detection guard.")

;; ---------------------------------------------------------------------------
;; Workspace tracking
;; ---------------------------------------------------------------------------

(defvar emthin--frame-workspace-table (make-hash-table :test 'eq)
  "Maps Emacs frame objects to compositor workspace IDs.")

(defvar emthin--ws-to-frame-table (make-hash-table :test 'eql)
  "Reverse mapping: workspace-id to frame.")

(defvar emthin--active-workspace-id nil
  "Currently active workspace ID in the compositor.")

(defun emthin--map-frame-to-workspace (frame workspace-id)
  "Map FRAME to WORKSPACE-ID in both forward and reverse tables."
  (puthash frame workspace-id emthin--frame-workspace-table)
  (puthash workspace-id frame emthin--ws-to-frame-table))

(defun emthin--unmap-frame (frame)
  "Remove FRAME from workspace tables.  Idempotent."
  (let ((ws-id (gethash frame emthin--frame-workspace-table)))
    (remhash frame emthin--frame-workspace-table)
    (when ws-id
      (remhash ws-id emthin--ws-to-frame-table))))

;; ---------------------------------------------------------------------------
;; App window target queue
;; ---------------------------------------------------------------------------

(defvar emthin--pending-app-targets nil
  "FIFO queue of windows reserved for newly created app buffers.")

(defun emthin--take-app-target-window ()
  "Return and dequeue the next live target window for an app."
  (let (target)
    (while (and emthin--pending-app-targets
                (not (window-live-p target)))
      (setq target (pop emthin--pending-app-targets)))
    (when (window-live-p target)
      target)))

;; ---------------------------------------------------------------------------
;; Data accessors
;; ---------------------------------------------------------------------------

(defsubst emthin--rect (x y w h)
  "Create a rect from X Y W H.  Values are floats (0..1) relative to frame."
  (list x y w h))

(defsubst emthin--rect-x (r) (nth 0 r))
(defsubst emthin--rect-y (r) (nth 1 r))
(defsubst emthin--rect-w (r) (nth 2 r))
(defsubst emthin--rect-h (r) (nth 3 r))

(defsubst emthin--px->frac (px dim)
  "Convert pixel PX to a fraction of DIM (frame pixel width/height)."
  (/ (float px) dim))

;; ---------------------------------------------------------------------------
;; IPC call helpers
;; ---------------------------------------------------------------------------

(defmacro emthin--call (method &rest plist)
  "Send a METHOD notification with alternating keyword-value PLIST."
  (declare (indent 1))
  `(emthin--send ',method (list ,@plist)))

(defun emthin--call* (method &rest plist)
  "Runtime version of `emthin--call' macro."
  (emthin--send method plist))

(provide 'emthin-core)
;;; emthin-core.el ends here
```

- [ ] **Step 2: Verify file was created**

Run: `ls -la elisp/emthin-core.el`
Expected: file exists, readable

---

### Task 2: Create `emthin-geom.el`

**Files:**
- Create: `elisp/emthin-geom.el`

**Responsibility:** Geometry calculation utilities that convert Emacs window coordinates to fractions.

- [ ] **Step 1: Write `emthin-geom.el`**

Extracted from `emthin-app.el` lines 287-319 (geometry reporting). Requires `emthin-core`.

```elisp
;;; emthin-geom.el --- Geometry utilities for emthin  -*- lexical-binding: t; -*-

(require 'emthin-core)

;; ---------------------------------------------------------------------------
;; Geometry helpers
;; ---------------------------------------------------------------------------

(defun emthin--frame-header-offset (&optional _frame)
  "Pixel height of external GTK bars (menu-bar + tool-bar).
Computed once when the compositor reports the surface size."
  (or emthin--header-offset 0))

(defun emthin--edges->rect (offset edges)
  "Convert pixel EDGES (X1 Y1 X2 Y2) to rect with header OFFSET."
  (emthin--rect (nth 0 edges)
                (+ (nth 1 edges) offset)
                (- (nth 2 edges) (nth 0 edges))
                (- (nth 3 edges) (nth 1 edges))))

(defun emthin--window-geometry (window)
  "Return rect (fractions 0..1 of frame) for Emacs WINDOW body area."
  (let* ((edges (window-body-pixel-edges window))
         (offset (emthin--frame-header-offset (window-frame window)))
         (x (nth 0 edges))
         (y (+ (nth 1 edges) offset))
         (w (- (nth 2 edges) (nth 0 edges)))
         (h (- (nth 3 edges) (nth 1 edges)))
         (fw (float (frame-pixel-width (window-frame window))))
         (fh (float (frame-pixel-height (window-frame window)))))
    (emthin--rect
     (emthin--px->frac x fw)
     (emthin--px->frac y fh)
     (emthin--px->frac w fw)
     (emthin--px->frac h fh))))

(provide 'emthin-geom)
;;; emthin-geom.el ends here
```

- [ ] **Step 2: Verify file was created**

Run: `ls -la elisp/emthin-geom.el`
Expected: file exists, readable

---

### Task 3: Create `emthin-life.el`

**Files:**
- Create: `elisp/emthin-life.el`

**Responsibility:** Pure window lifecycle — buffer creation, window display, title handling, kill-hooks, prefix hooks. Does NOT call sync functions (emthin--report-geometry, emthin--sync-focus) — those are orchestrated by the dispatch layer.

- [ ] **Step 1: Write `emthin-life.el`**

Extracted from `emthin-app.el` lines 166-285, with sync calls removed from `emthin--on-window-created` (moved to dispatch). Requires `emthin-core`.

```elisp
;;; emthin-life.el --- Window lifecycle for emthin  -*- lexical-binding: t; -*-

(require 'emthin-core)

;; ---------------------------------------------------------------------------
;; Buffer lookup
;; ---------------------------------------------------------------------------

(defun emthin--find-buffer (window-id)
  "Return the buffer whose `emthin--window-id' equals WINDOW-ID, or nil."
  (seq-find (lambda (buf)
              (equal (buffer-local-value 'emthin--window-id buf) window-id))
            (buffer-list)))

;; ---------------------------------------------------------------------------
;; Window creation
;; ---------------------------------------------------------------------------

(defun emthin--on-window-created (window-id title)
  "Create/display a buffer for the new embedded app."
  (condition-case err
      (let* ((buf-name (format "*emthin: %s*" (if (string-empty-p title) "app" title)))
             (buf (generate-new-buffer buf-name)))
        (with-current-buffer buf
          (setq-local emthin--window-id window-id)
          (setq-local mode-name "emthin")
          (setq-local buffer-read-only t)
          (setq-local left-fringe-width 0)
          (setq-local right-fringe-width 0)
          (setq-local left-margin-width 0)
          (setq-local right-margin-width 0)
          (setq-local cursor-type nil)
          (add-hook 'kill-buffer-hook #'emthin--kill-buffer-hook nil t)
          (add-hook 'post-command-hook #'emthin--post-command-prefix-done nil t))
        (let ((target (emthin--take-app-target-window)))
          (if target
              (set-window-buffer target buf)
            (display-buffer buf '((display-buffer-pop-up-window
                                    display-buffer-use-some-window)
                                   (inhibit-same-window . t)
                                   (reusable-frames . nil)))))
        (message "emthin: embedded app ready (id=%s)" window-id))
    (error
     (message "emthin: window-created error (id=%s): %s" window-id err))))

;; ---------------------------------------------------------------------------
;; Window destruction
;; ---------------------------------------------------------------------------

(defun emthin--on-window-destroyed (window-id)
  "Close all Emacs windows/buffer for WINDOW-ID and restore focus."
  (condition-case err
      (when-let* ((buf (emthin--find-buffer window-id)))
        (with-current-buffer buf
          (setq-local emthin--window-id nil))
        (dolist (win (reverse (get-buffer-window-list buf nil t)))
          (when (window-deletable-p win)
            (delete-window win)))
        (kill-buffer buf)
        (let ((next-wid (buffer-local-value 'emthin--window-id
                                            (window-buffer (selected-window)))))
          (if next-wid
              (emthin--call set-focus :window_id next-wid)
            (emthin--call set-focus)))
        (message "emthin: window %s destroyed" window-id))
    (error
     (message "emthin: window-destroyed error (id=%s): %s" window-id err))))

;; ---------------------------------------------------------------------------
;; Title updates
;; ---------------------------------------------------------------------------

(defun emthin--on-title-changed (window-id title)
  "Rename the embedded app buffer when the app title changes."
  (when-let* ((buf (emthin--find-buffer window-id)))
    (with-current-buffer buf
      (rename-buffer (format "*emthin: %s*" title) t))))

;; ---------------------------------------------------------------------------
;; Focus view
;; ---------------------------------------------------------------------------

(defun emthin--on-focus-view (window-id)
  "Select the Emacs window that corresponds to WINDOW-ID."
  (when-let* ((buf (emthin--find-buffer window-id))
              (target (get-buffer-window buf nil)))
    (when (window-live-p target)
      (select-window target))))

;; ---------------------------------------------------------------------------
;; Compositor-initiated resize
;; ---------------------------------------------------------------------------

(defun emthin--on-window-resized (window-id x y w h)
  "Handle a compositor-initiated resize of an embedded app.
Updates the buffer's `emthin--last-geometry' so Emacs doesn't
re-send stale geometry on the next sync-frame."
  (when-let* ((buf (emthin--find-buffer window-id)))
    (with-current-buffer buf
      (setq emthin--last-geometry (emthin--rect x y w h)))))

;; ---------------------------------------------------------------------------
;; Kill-buffer hook
;; ---------------------------------------------------------------------------

(defun emthin--kill-buffer-hook ()
  "Notify emthin to close the app when its Emacs buffer is killed."
  (condition-case err
      (when emthin--window-id
        (emthin--call close :window_id emthin--window-id))
    (error
     (message "emthin: kill-buffer-hook error: %s" err))))

;; ---------------------------------------------------------------------------
;; Prefix chord hooks
;; ---------------------------------------------------------------------------

(defun emthin--post-command-prefix-done ()
  "After a command completes in an embedded app buffer, ask the
compositor to restore keyboard focus to the embedded app."
  (condition-case err
      (when emthin--process
        (emthin--call prefix-done))
    (error
     (message "emthin: prefix-done error: %s" err))))

(defun emthin--post-command-prefix-clear ()
  "Clear the compositor's `prefix_active' flag after every Emacs
command, in any buffer."
  (condition-case err
      (when emthin--process
        (emthin--call prefix-clear))
    (error
     (message "emthin: prefix-clear error: %s" err))))

(add-hook 'post-command-hook #'emthin--post-command-prefix-clear)

(provide 'emthin-life)
;;; emthin-life.el ends here
```

- [ ] **Step 2: Verify file was created**

Run: `ls -la elisp/emthin-life.el`
Expected: file exists, readable

---

### Task 4: Create `emthin-sync.el`

**Files:**
- Create: `elisp/emthin-sync.el`

**Responsibility:** Frame and focus synchronization — `wid-wins-data`, `report-geometry`, `sync-frame`, `sync-focus`. Depends on `emthin-core` + `emthin-geom` only (no dependency on `emthin-life`).

- [ ] **Step 1: Write `emthin-sync.el`**

Extracted from `emthin-app.el` lines 321-415 (per-frame sync + focus sync). Requires `emthin-core` and `emthin-geom`.

```elisp
;;; emthin-sync.el --- Frame and focus sync for emthin  -*- lexical-binding: t; -*-

(require 'emthin-core)
(require 'emthin-geom)

;; ---------------------------------------------------------------------------
;; Per-frame sync helpers
;; ---------------------------------------------------------------------------

(defun emthin--wid-wins-data (frame)
  "Return hash-table wid→(win...) for FRAME (pure collection)."
  (let ((wid-wins (make-hash-table :test 'eql)))
    (dolist (win (window-list frame 'no-minibuf))
      (when-let* ((wid (buffer-local-value 'emthin--window-id
                                           (window-buffer win))))
        (puthash wid (cons win (gethash wid wid-wins)) wid-wins)))
    wid-wins))

(defun emthin--report-geometry (window-id window)
  "Send set_geometry for WINDOW-ID if geometry changed."
  (condition-case err
      (let* ((geo (emthin--window-geometry window))
             (buf (window-buffer window))
             (old-geo (buffer-local-value 'emthin--last-geometry buf)))
        (message "emthin: window %s geo=%s" window-id geo)
        (unless (equal geo old-geo)
          (with-current-buffer buf
            (setq emthin--last-geometry geo))
          (emthin--call* 'set-geometry
            :window_id window-id
            :x (emthin--rect-x geo)
            :y (emthin--rect-y geo)
            :w (emthin--rect-w geo)
            :h (emthin--rect-h geo))))
    (error
     (message "emthin: geometry error for window %s: %s" window-id err))))

;; ---------------------------------------------------------------------------
;; Per-frame sync (geometry + visibility)
;; ---------------------------------------------------------------------------

(defun emthin--sync-frame (frame)
  "Sync visibility and geometry for embedded app buffers in FRAME."
  (when emthin--process
    (let ((ws-id (gethash frame emthin--frame-workspace-table)))
      (when (eql ws-id emthin--active-workspace-id)
        (condition-case err
            (let ((wid-wins (emthin--wid-wins-data frame)))
              (maphash (lambda (_wid wins)
                         (dolist (win wins)
                           (set-window-scroll-bars win 0 nil 0 nil)
                           (set-window-fringes win 0 0)
                           (set-window-margins win 0 0)))
                       wid-wins)
              (dolist (buf (buffer-list))
                (when-let* ((wid (buffer-local-value 'emthin--window-id buf)))
                  (let* ((wins (gethash wid wid-wins))
                         (now-visible (and wins t))
                         (was-visible (buffer-local-value 'emthin--visible buf)))
                    (unless (eq now-visible was-visible)
                      (with-current-buffer buf
                        (setq emthin--visible now-visible))
                      (emthin--call* 'set-visibility
                                     :window_id wid
                                     :visible (if now-visible t :json-false)))
                    (when wins
                      (emthin--report-geometry wid (car wins)))))))
          (error
           (message "emthin: per-buffer sync error: %s" err)))
        (emthin--sync-focus frame)))))

(add-hook 'window-size-change-functions #'emthin--sync-frame)
(add-hook 'window-buffer-change-functions #'emthin--sync-frame)

;; ---------------------------------------------------------------------------
;; Focus sync
;; ---------------------------------------------------------------------------

(defun emthin--sync-focus (&optional _frame)
  "Sync focus if the focused window's app changed."
  (when emthin--process
    (condition-case err
        (let ((wid (buffer-local-value 'emthin--window-id
                                        (window-buffer (selected-window)))))
          (unless (eq wid emthin--last-focused-wid)
            (setq emthin--last-focused-wid wid)
            (if wid
                (emthin--call* 'set-focus :window_id wid)
              (emthin--call* 'set-focus))))
      (error
       (message "emthin: focus sync error: %s" err)))))

(add-hook 'window-selection-change-functions #'emthin--sync-focus)

(provide 'emthin-sync)
;;; emthin-sync.el ends here
```

- [ ] **Step 2: Verify file was created**

Run: `ls -la elisp/emthin-sync.el`
Expected: file exists, readable

---

### Task 5: Create `emthin-dispatch.el`

**Files:**
- Create: `elisp/emthin-dispatch.el`

**Responsibility:** IPC message dispatch — the `pcase` table that maps method symbols to handlers. Orchestrates life + sync functions.

- [ ] **Step 1: Write `emthin-dispatch.el`**

Extracted from `emthin-app.el` lines 115-164, with `surface_size` handlers calling `emthin--sync-frame` and `window_created` handler calling both `emthin--on-window-created` (life) then `emthin--report-geometry` + `emthin--sync-focus` (sync). Requires `emthin-core`, `emthin-life`, `emthin-sync`.

```elisp
;;; emthin-dispatch.el --- IPC message dispatch for emthin  -*- lexical-binding: t; -*-

(require 'emthin-core)
(require 'emthin-life)
(require 'emthin-sync)

;; ---------------------------------------------------------------------------
;; Message dispatch
;; ---------------------------------------------------------------------------

(defun emthin--dispatch (method params)
  "Dispatch a parsed METHOD with PARAMS plist from emthin."
  (pcase method
    ('connected
     (message "emthin: connected (version %s)"
              (or (plist-get params :version) "?"))
     (setq emthin--active-workspace-id 1)
     (emthin--map-frame-to-workspace (selected-frame) 1)
     (run-hooks 'emthin-connected-hook))
    ('error
     (message "emthin error: %s" (plist-get params :msg)))
    ('window_created
     (let* ((window-id (plist-get params :window_id))
            (title (or (plist-get params :title) "")))
       (emthin--on-window-created window-id title)
       (when-let* ((buf (emthin--find-buffer window-id))
                   (win (get-buffer-window buf t)))
         (set-window-scroll-bars win 0 nil 0 nil)
         (emthin--report-geometry window-id win))
       (emthin--sync-focus)))
    ('window_destroyed
     (emthin--on-window-destroyed (plist-get params :window_id)))
    ('title_changed
     (emthin--on-title-changed (plist-get params :window_id)
                                (or (plist-get params :title) "")))
    ('focus_view
     (emthin--on-focus-view (plist-get params :window_id)))
    ('window_resized
     (emthin--on-window-resized (plist-get params :window_id)
                                (plist-get params :x)
                                (plist-get params :y)
                                (plist-get params :w)
                                (plist-get params :h)))
    ('surface_size
     (let* ((w (plist-get params :width))
            (h (plist-get params :height))
            (frame-h (frame-pixel-height))
            (offset (or emthin--header-offset
                        (max 0 (- h frame-h)))))
       (setq emthin--header-offset offset)
       (message "emthin: surface=%sx%s bars=%dpx" w h offset)
       (dolist (frame (frame-list))
         (emthin--sync-frame frame))))
    ('x_wayland_ready
     nil)
    ((or 'workspace_created 'workspace_switched 'workspace_destroyed)
     nil)
    (_
     (message "emthin: unknown message type %s" method))))

(add-hook 'emthin--message-hook #'emthin--dispatch)

(provide 'emthin-dispatch)
;;; emthin-dispatch.el ends here
```

- [ ] **Step 2: Verify file was created**

Run: `ls -la elisp/emthin-dispatch.el`
Expected: file exists, readable

---

### Task 6: Rewrite `emthin-app.el` as loader

**Files:**
- Modify: `elisp/emthin-app.el` (replace full content with loader)

`emthin-app.el` becomes a thin backwards-compat loader that requires all new modules and provides `emthin-app`. This ensures `emthin-workspace.el` and `emthin-manage.el` continue to work without modification.

- [ ] **Step 1: Replace `emthin-app.el` with loader**

```elisp
;;; emthin-app.el --- Emacs↔emthin IPC client (module loader)  -*- lexical-binding: t; -*-

;; This file is kept for backwards compatibility.  It loads the
;; decomposed modules that implement the emthin Elisp client.

(require 'emthin-core)
(require 'emthin-geom)
(require 'emthin-life)
(require 'emthin-sync)
(require 'emthin-dispatch)

(provide 'emthin-app)
;;; emthin-app.el ends here
```

- [ ] **Step 2: Verify the file is correct**

Run: `wc -l elisp/emthin-app.el`
Expected: 14 lines (was 418)

---

### Task 7: Remove stale `.elc` files

**Files:**
- Delete: `elisp/emthin-app.elc`
- Delete: `elisp/emthin-connect.elc` (stale — likely not in sync)
- Delete: `elisp/emthin-ipc.elc` (stale)
- Delete: `elisp/emthin-workspace.elc` (stale)
- Delete: `elisp/emthin.elc` (stale)

- [ ] **Step 1: Delete all `.elc` files**

Run: `rm elisp/*.elc`

- [ ] **Step 2: Verify all `.elc` files are gone**

Run: `ls elisp/*.elc 2>&1 || echo "no .elc files found"`
Expected: "no .elc files found"
