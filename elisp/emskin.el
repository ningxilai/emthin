;;; emskin.el --- Emacs IPC client for the emskin Wayland compositor  -*- lexical-binding: t; -*-

;;; Code:

(require 'cl-lib)

;; ---------------------------------------------------------------------------
;; Connection settings
;; ---------------------------------------------------------------------------

(defvar emskin-ipc-path nil
  "Explicit IPC socket path.  When nil, auto-discovered via parent PID.")

;; ---------------------------------------------------------------------------
;; Shared internal state
;; ---------------------------------------------------------------------------

(defvar emskin--process nil
  "The network process connected to emskin's IPC socket.")

(defvar emskin--read-buf ""
  "Accumulates raw bytes received from emskin.")

(defvar emskin--header-offset nil
  "Pixel height of external GTK bars (menu-bar + tool-bar).
Seeded once on the first compositor SurfaceSize event and kept
constant thereafter — it's a property of the Emacs GTK frame, not of
the compositor's surface, so re-measuring on every resize would race
with GTK and break app placement when a layer-shell bar appears or
disappears.")

(defvar-local emskin--window-id nil
  "emskin window_id for the embedded app in this buffer.")

(defvar-local emskin--visible nil
  "Whether this embedded app buffer is currently displayed in an Emacs window.")

(defvar-local emskin--last-geometry nil
  "Last geometry sent for this buffer's embedded app window, to skip no-op updates.")

(defvar emskin--mirror-table (make-hash-table :test 'eql)
  "Tracks source and mirror windows per embedded app.
Key: window-id.  Value: (SOURCE-WIN . ((VIEW-ID . EMACS-WIN) ...)).")

(defvar emskin--last-focused-wid 'unset
  "Last window-id sent via set_focus IPC.  Used as change-detection guard.")

(defvar emskin--next-view-id 0
  "Counter for generating unique mirror view IDs.")

(defvar emskin--pending-native-app-targets nil
  "FIFO queue of windows reserved for newly created native app buffers.
Each `emskin-open-native-app' call appends the currently selected window.
When the compositor later emits `window_created', emskin tries to display
the new app buffer in the oldest still-live queued window before falling
back to the generic `display-buffer' path.")

;; --- Workspace tracking ---
(defvar emskin--frame-workspace-table (make-hash-table :test 'eq)
  "Maps Emacs frame objects to compositor workspace IDs.")

(defvar emskin--pending-frame-queue nil
  "Frames awaiting workspace_created IPC confirmation (FIFO order).")

(defvar emskin--active-workspace-id nil
  "Currently active workspace ID in the compositor.")

(defvar emskin--workspace-switch-suppressed nil
  "When non-nil, suppress workspace switch from after-focus-change.")

;; ---------------------------------------------------------------------------
;; Load sub-modules
;; ---------------------------------------------------------------------------

(require 'emskin-ipc)
(require 'emskin-app)
(require 'emskin-workspace)

;; ---------------------------------------------------------------------------
;; Config sync
;; ---------------------------------------------------------------------------

(defun emskin-apply-config ()
  "Re-push every registered effect's current value to the compositor.
Use after modifying variables with `setq'; toggle commands already
sync on every flip."
  (interactive)
  (unless emskin--process
    (user-error "emskin: not connected"))
  (run-hooks 'emskin-connected-hook)
  (message "emskin: config applied"))

;; ---------------------------------------------------------------------------
;; App launching
;; ---------------------------------------------------------------------------

(defun emskin--take-native-app-target-window ()
  "Return and dequeue the next live target window for a native app."
  (let (target)
    (while (and emskin--pending-native-app-targets
                (not (window-live-p target)))
      (setq target (pop emskin--pending-native-app-targets)))
    (when (window-live-p target)
      target)))

(defun emskin-open-native-app (command)
  "Launch a native Wayland application inside emskin.
COMMAND is a shell command string, e.g. \"foot\" or \"firefox\"."
  (interactive "sCommand: ")
  (let* ((args (split-string-and-unquote command))
         (target (selected-window))
         (old-targets emskin--pending-native-app-targets))
    (setq emskin--pending-native-app-targets
          (append emskin--pending-native-app-targets (list target)))
    (condition-case err
        (progn
          (apply #'start-process
                 (format "emskin-%s" (car args))
                 nil args)
          (message "emskin: launched native app: %s" command))
      (error
       (setq emskin--pending-native-app-targets old-targets)
       (signal (car err) (cdr err))))))

;; ---------------------------------------------------------------------------
;; Auto-connect when running inside emskin
;; ---------------------------------------------------------------------------

(defun emskin-maybe-auto-connect ()
  "Connect to emskin IPC if we appear to be running inside emskin.
Checks for the emskin-specific socket file derived from our parent PID."
  (let ((path (emskin--ipc-path)))
    (when (file-exists-p path)
      (run-with-timer 0.5 nil #'emskin-connect))))

(add-hook 'emacs-startup-hook #'emskin-maybe-auto-connect)

(provide 'emskin)
;;; emskin.el ends here
