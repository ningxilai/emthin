;;; emthin-app.el --- Embedded app lifecycle (EIEIO)  -*- lexical-binding: t; -*-

(require 'eieio)
(require 'cl-lib)
(require 'subr-x)
(require 'emthin-ipc)
(require 'emthin-geom)
(require 'emthin-layout)

;; ── Application object ──

(defclass emthin--app ()
  ((window-id  :initarg :window-id :type integer
               :documentation "Compositor-assigned window ID.")
   (buffer     :initarg :buffer   :type buffer
               :documentation "Emacs buffer for the embedded app.")
   (last-geometry :initform nil
                  :documentation "Last emthin--rect sent to compositor.")
   (saved-geometry :initform nil
                    :documentation "Geometry saved before fullscreen (manage.el).")
   (layout :initarg :layout
           :type (or null emthin-layout)
           :documentation "Layout policy object for this app."))
  :documentation "An embedded application managed by emthin.")

;; ── Global state ──

(defvar emthin--header-offset nil
  "Pixel height of GTK menu-bar + tool-bar. Updated on every surface_size event.")

(defvar emthin--frame-layout (make-instance 'emthin-layout-fill)
  "Frame-level layout strategy.  Defaults to fill behavior (wid-wins).")

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

;; ── Buffer-local variables (defvar for Emacs 30 void-variable safety) ──

(defvar emthin--window-id nil)
(defvar emthin--visible nil)

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
             (layout (make-instance 'emthin-layout-fill))
             (app (make-instance 'emthin--app
                    :window-id window-id :buffer buf :layout layout))
             win)
        (with-current-buffer buf
          (setq-local emthin--window-id window-id)
          (setq-local emthin--visible nil)
          (setq-local mode-name "emthin")
          (setq-local buffer-read-only t)
          (setq-local left-fringe-width 0)
          (setq-local right-fringe-width 0)
          (setq-local left-margin-width 0)
          (setq-local right-margin-width 0)
          (setq-local cursor-type nil)
           (add-hook 'kill-buffer-hook #'emthin--kill-buffer-hook nil t))
         (emthin--register-app app)
        (setq win (if-let* ((target (emthin--take-app-target-window)))
                       (progn (set-window-buffer target buf) target)
                     (display-buffer buf '((display-buffer-pop-up-window
                                            display-buffer-use-some-window)
                                           (inhibit-same-window . t)
                                           (reusable-frames . nil)))))
        ;; Send initial geometry immediately to avoid the app flashing at
        ;; (0, 0) before sync-frame runs.
        (when (window-live-p win)
          (emthin--apply-geometry app win))
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

(defun emthin--on-window-resized (window-id x y w h)
  "Update app's last-geometry from compositor-initiated resize."
  (when-let* ((app (emthin--find-app window-id)))
    (let ((geo (make-emthin--rect :x x :y y :w w :h h)))
      (oset app last-geometry geo))))

(defun emthin--on-surface-size (width height)
  "Record header offset from surface size event."
  (let* ((frame-h (frame-pixel-height))
         (offset (max 0 (- height frame-h))))
    (setq emthin--header-offset offset)
    (message "emthin: surface=%sx%s bars=%dpx" width height offset)))

(defun emthin--on-connected (version)
  "Handle compositor connected event."
  (message "emthin: connected (version %s)" version)
  (run-hooks 'emthin-connected-hook))

(defun emthin--set-migration-policy (policy)
  "Set compositor migration policy: \\='manual or \\='by-workspace-affinity."
  (interactive "SMigration policy (manual/by-workspace-affinity): ")
  (emthin--send 'set-migration-policy `(:policy ,(symbol-name policy))))

;; ── Kill-buffer hook ──

(defun emthin--kill-buffer-hook ()
  "Notify emthin to close the app when its buffer is killed."
  (condition-case err
      (when (and (boundp 'emthin--window-id)
                 emthin--window-id)
        (emthin--send 'close `(:window_id ,emthin--window-id)))
    (error
     (message "emthin: kill-buffer-hook error: %s" err))))

;; ── Register on dispatch hooks ──

(add-hook 'emthin--connected-hook       #'emthin--on-connected)
(add-hook 'emthin--window-created-hook  #'emthin--on-window-created)
(add-hook 'emthin--window-destroyed-hook #'emthin--on-window-destroyed)
(add-hook 'emthin--title-changed-hook   #'emthin--on-title-changed)
(add-hook 'emthin--window-resized-hook  #'emthin--on-window-resized)
(add-hook 'emthin--surface-size-hook    #'emthin--on-surface-size)

(provide 'emthin-app)
;;; emthin-app.el ends here
