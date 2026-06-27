;;; emthin-dispatch.el --- IPC message dispatch for emthin  -*- lexical-binding: t; -*-

(require 'cl-lib)
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
