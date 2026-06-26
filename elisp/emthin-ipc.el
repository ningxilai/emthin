;;; emthin-ipc.el --- IPC connection and protocol for emthin  -*- lexical-binding: t; -*-

(require 'jsonrpc)

;; ---------------------------------------------------------------------------
;; IPC connection state
;; ---------------------------------------------------------------------------

(defvar emthin--jsonrpc-conn nil
  "JSON-RPC connection to emthin compositor.")

(defvar emthin--process nil
  "Non-nil while the IPC connection is active.")

(defvar emthin-ipc-path nil
  "Explicit IPC socket path.  When nil, auto-discovered via parent PID.")

;; ---------------------------------------------------------------------------
;; Hooks
;; ---------------------------------------------------------------------------

(defvar emthin--message-hook nil
  "Hook run with (METHOD PARAMS) for each incoming JSON-RPC notification.")

(defvar emthin-connected-hook nil
  "Hook run after the IPC connection to emthin is (re-)established.")

;; ---------------------------------------------------------------------------
;; Helpers
;; ---------------------------------------------------------------------------

(defun emthin--kebab->snake (sym)
  "Convert SYM from kebab-case to snake_case.
E.g., `set-focus' → `set_focus'.  No-op if already snake_case."
  (let ((name (symbol-name sym)))
    (if (string-search "-" name)
        (intern (string-replace "-" "_" name))
      sym)))

;; ---------------------------------------------------------------------------
;; Sending
;; ---------------------------------------------------------------------------

(defun emthin--send (method params)
  "Send JSON-RPC notification METHOD with PARAMS.
METHOD is a kebab-case or snake_case symbol (kebab converted
to snake for the wire format).  PARAMS is a plist suitable for
`json-serialize'."
  (when emthin--jsonrpc-conn
    (jsonrpc-notify emthin--jsonrpc-conn
                     (emthin--kebab->snake method) params)))

;; ---------------------------------------------------------------------------
;; Socket discovery
;; ---------------------------------------------------------------------------

(defun emthin--ipc-path ()
  "Return the IPC socket path, auto-discovering via parent PID."
  (or emthin-ipc-path
      (with-temp-buffer
        (insert-file-contents-literally
         (format "/proc/%d/status" (emacs-pid)))
        (goto-char (point-min))
        (let ((ppid (and (re-search-forward "^PPid:\t\\([0-9]+\\)" nil t)
                         (match-string 1))))
          (format "%s/emthin-%s.ipc"
                  (or (getenv "XDG_RUNTIME_DIR") "/tmp")
                  ppid)))))

;; ---------------------------------------------------------------------------
;; Connection
;; ---------------------------------------------------------------------------

(defun emthin-connect ()
  "Connect to the emthin IPC socket (auto-discovers path)."
  (interactive)
  ;; Clean up stale connection
  (when emthin--jsonrpc-conn
    (jsonrpc-shutdown emthin--jsonrpc-conn)
    (setq emthin--jsonrpc-conn nil
          emthin--process nil))
  (let* ((path (emthin--ipc-path))
         (proc (condition-case err
                   (make-network-process
                    :name "emthin-ipc"
                    :family 'local
                    :service path
                    :coding 'binary)
                 (error
                  (message "emthin: failed to connect to %s: %s" path err)
                  nil))))
    (when proc
      (setq emthin--jsonrpc-conn
            (jsonrpc-process-connection
             :name "emthin"
             :process proc
             :notification-dispatcher #'emthin--dispatch-notification
             :on-shutdown
             (lambda (_c)
               (message "emthin: IPC disconnected")
               (setq emthin--jsonrpc-conn nil
                     emthin--process nil))))
      (setq emthin--process t)
      ;; Register a custom events buffer with the connection.  Without
      ;; this, jsonrpc-events-buffer lazily creates *emthin events* in
      ;; fundamental-mode with system-default coding, and Chinese chars
      ;; from clipboard/IME events display as octal escapes.
      ;;
      ;; save-buffer-coding-system makes basic-save-buffer-1 bind
      ;; coding-system-for-write so write-region skips
      ;; select-safe-coding-system entirely — avoiding the
      ;; buffer-chars-modified-tick race (Emacs 32, line 1067).
      (let ((buf (jsonrpc-events-buffer emthin--jsonrpc-conn)))
        (with-current-buffer buf
          (setq default-directory temporary-file-directory
                buffer-offer-save nil
                buffer-auto-save-file-name nil
                buffer-file-coding-system 'utf-8-unix
                save-buffer-coding-system 'utf-8-unix)))
      (message "emthin: connecting to %s" path))))

(defun emthin--dispatch-notification (_conn method params)
  "Dispatch incoming JSON-RPC notification METHOD with PARAMS."
  (condition-case err
      (run-hook-with-args 'emthin--message-hook method params)
    (error
     (message "emthin: notification dispatch error (%s): %s" method err))))

;; ---------------------------------------------------------------------------
;; DBus router rule management
;; ---------------------------------------------------------------------------

(defun emthin-dbus-router-add-rule (id destination interface method priority target)
  "Add a routing rule via IPC.
ID is a unique string identifier.
DESTINATION, INTERFACE, METHOD are glob patterns (or nil for wildcard).
PRIORITY is an integer. TARGET is \"host\", \"isolated\", or \"deny\"."
  (emthin--send 'dbus_router_add_rule
                `(:rule ,(emthin--dbus-rule-to-plist
                          id destination interface method priority target))))

(defun emthin-dbus-router-remove-rule (id)
  "Remove routing rule with ID via IPC."
  (emthin--send 'dbus_router_remove_rule `(:id ,id)))

(defun emthin-dbus-router-list-rules ()
  "Request the list of current routing rules via IPC.
The result arrives as a `dbus_router_rules' notification."
  (emthin--send 'dbus_router_list_rules nil))

(defun emthin--dbus-rule-to-plist (id destination interface method priority target)
  "Convert rule fields to a plist for JSON serialization."
  (let ((plist `(:id ,id :priority ,priority :target ,target)))
    (when destination
      (setq plist (plist-put plist :destination destination)))
    (when interface
      (setq plist (plist-put plist :interface interface)))
    (when method
      (setq plist (plist-put plist :method method)))
    plist))

(provide 'emthin-ipc)
;;; emthin-ipc.el ends here
