# Elisp Architecture Optimization — Step 1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split `emthin-app.el` into focused modules following CLFSWM-style separation of concerns (geom, dispatch, sync), introduce EIEIO `emthin--app` class, keep IPC/launch frozen.

**Architecture:** Four-layer hierarchy — geom (pure data, 0 deps) → dispatch (hook routing, 1 dep) → app (EIEIO lifecycle, 3 deps) → sync (side-effect frame sync, 3 deps). Workspace/manage adapted to new hooks/APIs.

**Tech Stack:** Emacs Lisp, EIEIO, cl-lib

---

### Task 1: Create `emthin-geom.el`

**Files:**
- Create: `elisp/emthin-geom.el`

- [ ] **Step 1: Write the file**

```elisp
;;; emthin-geom.el --- Pure geometry primitives for emthin  -*- lexical-binding: t; -*-

(require 'cl-lib)

(cl-defstruct emthin--rect
  "Float rectangle relative to Emacs frame (0..1)."
  (x 0.0 :type float)
  (y 0.0 :type float)
  (w 1.0 :type float)
  (h 1.0 :type float))

(defun emthin--px->fl (px dim)
  "Convert pixel PX to a fraction of DIM."
  (/ (float px) (float dim)))

(defun emthin--fl->px (fl dim)
  "Convert float FL to integer pixel within DIM."
  (round (* fl (float dim))))

(defun emthin--window-geometry (window &optional header-offset)
  "Return emthin--rect for WINDOW body area (fractions 0..1).
HEADER-OFFSET (pixels) is added to y to compensate for GTK external bars."
  (let* ((edges (window-body-pixel-edges window))
         (offset (or header-offset 0))
         (x (nth 0 edges))
         (y (+ (nth 1 edges) offset))
         (w (- (nth 2 edges) (nth 0 edges)))
         (h (- (nth 3 edges) (nth 1 edges)))
         (fw (float (frame-pixel-width (window-frame window))))
         (fh (float (frame-pixel-height (window-frame window)))))
    (make-emthin--rect
     :x (emthin--px->fl x fw)
     :y (emthin--px->fl y fh)
     :w (emthin--px->fl w fw)
     :h (emthin--px->fl h fh))))

(defun emthin--rect-scale (rect fw fh)
  "Convert RECT to pixel coordinates (list x y w h)."
  (list (emthin--fl->px (emthin--rect-x rect) fw)
        (emthin--fl->px (emthin--rect-y rect) fh)
        (emthin--fl->px (emthin--rect-w rect) fw)
        (emthin--fl->px (emthin--rect-h rect) fh)))

(defun emthin--rect-center (rect)
  "Return (CX . CY) center of RECT."
  (cons (+ (emthin--rect-x rect) (/ (emthin--rect-w rect) 2.0))
        (+ (emthin--rect-y rect) (/ (emthin--rect-h rect) 2.0))))

(provide 'emthin-geom)
;;; emthin-geom.el ends here
```

- [ ] **Step 2: Verify it loads**

Run: `emacs --batch --eval "(progn (push \"$(pwd)/elisp\" load-path) (require 'emthin-geom) (message \"OK\"))"`
Expected: prints "OK"

- [ ] **Step 3: Commit**

```bash
git add elisp/emthin-geom.el
git commit -m "feat(elisp): add emthin-geom.el with emthin--rect struct and conversion functions"
```

---

### Task 2: Create `emthin-dispatch.el`

**Files:**
- Create: `elisp/emthin-dispatch.el`

- [ ] **Step 1: Write the file**

