;;; emskin-workspace.el --- Workspace management for emskin  -*- lexical-binding: t; -*-

(require 'emskin-app)
(require 'emskin-ipc)

;; ---------------------------------------------------------------------------
;; Workspace-local state
;; ---------------------------------------------------------------------------

(defvar emskin--pending-frame-queue nil
  "Frames awaiting workspace_created IPC confirmation (FIFO order).")

(defvar emskin--workspace-switch-suppressed nil
  "When non-nil, suppress workspace switch from after-focus-change.")

;; ---------------------------------------------------------------------------
;; IPC message handler (registered on emskin--message-hook)
;; ---------------------------------------------------------------------------

(defun emskin--handle-workspace-message (msg)
  "Dispatch workspace-related IPC messages from emskin."
  (let ((type (gethash "type" msg "")))
    (cond
     ((string= type "workspace_created")
      (emskin--on-workspace-created (gethash "workspace_id" msg)))
     ((string= type "workspace_switched")
      (emskin--on-workspace-switched (gethash "workspace_id" msg)))
     ((string= type "workspace_destroyed")
      (emskin--on-workspace-destroyed (gethash "workspace_id" msg))))))

(add-hook 'emskin--message-hook #'emskin--handle-workspace-message)

;; ---------------------------------------------------------------------------
;; Workspace lifecycle
;; ---------------------------------------------------------------------------

(defun emskin--on-workspace-created (workspace-id)
  "Associate the most recently created frame with WORKSPACE-ID."
  (if emskin--pending-frame-queue
      (let ((frame (pop emskin--pending-frame-queue)))
        (when (frame-live-p frame)
          (puthash frame workspace-id emskin--frame-workspace-table)
          (message "emskin: frame → workspace %d" workspace-id)
          (emskin--sync-frame frame)))
    (puthash (selected-frame) workspace-id emskin--frame-workspace-table)))

(defun emskin--on-workspace-switched (workspace-id)
  "Update active workspace tracking and re-sync geometry."
  (setq emskin--active-workspace-id workspace-id)
  (setq emskin--workspace-switch-suppressed t)
  (run-with-timer 0.3 nil (lambda () (setq emskin--workspace-switch-suppressed nil)))
  (setq emskin--last-focused-wid 'unset)
  (emskin--resync-workspace)
  (emskin--sync-focus (selected-window)))

(defun emskin--on-workspace-destroyed (workspace-id)
  "Clean up frame-workspace mapping for destroyed workspace."
  (maphash (lambda (frame ws-id)
             (when (eql ws-id workspace-id)
               (remhash frame emskin--frame-workspace-table)))
           emskin--frame-workspace-table))

;; ---------------------------------------------------------------------------
;; Resync
;; ---------------------------------------------------------------------------

(defun emskin--resync-workspace ()
  "Force full re-sync for the active workspace's frame.
Clears change detection then delegates to `emskin--sync-frame',
which handles source/mirror separation correctly."
  (dolist (buf (buffer-list))
    (when (buffer-local-value 'emskin--window-id buf)
      (with-current-buffer buf
        (setq-local emskin--last-geometry nil))))
  (when-let* ((fr (emskin--active-frame)))
    (emskin--sync-frame fr)))

(defun emskin--active-frame ()
  "Return the Emacs frame for the active workspace, or nil."
  (let (result)
    (maphash (lambda (frame ws-id)
               (when (eql ws-id emskin--active-workspace-id)
                 (setq result frame)))
             emskin--frame-workspace-table)
    result))

;; ---------------------------------------------------------------------------
;; Frame creation / deletion hooks
;; ---------------------------------------------------------------------------

(defun emskin--after-make-frame (frame)
  "Queue FRAME for workspace association when a non-child frame is created."
  (when (and emskin--process
             emskin--active-workspace-id
             (not (frame-parameter frame 'parent-frame)))
    (setq emskin--pending-frame-queue
          (nconc emskin--pending-frame-queue (list frame)))))

(defun emskin--delete-frame-hook (frame)
  "Clean up workspace mapping when a frame is deleted."
  (remhash frame emskin--frame-workspace-table))

;; ---------------------------------------------------------------------------
;; Focus-change driven workspace switch
;; ---------------------------------------------------------------------------

(defun emskin--after-focus-change ()
  "Detect frame switch and request compositor workspace switch."
  (when (and emskin--process
             (not emskin--workspace-switch-suppressed))
    (let* ((frame (selected-frame))
           (ws-id (gethash frame emskin--frame-workspace-table)))
      (when (and ws-id
                 (not (eql ws-id emskin--active-workspace-id)))
        (setq emskin--workspace-switch-suppressed t)
        (emskin--send `((type . "switch_workspace")
                        (workspace_id . ,ws-id)))
        (run-with-timer 0.2 nil
                        (lambda ()
                          (setq emskin--workspace-switch-suppressed nil)))))))

;; ---------------------------------------------------------------------------
;; other-frame advice
;; ---------------------------------------------------------------------------

(defun emskin--advise-other-frame (orig-fn &optional arg &rest args)
  "Switch compositor workspace around `other-frame'.
Sends switch_workspace BEFORE so GTK can focus the target window.
Resync is handled by `emskin--on-workspace-switched' when the
compositor confirms the switch via IPC — NOT here, because
`active-workspace-id' is still stale at this point."
  (when emskin--process
    (let* ((n (or arg 1))
           (target (let ((f (selected-frame)))
                     (dotimes (_ (abs n))
                       (setq f (if (> n 0) (next-frame f) (previous-frame f))))
                     f))
           (ws-id (gethash target emskin--frame-workspace-table)))
      (when (and ws-id (not (eql ws-id emskin--active-workspace-id)))
        (emskin--send `((type . "switch_workspace")
                        (workspace_id . ,ws-id))))))
  (apply orig-fn arg args))

;; ---------------------------------------------------------------------------
;; Register hooks and advice
;; ---------------------------------------------------------------------------

(advice-add 'other-frame :around #'emskin--advise-other-frame)
(add-hook 'after-make-frame-functions #'emskin--after-make-frame)
(add-function :after after-focus-change-function #'emskin--after-focus-change)
(add-hook 'delete-frame-functions #'emskin--delete-frame-hook)

(provide 'emskin-workspace)
;;; emskin-workspace.el ends here
