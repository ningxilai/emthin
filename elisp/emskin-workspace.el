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

(defvar emskin--workspace-switch-timer nil
  "Single timer handle for workspace switch debounce.")

;; ---------------------------------------------------------------------------
;; IPC message handler (registered on emskin--message-hook)
;; ---------------------------------------------------------------------------

(defun emskin--handle-workspace-message (method params)
  "Dispatch workspace-related IPC messages from emskin."
  (pcase method
    ('workspace_created
     (emskin--on-workspace-created (plist-get params :workspace_id)))
    ('workspace_switched
     (emskin--on-workspace-switched (plist-get params :workspace_id)))
    ('workspace_destroyed
     (emskin--on-workspace-destroyed (plist-get params :workspace_id)))))

(add-hook 'emskin--message-hook #'emskin--handle-workspace-message)

;; ---------------------------------------------------------------------------
;; Workspace lifecycle
;; ---------------------------------------------------------------------------

(defun emskin--on-workspace-created (workspace-id)
  "Associate the most recently created frame with WORKSPACE-ID."
  (if emskin--pending-frame-queue
      (let ((frame (pop emskin--pending-frame-queue)))
        (when (frame-live-p frame)
          (emskin--map-frame-to-workspace frame workspace-id)
          (message "emskin: frame → workspace %d" workspace-id)
          (emskin--sync-frame frame)))
    (emskin--map-frame-to-workspace (selected-frame) workspace-id)))

(defun emskin--suppress-workspace-switch-thunks (&optional seconds)
  "Return list of thunks to suppress workspace switch for SECONDS."
  (let ((delay (or seconds 0.3))
        (current-timer emskin--workspace-switch-timer))
    (list
     (lambda () (setq emskin--workspace-switch-suppressed t))
     (lambda ()
       (when (timerp current-timer)
         (cancel-timer current-timer)))
     (lambda ()
       (setq emskin--workspace-switch-timer
             (run-with-timer delay nil
               (lambda ()
                 (setq emskin--workspace-switch-suppressed nil)
                 (setq emskin--workspace-switch-timer nil))))))))

(defun emskin--on-workspace-switched (workspace-id)
  "Update active workspace tracking and re-sync geometry."
  (emskin--exec-effects
   (append (list (lambda () (setq emskin--active-workspace-id workspace-id))
                 (lambda () (setq emskin--last-focused-wid 'unset))
                 (lambda () (emskin--resync-workspace)))
           (emskin--suppress-workspace-switch-thunks 0.3)
           (or (emskin--sync-focus-thunks (selected-window)) nil))))

(defun emskin--on-workspace-destroyed (workspace-id)
  "Clean up frame-workspace mapping for destroyed workspace."
  (maphash (lambda (frame ws-id)
             (when (eql ws-id workspace-id)
               (emskin--unmap-frame frame)))
           emskin--frame-workspace-table))

;; ---------------------------------------------------------------------------
;; Resync
;; ---------------------------------------------------------------------------

(defun emskin--resync-workspace ()
  "Force full re-sync for the active workspace's frame.
Clears geometry cache only for windows in the active frame, then
delegates to `emskin--sync-frame'."
  (when-let* ((fr (emskin--active-frame)))
    (walk-windows (lambda (win)
                    (let ((buf (window-buffer win)))
                      (when (buffer-local-value 'emskin--window-id buf)
                        (with-current-buffer buf
                          (setq-local emskin--last-geometry nil)))))
                  nil fr)
    (emskin--sync-frame fr)))

(defun emskin--active-frame ()
  "Return the Emacs frame for the active workspace, or nil.
Uses the reverse mapping table for O(1) lookup."
  (gethash emskin--active-workspace-id emskin--ws-to-frame-table))

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
  (emskin--unmap-frame frame))

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
        (emskin--exec-effects
         (append (emskin--suppress-workspace-switch-thunks 0.2)
                 (list (lambda ()
                         (emskin--call* 'switch-workspace
                                        :workspace_id ws-id)))))))))

;; ---------------------------------------------------------------------------
;; other-frame advice
;; ---------------------------------------------------------------------------

(defun emskin--advise-other-frame (orig-fn &optional arg &rest args)
  "Switch compositor workspace around `other-frame'.
Suppresses `emskin--after-focus-change' before delegating to the
original, then sends `switch-workspace' based on the actual target
frame — no repeated frame-cycle logic."
  (when emskin--process
    (emskin--exec-effects
     (list (lambda () (setq emskin--workspace-switch-suppressed t)))))
  (unwind-protect
      (apply orig-fn arg args)
    (when emskin--process
      (let* ((frame (selected-frame))
             (ws-id (gethash frame emskin--frame-workspace-table)))
        (emskin--exec-effects
         (append (emskin--suppress-workspace-switch-thunks 0.2)
                 (when (and ws-id
                            (not (eql ws-id emskin--active-workspace-id)))
                   (list (lambda ()
                           (emskin--call* 'switch-workspace
                                          :workspace_id ws-id))))))))))

;; ---------------------------------------------------------------------------
;; Register hooks and advice
;; ---------------------------------------------------------------------------

(advice-add 'other-frame :around #'emskin--advise-other-frame)
(add-hook 'after-make-frame-functions #'emskin--after-make-frame)
(add-function :after after-focus-change-function #'emskin--after-focus-change)
(add-hook 'delete-frame-functions #'emskin--delete-frame-hook)

(provide 'emskin-workspace)
;;; emskin-workspace.el ends here