```elisp
;;; emthin-dispatch.el --- IPC message dispatch for emthin  -*- lexical-binding: t; -*-

(require 'emthin-ipc)

;; ── Per-message hook variables ──

(defvar emthin--connected-hook nil
  "Hook run on compositor connected. Args: (version)")

(defvar emthin--error-hook nil
  "Hook run on compositor error. Args: (msg)")

(defvar emthin--window-created-hook nil
  "Hook run on new window. Args: (window-id title)")

(defvar emthin--window-destroyed-hook nil
  "Hook run on window destroyed. Args: (window-id)")

(defvar emthin--title-changed-hook nil
  "Hook run on window title change. Args: (window-id title)")

(defvar emthin--focus-view-hook nil
  "Hook run on compositor request to select window. Args: (window-id)")

(defvar emthin--window-resized-hook nil
  "Hook run on compositor-initiated resize. Args: (window-id x y w h)")

(defvar emthin--surface-size-hook nil
  "Hook run on surface size change. Args: (width height)")

(defvar emthin--workspace-created-hook nil
  "Hook run on workspace created. Args: (workspace-id)")

(defvar emthin--workspace-switched-hook nil
  "Hook run on workspace switched. Args: (workspace-id)")

(defvar emthin--workspace-destroyed-hook nil
  "Hook run on workspace destroyed. Args: (workspace-id)")

(defvar emthin--xwayland-ready-hook nil
  "Hook run on XWayland ready. Args: none")

;; ── Dispatch function ──

(defun emthin--dispatch (method params)
  "Dispatch incoming METHOD (symbol) with PARAMS plist to per-message hooks."
  (pcase method
    ('connected
     (run-hook-with-args 'emthin--connected-hook
       (or (plist-get params :version) "?")))
    ('error
     (run-hook-with-args 'emthin--error-hook
       (plist-get params :msg)))
    ('window_created
     (run-hook-with-args 'emthin--window-created-hook
       (plist-get params :window_id)
       (or (plist-get params :title) "")))
    ('window_destroyed
     (run-hook-with-args 'emthin--window-destroyed-hook
       (plist-get params :window_id)))
    ('title_changed
     (run-hook-with-args 'emthin--title-changed-hook
       (plist-get params :window_id)
       (or (plist-get params :title) "")))
    ('focus_view
     (run-hook-with-args 'emthin--focus-view-hook
       (plist-get params :window_id)))
    ('window_resized
     (run-hook-with-args 'emthin--window-resized-hook
       (plist-get params :window_id)
       (plist-get params :x)
       (plist-get params :y)
       (plist-get params :w)
       (plist-get params :h)))
    ('surface_size
     (run-hook-with-args 'emthin--surface-size-hook
       (plist-get params :width)
       (plist-get params :height)))
    ('x_wayland_ready
     (run-hook-with-args 'emthin--xwayland-ready-hook))
    ('workspace_created
     (run-hook-with-args 'emthin--workspace-created-hook
       (plist-get params :workspace_id)))
    ('workspace_switched
     (run-hook-with-args 'emthin--workspace-switched-hook
       (plist-get params :workspace_id)))
    ('workspace_destroyed
     (run-hook-with-args 'emthin--workspace-destroyed-hook
       (plist-get params :workspace_id)))
    (_
     (message "emthin: unknown message type %s" method))))

(add-hook 'emthin--message-hook #'emthin--dispatch)

(provide 'emthin-dispatch)
;;; emthin-dispatch.el ends here
```

- [ ] **Step 2: Verify it loads**

Run: `emacs --batch --eval "(progn (push \"$(pwd)/elisp\" load-path) (require 'emthin-dispatch) (message \"OK\"))"`
Expected: prints "OK"

- [ ] **Step 3: Commit**

```bash
git add elisp/emthin-dispatch.el
git commit -m "feat(elisp): add emthin-dispatch.el with per-message hook dispatch"
```

---

### Task 3: Rewrite `emthin-app.el` as EIEIO lifecycle

**Files:**
- Modify: `elisp/emthin-app.el`

- [ ] **Step 1: Replace entire file with EIEIO-based lifecycle**

