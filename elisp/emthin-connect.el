;;; emthin-connect.el --- Connection lifecycle for emthin  -*- lexical-binding: t; -*-

;;; Code:

(require 'emthin-ipc)

;; ---------------------------------------------------------------------------
;; Auto-connect when running inside emthin
;; ---------------------------------------------------------------------------

(defun emthin-maybe-auto-connect ()
  "Connect to emthin IPC if we appear to be running inside emthin.
Checks for the emthin-specific socket file derived from our parent PID."
  (let ((path (emthin--ipc-path)))
    (when (file-exists-p path)
      (run-with-timer 0.5 nil #'emthin-connect))))

(add-hook 'emacs-startup-hook #'emthin-maybe-auto-connect)

(provide 'emthin-connect)
;;; emthin-connect.el ends here
