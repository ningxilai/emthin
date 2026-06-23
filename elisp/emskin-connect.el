;;; emskin-connect.el --- Connection lifecycle for emskin  -*- lexical-binding: t; -*-

;;; Code:

(require 'emskin-ipc)

;; ---------------------------------------------------------------------------
;; Connection settings
;; ---------------------------------------------------------------------------

(defvar emskin-ipc-path nil
  "Explicit IPC socket path.  When nil, auto-discovered via parent PID.")

;; ---------------------------------------------------------------------------
;; Config sync
;; ---------------------------------------------------------------------------

(defun emskin-apply-config ()
  "Re-sync state with the compositor after `setq' changes."
  (interactive)
  (unless emskin--process
    (user-error "emskin: not connected"))
  (run-hooks 'emskin-connected-hook)
  (message "emskin: config applied"))

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

(provide 'emskin-connect)
;;; emskin-connect.el ends here