```elisp
;;; emthin-app.el --- Embedded app lifecycle (EIEIO)  -*- lexical-binding: t; -*-

(require 'eieio)
(require 'cl-lib)
(require 'subr-x)
(require 'emthin-ipc)
(require 'emthin-geom)

;; ── Application object ──

(defclass emthin--app ()
  ((window-id  :initarg :window-id :type integer
               :documentation "Compositor-assigned window ID.")
   (buffer     :initarg :buffer   :type buffer
               :documentation "Emacs buffer for the embedded app.")
   (last-geometry :initform nil
                  :documentation "Last emthin--rect sent to compositor.")
   (saved-geometry :initform nil
                   :documentation "Geometry saved before fullscreen (manage.el)."))
  :documentation "An embedded application managed by emthin.")

;; ── Global state ──

(defvar emthin--header-offset nil
  "Pixel height of GTK menu-bar + tool-bar. Seeded once on first surface_size.")

(defvar emthin--app-table (make-hash-table :test 'eql)
  "window-id → emthin--app instance")

(defvar emthin--pending-app-targets nil
  "FIFO queue of windows reserved for newly created app buffers.")

;; ── Workspace state (kept here so sync.el can read them without circular dep) ──

(defvar emthin--frame-workspace-table (make-hash-table :test 'eq)
  "Maps Emacs frame objects to compositor workspace IDs.")

(defvar emthin--ws-to-frame-table (make-hash-table :test 'eql)
  "Reverse mapping: workspace-id to frame.")

(defvar emthin--active-workspace-id nil
  "Currently active workspace ID in the compositor.")

;; ── Registry ──

(defun emthin--find-app (window-id)
  "Return emthin--app for WINDOW-ID, or nil."
  (gethash window-id emthin--app-table))

(defun emthin--register-app (app)
  "Register APP instance in the global table."
  (puthash (oref app window-id) app emthin--app-table))

(defun emthin--unregister-app (window-id)
  "Remove WINDOW-ID from the global table."
  (remhash window-id emthin--app-table))

;; ── Target window queue ──

(defun emthin--take-app-target-window ()
  "Dequeue and return the next live target window for an app."
  (let (target)
    (while (and emthin--pending-app-targets
                (not (window-live-p target)))
      (setq target (pop emthin--pending-app-targets)))
    (when (window-live-p target)
      target)))

;; ── Header offset ──

(defun emthin--frame-header-offset (&optional _frame)
  "Pixel height of GTK bars.  0 if not yet seeded."
  (or emthin--header-offset 0))

;; ── Hook handlers (registered below) ──

(defun emthin--on-window-created (window-id title)
  "Create buffer and app object for new embedded window."
  (condition-case err
      (let* ((buf-name (format "*emthin: %s*"
                               (if (string-empty-p title) "app" title)))
             (buf (generate-new-buffer buf-name))
             (app (make-instance 'emthin--app
                    :window-id window-id :buffer buf)))
        (with-current-buffer buf
          (setq-local emthin--window-id window-id)
          (setq-local emthin--visible nil)
          (setq-local emthin--last-geometry nil)
          (setq-local mode-name "emthin")
          (setq-local buffer-read-only t)
          (setq-local left-fringe-width 0)
          (setq-local right-fringe-width 0)
          (setq-local left-margin-width 0)
          (setq-local right-margin-width 0)
          (setq-local cursor-type nil)
          (add-hook 'kill-buffer-hook #'emthin--kill-buffer-hook nil t)
          (add-hook 'post-command-hook #'emthin--post-command-prefix-done nil t))
        (emthin--register-app app)
        (let ((target (emthin--take-app-target-window)))
          (if target
              (set-window-buffer target buf)
            (display-buffer buf '((display-buffer-pop-up-window
                                   display-buffer-use-some-window)
                                  (inhibit-same-window . t)
                                  (reusable-frames . nil)))))
        (message "emthin: app ready (id=%s)" window-id))
    (error
     (message "emthin: window-created error (id=%s): %s" window-id err))))

(defun emthin--on-window-destroyed (window-id)
  "Close Emacs windows/buffer for WINDOW-ID and send focus restore."
  (condition-case err
      (when-let* ((app (emthin--find-app window-id)))
        (let ((buf (oref app buffer)))
          (dolist (win (reverse (get-buffer-window-list buf nil t)))
            (when (window-deletable-p win)
              (delete-window win)))
          (kill-buffer buf)
          (emthin--unregister-app window-id))
        (emthin--send 'set-focus nil)
        (message "emthin: window %s destroyed" window-id))
    (error
     (message "emthin: window-destroyed error (id=%s): %s" window-id err))))

(defun emthin--on-title-changed (window-id title)
  "Rename the embedded app buffer when the app title changes."
  (when-let* ((app (emthin--find-app window-id)))
    (with-current-buffer (oref app buffer)
      (rename-buffer (format "*emthin: %s*" title) t))))

(defun emthin--on-focus-view (window-id)
  "Select the Emacs window displaying WINDOW-ID."
  (when-let* ((app (emthin--find-app window-id))
              (buf (oref app buffer))
              (target (get-buffer-window buf nil)))
    (when (window-live-p target)
      (select-window target))))

(defun emthin--on-window-resized (window-id x y w h)
  "Update app's last-geometry from compositor-initiated resize."
  (when-let* ((app (emthin--find-app window-id)))
    (let ((geo (make-emthin--rect :x x :y y :w w :h h)))
      (oset app last-geometry geo)
      ;; Also set buffer-local for backward compat during transition.
      (with-current-buffer (oref app buffer)
        (setq-local emthin--last-geometry geo)))))

(defun emthin--on-surface-size (width height)
  "Record header offset from first surface size event."
  (let ((frame-h (frame-pixel-height))
        (offset (or emthin--header-offset
                    (max 0 (- height (frame-pixel-height))))))
    (setq emthin--header-offset offset)
    (message "emthin: surface=%sx%s bars=%dpx" width height offset)))

(defun emthin--on-connected (version)
  "Handle compositor connected event."
  (message "emthin: connected (version %s)" version)
  (run-hooks 'emthin-connected-hook))

;; ── Kill-buffer hook ──

(defun emthin--kill-buffer-hook ()
  "Notify emthin to close the app when its buffer is killed."
  (condition-case err
      (when (and (boundp 'emthin--window-id)
                 emthin--window-id)
        (emthin--send 'close `(:window_id ,emthin--window-id)))
    (error
     (message "emthin: kill-buffer-hook error: %s" err))))

