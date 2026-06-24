;;; emskin-ipc.el --- IPC connection and protocol for emskin  -*- lexical-binding: t; -*-

(require 'jsonrpc)

;; ---------------------------------------------------------------------------
;; IPC connection state
;; ---------------------------------------------------------------------------

(defvar emskin--jsonrpc-conn nil
  "JSON-RPC connection to emskin compositor.")

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
;; Sending
;; ---------------------------------------------------------------------------

(defun emskin--send (method params)
  "Send JSON-RPC notification METHOD with PARAMS.
PARAMS is a plist suitable for `json-serialize'."
  (when emskin--jsonrpc-conn
    (jsonrpc-notify emskin--jsonrpc-conn method params)))

(defun emskin--send-thunk (method params)
  "Return thunk that sends METHOD+PARAMS when called.
Encoding happens at thunk-creation time; network write happens
when the thunk is called."
  (let ((conn emskin--jsonrpc-conn))
    (lambda ()
      (when conn (jsonrpc-notify conn method params)))))

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
    (setq emskin--jsonrpc-conn nil))
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
               (setq emskin--jsonrpc-conn nil))))
      (message "emskin: connecting to %s" path))))

(defun emskin--dispatch-notification (_conn method params)
  "Dispatch incoming JSON-RPC notification METHOD with PARAMS."
  (run-hook-with-args 'emskin--message-hook method params))

(provide 'emskin-ipc)
;;; emskin-ipc.el ends here
