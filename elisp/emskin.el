;;; emskin.el --- Emacs IPC client for the emskin Wayland compositor  -*- lexical-binding: t; -*-

;;; Code:

(require 'cl-lib)

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

(require 'emskin-connect)
(require 'emskin-ipc)
(require 'emskin-app)
(require 'emskin-workspace)
(require 'emskin-launch)

(provide 'emskin)
;;; emskin.el ends here