;; ── Prefix hooks ──

(defun emthin--post-command-prefix-done ()
  "After a command in an app buffer, ask compositor to restore focus."
  (condition-case err
      (when emthin--process
        (emthin--send 'prefix-done))
    (error
     (message "emthin: prefix-done error: %s" err))))

(defun emthin--post-command-prefix-clear ()
  "After every command in any buffer, clear prefix_active flag."
  (condition-case err
      (when emthin--process
        (emthin--send 'prefix-clear))
    (error
     (message "emthin: prefix-clear error: %s" err))))

(add-hook 'post-command-hook #'emthin--post-command-prefix-clear)

;; ── Register on dispatch hooks ──

(add-hook 'emthin--connected-hook       #'emthin--on-connected)
(add-hook 'emthin--window-created-hook  #'emthin--on-window-created)
(add-hook 'emthin--window-destroyed-hook #'emthin--on-window-destroyed)
(add-hook 'emthin--title-changed-hook   #'emthin--on-title-changed)
(add-hook 'emthin--focus-view-hook      #'emthin--on-focus-view)
(add-hook 'emthin--window-resized-hook  #'emthin--on-window-resized)
(add-hook 'emthin--surface-size-hook    #'emthin--on-surface-size)

(provide 'emthin-app)
;;; emthin-app.el ends here
```

- [ ] **Step 2: Verify it loads**

Run: `emacs --batch --eval "(progn (push \"$(pwd)/elisp\" load-path) (require 'emthin-app) (message \"OK\"))"`
Expected: prints "OK"

- [ ] **Step 3: Commit**

```bash
git add elisp/emthin-app.el
git commit -m "feat(elisp): rewrite emthin-app.el as EIEIO lifecycle with dispatch hooks"
```

---

### Task 4: Create `emthin-sync.el`

**Files:**
- Create: `elisp/emthin-sync.el`
- Modify: `elisp/emthin-app.el` (add sync require — no, sync requires app)

- [ ] **Step 1: Write the file**

```elisp
;;; emthin-sync.el --- Frame sync pass for emthin  -*- lexical-binding: t; -*-

(require 'emthin-ipc)
(require 'emthin-geom)
(require 'emthin-app)

;; ── Global state ──

(defvar emthin--last-focused-wid 'unset
  "Last window-id sent via set_focus IPC.  Change-detection guard.")

;; ── Data collection ──

(defun emthin--wid-wins-data (frame)
  "Return hash-table wid→(win...) for FRAME (pure collection)."
  (let ((wid-wins (make-hash-table :test 'eql)))
    (dolist (win (window-list frame 'no-minibuf))
      (when-let* ((wid (buffer-local-value 'emthin--window-id
                                            (window-buffer win))))
        (puthash wid (cons win (gethash wid wid-wins)) wid-wins)))
    wid-wins))

;; ── Geometry apply ──

(defun emthin--apply-geometry (window-id window)
  "Send set_geometry for WINDOW-ID if geometry changed."
  (condition-case err
      (let* ((geo (emthin--window-geometry window
                    (emthin--frame-header-offset)))
             (buf (window-buffer window))
             (old-geo (buffer-local-value 'emthin--last-geometry buf)))
        (unless (equal geo old-geo)
          (with-current-buffer buf
            (setq-local emthin--last-geometry geo))
          (emthin--send 'set-geometry
            `(:window_id ,window-id
              :x ,(emthin--rect-x geo)
              :y ,(emthin--rect-y geo)
              :w ,(emthin--rect-w geo)
              :h ,(emthin--rect-h geo)))))
    (error
     (message "emthin: geometry error for window %s: %s" window-id err))))

;; ── Visibility apply ──

(defun emthin--apply-visible (window-id now-visible was-visible)
  "Send set_visibility if visibility changed."
  (unless (eq now-visible was-visible)
    (when-let* ((buf (emthin--find-buffer window-id)))
      (with-current-buffer buf
        (setq-local emthin--visible now-visible)))
    (emthin--send 'set-visibility
      `(:window_id ,window-id
        :visible ,(if now-visible t :json-false)))))

;; ── Frame sync ──

(defun emthin--sync-frame (frame)
  "Sync visibility and geometry for embedded app buffers in FRAME.
Only processes the active workspace's frame."
  (when emthin--process
    (let ((ws-id (gethash frame emthin--frame-workspace-table)))
      (when (eql ws-id emthin--active-workspace-id)
        (condition-case err
            (let ((wid-wins (emthin--wid-wins-data frame)))
              (maphash (lambda (wid wins)
                         (let ((now-visible (and wins t))
                               (was-visible
                                (when-let* ((app (emthin--find-app wid)))
                                  (buffer-local-value
                                   'emthin--visible (oref app buffer)))))
                           (emthin--apply-visible wid now-visible was-visible)
                           (when wins
                             (emthin--apply-geometry wid (car wins)))))
                       wid-wins))
          (error
           (message "emthin: sync-frame error: %s" err)))
        (emthin--sync-focus frame)))))

(add-hook 'window-size-change-functions  #'emthin--sync-frame)
(add-hook 'window-buffer-change-functions #'emthin--sync-frame)

;; ── Focus sync ──

(defun emthin--sync-focus (&optional _frame)
  "Sync focus if the focused window's app changed."
  (when emthin--process
    (condition-case err
        (let ((wid (buffer-local-value 'emthin--window-id
                                        (window-buffer (selected-window)))))
          (unless (eq wid emthin--last-focused-wid)
            (setq emthin--last-focused-wid wid)
            (if wid
                (emthin--send 'set-focus `(:window_id ,wid))
              (emthin--send 'set-focus))))
      (error
       (message "emthin: focus sync error: %s" err)))))

(add-hook 'window-selection-change-functions #'emthin--sync-focus)

(provide 'emthin-sync)
;;; emthin-sync.el ends here
```

- [ ] **Step 2: Verify it loads**

Run: `emacs --batch --eval "(progn (push \"$(pwd)/elisp\" load-path) (require 'emthin-sync) (message \"OK\"))"`
Expected: prints "OK"

- [ ] **Step 3: Commit**

```bash
git add elisp/emthin-sync.el
git commit -m "feat(elisp): add emthin-sync.el with frame sync and focus sync"
```

---

### Task 5: Adapt `emthin-workspace.el`

**Files:**
- Modify: `elisp/emthin-workspace.el`

- [ ] **Step 1: Replace workspace IPC message handler with dispatch hooks**

Changes:
1. Remove `emthin--handle-workspace-message` and its `add-hook` to `emthin--message-hook`
2. Change `(require 'emthin-app)` → `(require 'emthin-sync)`
3. Register `emthin--on-workspace-created/switched/destroyed` directly on dispatch hooks

```elisp
;;; emthin-workspace.el --- Workspace management for emthin  -*- lexical-binding: t; -*-

(require 'emthin-sync)
(require 'emthin-ipc)

;; ── Workspace-local state ──
;; emthin--frame-workspace-table, emthin--ws-to-frame-table, and
;; emthin--active-workspace-id are defined in emthin-app.el (so
;; emthin-sync.el can read them without circular dependency).

(defvar emthin--pending-frame-queue nil
  "Frames awaiting workspace_created IPC confirmation (FIFO).")

(defvar emthin--workspace-switch-suppressed nil
  "When non-nil, suppress workspace switch from after-focus-change.")

(defvar emthin--workspace-switch-timer nil
  "Single timer handle for workspace switch debounce.")

;; ── Frame ↔ workspace mapping ──

(defun emthin--map-frame-to-workspace (frame workspace-id)
  "Map FRAME to WORKSPACE-ID in both tables."
  (puthash frame workspace-id emthin--frame-workspace-table)
  (puthash workspace-id frame emthin--ws-to-frame-table))

(defun emthin--unmap-frame (frame)
  "Remove FRAME from workspace tables. Idempotent."
  (let ((ws-id (gethash frame emthin--frame-workspace-table)))
    (remhash frame emthin--frame-workspace-table)
    (when ws-id
      (remhash ws-id emthin--ws-to-frame-table))))

(defun emthin--active-frame ()
  "Return the Emacs frame for the active workspace."
  (gethash emthin--active-workspace-id emthin--ws-to-frame-table))

;; ── Workspace lifecycle (dispatch hook handlers) ──

(defun emthin--on-workspace-created (workspace-id)
  "Associate the most recently created frame with WORKSPACE-ID."
  (if emthin--pending-frame-queue
      (let ((frame (pop emthin--pending-frame-queue)))
        (when (frame-live-p frame)
          (emthin--map-frame-to-workspace frame workspace-id)
          (message "emthin: frame → workspace %d" workspace-id)
          (emthin--sync-frame frame)))
    (emthin--map-frame-to-workspace (selected-frame) workspace-id)))

(defun emthin--suppress-workspace-switch (&optional seconds)
  "Suppress after-focus-change for SECONDS (default 0.3)."
  (let ((delay (or seconds 0.3)))
    (setq emthin--workspace-switch-suppressed t)
    (when (timerp emthin--workspace-switch-timer)
      (cancel-timer emthin--workspace-switch-timer))
    (setq emthin--workspace-switch-timer
          (run-with-timer delay nil
            (lambda ()
              (setq emthin--workspace-switch-suppressed nil
                    emthin--workspace-switch-timer nil))))))

(defun emthin--on-workspace-switched (workspace-id)
  "Update active workspace tracking and re-sync."
  (setq emthin--active-workspace-id workspace-id)
  (setq emthin--last-focused-wid 'unset)
  (emthin--resync-workspace)
  (emthin--suppress-workspace-switch 0.3)
  (emthin--sync-focus (selected-window)))

(defun emthin--on-workspace-destroyed (workspace-id)
  "Clean up frame-workspace mapping for destroyed workspace."
  (maphash (lambda (frame ws-id)
             (when (eql ws-id workspace-id)
               (emthin--unmap-frame frame)))
           emthin--frame-workspace-table))

;; ── Resync ──

(defun emthin--resync-workspace ()
  "Force full re-sync for the active workspace's frame."
  (when-let* ((fr (emthin--active-frame)))
    (condition-case err
        (progn
          (walk-windows (lambda (win)
                          (let ((buf (window-buffer win)))
                            (when (buffer-local-value 'emthin--window-id buf)
                              (with-current-buffer buf
                                (setq-local emthin--last-geometry nil)))))
                        nil fr)
          (emthin--sync-frame fr))
      (error
       (message "emthin: resync error: %s" err)))))

;; ── Frame creation / deletion hooks ──

(defun emthin--after-make-frame (frame)
  "Queue FRAME for workspace association."
  (condition-case err
      (when (and emthin--process
                 emthin--active-workspace-id
                 (not (frame-parameter frame 'parent-frame)))
        (setq emthin--pending-frame-queue
              (nconc emthin--pending-frame-queue (list frame))))
    (error
     (message "emthin: after-make-frame error: %s" err))))

(defun emthin--delete-frame-hook (frame)
  "Clean up workspace mapping when a frame is deleted."
  (condition-case err
      (emthin--unmap-frame frame)
    (error
     (message "emthin: delete-frame error: %s" err))))

;; ── Focus-change driven workspace switch ──

(defun emthin--after-focus-change ()
  "Detect frame switch and request compositor workspace switch."
  (when (and emthin--process
             (not emthin--workspace-switch-suppressed))
    (condition-case err
        (let* ((frame (selected-frame))
               (ws-id (gethash frame emthin--frame-workspace-table)))
          (when (and ws-id
                     (not (eql ws-id emthin--active-workspace-id)))
            (emthin--suppress-workspace-switch 0.2)
            (emthin--send 'switch-workspace `(:workspace_id ,ws-id))))
      (error
       (message "emthin: after-focus-change error: %s" err)))))

;; ── other-frame advice ──

(defun emthin--advise-other-frame (orig-fn &optional arg &rest args)
  "Switch compositor workspace around `other-frame'."
  (when emthin--process
    (setq emthin--workspace-switch-suppressed t))
  (unwind-protect
      (apply orig-fn arg args)
    (when emthin--process
      (let* ((frame (selected-frame))
             (ws-id (gethash frame emthin--frame-workspace-table)))
        (emthin--suppress-workspace-switch 0.2)
        (when (and ws-id
                   (not (eql ws-id emthin--active-workspace-id)))
          (emthin--send 'switch-workspace `(:workspace_id ,ws-id)))))))

;; ── Register hooks and advice ──

(advice-add 'other-frame :around #'emthin--advise-other-frame)
(add-hook 'after-make-frame-functions #'emthin--after-make-frame)
(add-function :after after-focus-change-function #'emthin--after-focus-change)
(add-hook 'delete-frame-functions #'emthin--delete-frame-hook)

(add-hook 'emthin--workspace-created-hook  #'emthin--on-workspace-created)
(add-hook 'emthin--workspace-switched-hook #'emthin--on-workspace-switched)
(add-hook 'emthin--workspace-destroyed-hook #'emthin--on-workspace-destroyed)

(provide 'emthin-workspace)
;;; emthin-workspace.el ends here
```

- [ ] **Step 2: Verify it loads**

Run: `emacs --batch --eval "(progn (push \"$(pwd)/elisp\" load-path) (require 'emthin-workspace) (message \"OK\"))"`
Expected: prints "OK"

- [ ] **Step 3: Commit**

```bash
git add elisp/emthin-workspace.el
git commit -m "refactor(elisp): adapt emthin-workspace.el for dispatch hooks"
```

---

### Task 6: Adapt `emthin-manage.el`

**Files:**
- Modify: `elisp/emthin-manage.el`

- [ ] **Step 1: Replace old API calls with new ones**

Changes:
1. `(require 'emthin-app)` → `(require 'emthin-sync)` (sync re-exports app via require chain)
2. `(emthin--rect 0.0 0.0 1.0 1.0)` → `(make-emthin--rect)`
3. `emthin--report-geometry` → `emthin--apply-geometry` (same signature: wid window)

Edit `elisp/emthin-manage.el`:

Line 4: change `(require 'emthin-app)` to `(require 'emthin-sync)`
Line 179: change `(full (emthin--rect 0.0 0.0 1.0 1.0)))` to `(full (make-emthin--rect)))`
Line 212: change `(emthin--report-geometry wid win)` to `(emthin--apply-geometry wid win)`

- [ ] **Step 2: Commit**

```bash
git add elisp/emthin-manage.el
git commit -m "refactor(elisp): adapt emthin-manage.el for new API names"
```

---

### Task 7: Update `emthin.el`

**Files:**
- Modify: `elisp/emthin.el`

- [ ] **Step 1: Add requires for new modules in load order**

```elisp
;;; emthin.el --- Emacs IPC client for the emthin Wayland compositor  -*- lexical-binding: t; -*-

;;; Code:

(require 'emthin-connect)
(require 'emthin-ipc)
(require 'emthin-geom)
(require 'emthin-dispatch)
(require 'emthin-app)
(require 'emthin-sync)
(require 'emthin-workspace)
(require 'emthin-launch)
(require 'emthin-manage)

(provide 'emthin)
;;; emthin.el ends here
```

- [ ] **Step 2: Verify full require chain loads**

Run: `emacs --batch --eval "(progn (push \"$(pwd)/elisp\" load-path) (require 'emthin) (message \"OK\"))"`
Expected: prints "OK"

- [ ] **Step 3: Commit**

```bash
git add elisp/emthin.el
git commit -m "refactor(elisp): update emthin.el requires for new module layout"
```

---

### Task 8: Full load verification

**Files:**
- All

- [ ] **Step 1: Full require chain test**

Run:
```bash
emacs --batch \
  --eval "(push \"$(pwd)/elisp\" load-path)" \
  --eval "(require 'emthin-geom)" \
  --eval "(require 'emthin-dispatch)" \
  --eval "(require 'emthin-app)" \
  --eval "(require 'emthin-sync)" \
  --eval "(require 'emthin-workspace)" \
  --eval "(require 'emthin-manage)" \
  --eval "(require 'emthin)" \
  --eval "(message \"ALL OK\")"
```

Expected: prints "ALL OK" with no errors

- [ ] **Step 2: Verify geom struct and functions work**

```bash
emacs --batch \
  --eval "(push \"$(pwd)/elisp\" load-path)" \
  --eval "(require 'emthin-geom)" \
  --eval "(let ((r (make-emthin--rect :x 0.25 :y 0.5 :w 0.5 :h 0.25))) (message \"rect: %s\" r))"
```

Expected: prints a valid rect representation with x=0.25, y=0.5, w=0.5, h=0.25

- [ ] **Step 3: Verify dispatch hook registration works**

```bash
emacs --batch \
  --eval "(push \"$(pwd)/elisp\" load-path)" \
  --eval "(require 'emthin-dispatch)" \
  --eval "(require 'emthin-app)" \
  --eval "(progn (message \"window-created-hook: %s\" emthin--window-created-hook) t)"
```

Expected: shows emthin--on-window-created as handler on emthin--window-created-hook

- [ ] **Step 4: Commit**

```bash
git commit --allow-empty -m "chore: verify full elisp module chain loads correctly"
```
