;;; emskin-ipc.el --- IPC connection and protocol for emskin  -*- lexical-binding: t; -*-

(require 'jsonrpc)

;; ---------------------------------------------------------------------------
;; IPC connection state
;; ---------------------------------------------------------------------------

(defvar emskin--jsonrpc-conn nil
  "JSON-RPC connection to emskin compositor.")

(defvar emskin--process nil
  "Non-nil while the IPC connection is active.")

(defvar emskin-ipc-path nil
  "Explicit IPC socket path.  When nil, auto-discovered via parent PID.")

;; ---------------------------------------------------------------------------
;; Hooks
;; ---------------------------------------------------------------------------

(defvar emskin--message-hook nil
  "Hook run with (METHOD PARAMS) for each incoming JSON-RPC notification.")

(defvar emskin-connected-hook nil
  "Hook run after the IPC connection to emskin is (re-)established.")

;; ---------------------------------------------------------------------------
;; Helpers
;; ---------------------------------------------------------------------------

(defun emskin--kebab->snake (sym)
  "Convert SYM from kebab-case to snake_case.
E.g., `set-focus' → `set_focus'.  No-op if already snake_case."
  (let ((name (symbol-name sym)))
    (if (string-search "-" name)
        (intern (string-replace "-" "_" name))
      sym)))

;; ---------------------------------------------------------------------------
;; Sending
;; ---------------------------------------------------------------------------

(defun emskin--send (method params)
  "Send JSON-RPC notification METHOD with PARAMS.
METHOD is a kebab-case or snake_case symbol (kebab converted
to snake for the wire format).  PARAMS is a plist suitable for
`json-serialize'."
  (when emskin--jsonrpc-conn
    (jsonrpc-notify emskin--jsonrpc-conn
                     (emskin--kebab->snake method) params)))

;; ---------------------------------------------------------------------------
;; Socket discovery
;; ---------------------------------------------------------------------------

(defun emskin--ipc-path ()
  "Return the IPC socket path, auto-discovering via parent PID."
  (or emskin-ipc-path
      (with-temp-buffer
        (insert-file-contents-literally
         (format "/proc/%d/status" (emacs-pid)))
        (goto-char (point-min))
        (let ((ppid (and (re-search-forward "^PPid:\t\\([0-9]+\\)" nil t)
                         (match-string 1))))
          (format "%s/emskin-%s.ipc"
                  (or (getenv "XDG_RUNTIME_DIR") "/tmp")
                  ppid)))))

;; ---------------------------------------------------------------------------
;; Connection
;; ---------------------------------------------------------------------------

(defun emskin-connect ()
  "Connect to the emskin IPC socket (auto-discovers path)."
  (interactive)
  ;; Clean up stale connection
  (when emskin--jsonrpc-conn
    (jsonrpc-shutdown emskin--jsonrpc-conn)
    (setq emskin--jsonrpc-conn nil
          emskin--process nil))
  (let* ((path (emskin--ipc-path))
         (proc (condition-case err
                   (make-network-process
                    :name "emskin-ipc"
                    :family 'local
                    :service path
                    :coding 'binary)
                 (error
                  (message "emskin: failed to connect to %s: %s" path err)
                  nil))))
    (when proc
      (setq emskin--jsonrpc-conn
            (jsonrpc-process-connection
             :process proc
             :notification-dispatcher #'emskin--dispatch-notification
             :on-shutdown
             (lambda (_c)
               (message "emskin: IPC disconnected")
               (setq emskin--jsonrpc-conn nil
                     emskin--process nil))))
      (setq emskin--process t)
      (message "emskin: connecting to %s" path))))

(defun emskin--dispatch-notification (_conn method params)
  "Dispatch incoming JSON-RPC notification METHOD with PARAMS."
  (condition-case err
      (run-hook-with-args 'emskin--message-hook method params)
    (error
     (message "emskin: notification dispatch error (%s): %s" method err))))

(provide 'emskin-ipc)
;;; emskin-ipc.el ends here
