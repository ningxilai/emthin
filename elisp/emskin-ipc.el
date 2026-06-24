;;; emskin-ipc.el --- IPC connection and protocol for emskin  -*- lexical-binding: t; -*-

(require 'json)

;; ---------------------------------------------------------------------------
;; IPC connection state
;; ---------------------------------------------------------------------------

(defvar emskin--process nil
  "The network process connected to emskin's IPC socket.")

(defvar emskin--read-buf ""
  "Accumulates raw bytes received from emskin.")

(defvar emskin-ipc-path nil
  "Explicit IPC socket path.  When nil, auto-discovered via parent PID.")

;; ---------------------------------------------------------------------------
;; Hooks
;; ---------------------------------------------------------------------------

(defvar emskin--message-hook nil
  "Hook run with each decoded IPC message (hash-table).")

(defvar emskin-connected-hook nil
  "Hook run after the IPC connection to emskin is (re-)established.")

;; ---------------------------------------------------------------------------
;; Codec: pure primitives
;; ---------------------------------------------------------------------------

(defun emskin--read-u32-le (buf offset)
  "Read a 32-bit little-endian unsigned integer from BUF at OFFSET.
Returns nil if BUF is too short."
  (when (>= (length buf) (+ offset 4))
    (+ (aref buf offset)
       (ash (aref buf (+ offset 1)) 8)
       (ash (aref buf (+ offset 2)) 16)
       (ash (aref buf (+ offset 3)) 24))))

(defun emskin--encode-message (msg)
  "Encode MSG (alist) as a framed JSON message (unibyte string)."
  (let* ((json (encode-coding-string (json-encode msg) 'utf-8 t))
         (len (length json))
         (prefix (unibyte-string
                  (logand len #xff)
                  (logand (ash len -8) #xff)
                  (logand (ash len -16) #xff)
                  (logand (ash len -24) #xff))))
    (concat prefix json)))

(defun emskin--decode-one (buf)
  "Try extract one framed JSON message from BUF.
Returns (REMAINING-BUF . MSG) where REMAINING-BUF is the unconsumed
data and MSG is the decoded object (hash-table) or nil.  Pure — BUF
is not modified.
On incomplete frame returns (buf . nil)."
  (let* ((len (emskin--read-u32-le buf 0)))
    (if (and len (>= (length buf) (+ 4 len)))
        (let* ((payload (decode-coding-string
                          (substring buf 4 (+ 4 len)) 'utf-8))
               (obj (json-parse-string payload)))
          (cons (substring buf (+ 4 len)) obj))
      (cons buf nil))))

;; ---------------------------------------------------------------------------
;; Codec: legacy wrappers
;; ---------------------------------------------------------------------------

(defun emskin--decode-next ()
  "Convenience wrapper over `emskin--decode-one' on `emskin--read-buf'.
Returns parsed message or nil; mutates `emskin--read-buf'."
  (let ((result (emskin--decode-one emskin--read-buf)))
    (setq emskin--read-buf (car result))
    (cdr result)))

;; ---------------------------------------------------------------------------
;; Socket discovery
;; ---------------------------------------------------------------------------

(defun emskin--ipc-path ()
  "Return the IPC socket path, auto-discovering via parent PID.
Reads /proc directly instead of spawning a subprocess."
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
;; Process filter and sentinel
;; ---------------------------------------------------------------------------

(defun emskin--filter (proc data)
  "Accumulate DATA from PROC and dispatch complete messages.
Reads and writes `emskin--read-buf' only at entry and exit;
internal decode loop uses a local buffer variable."
  (ignore proc)
  (let ((buf (concat emskin--read-buf data))
        result msg)
    (while (setq result (emskin--decode-one buf)
                 msg (cdr result))
      (setq buf (car result))
      (run-hook-with-args 'emskin--message-hook msg))
    (setq emskin--read-buf buf)))

(defun emskin--sentinel (proc event)
  "Handle IPC connection state changes."
  (ignore proc)
  (when (string-match-p "\\(closed\\|failed\\|broken\\|finished\\)" event)
    (message "emskin: IPC connection %s" (string-trim event))
    (setq emskin--process nil)))

;; ---------------------------------------------------------------------------
;; Send / Connect
;; ---------------------------------------------------------------------------

(defun emskin--send-thunk (msg)
  "Return thunk that sends MSG when called.
Encoding (pure computation) happens at thunk creation time; the
actual network write (effect) happens when the thunk is executed.
The process reference is captured at creation time."
  (let ((encoded (emskin--encode-message msg))
        (proc emskin--process))
    (lambda ()
      (when proc
        (process-send-string proc encoded)))))

(defun emskin--send (msg)
  "Send MSG (alist) to emskin over IPC."
  (funcall (emskin--send-thunk msg)))

(defun emskin-connect ()
  "Connect to the emskin IPC socket (auto-discovers path)."
  (interactive)
  (when emskin--process
    (delete-process emskin--process)
    (setq emskin--process nil))
  (setq emskin--read-buf "")
  (let ((path (emskin--ipc-path)))
    (condition-case err
        (progn
          (setq emskin--process
                (make-network-process
                 :name "emskin-ipc"
                 :family 'local
                 :service path
                 :coding 'binary
                 :filter #'emskin--filter
                 :sentinel #'emskin--sentinel
                 :nowait nil))
          (message "emskin: connecting to %s" path))
      (error
       (message "emskin: failed to connect to %s: %s" path err)))))

(provide 'emskin-ipc)
;;; emskin-ipc.el ends here
