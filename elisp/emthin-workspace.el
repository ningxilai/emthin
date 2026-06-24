;;; emthin-workspace.el --- Workspace management for emthin  -*- lexical-binding: t; -*-

(require 'emthin-app)
(require 'emthin-ipc)

;; ---------------------------------------------------------------------------
;; Workspace-local state
;; ---------------------------------------------------------------------------

(defvar emthin--pending-frame-queue nil
  "Frames awaiting workspace_created IPC confirmation (FIFO order).")

(defvar emthin--workspace-switch-suppressed nil
  "When non-nil, suppress workspace switch from after-focus-change.")

(defvar emthin--workspace-switch-timer nil
  "Single timer handle for workspace switch debounce.")

;; ---------------------------------------------------------------------------
;; IPC message handler (registered on emthin--message-hook)
;; ---------------------------------------------------------------------------

(defun emthin--handle-workspace-message (method params)
  "Dispatch workspace-related IPC messages from emthin."
  (pcase method
    ('workspace_created
     (emthin--on-workspace-created (plist-get params :workspace_id)))
    ('workspace_switched
     (emthin--on-workspace-switched (plist-get params :workspace_id)))
    ('workspace_destroyed
     (emthin--on-workspace-destroyed (plist-get params :workspace_id)))))

(add-hook 'emthin--message-hook #'emthin--handle-workspace-message)

;; ---------------------------------------------------------------------------
;; Workspace lifecycle
;; ---------------------------------------------------------------------------

(defun emthin--on-workspace-created (workspace-id)
  "Associate the most recently created frame with WORKSPACE-ID."
  (if emthin--pending-frame-queue
      (let ((frame (pop emthin--pending-frame-queue)))
        (when (frame-live-p frame)
          (emthin--map-frame-to-workspace frame workspace-id)
          (message "emthin: frame → workspace %d" workspace-id)
          (emthin--sync-frame frame)))
    (emthin--map-frame-to-workspace (selected-frame) workspace-id)))

(defun emthin--suppress-workspace-switch (&optional seconds)
  "Suppress `emthin--after-focus-change' for SECONDS (default 0.3)."
  (let ((delay (or seconds 0.3)))
    (setq emthin--workspace-switch-suppressed t)
    (when (timerp emthin--workspace-switch-timer)
      (cancel-timer emthin--workspace-switch-timer))
    (setq emthin--workspace-switch-timer
          (run-with-timer delay nil
            (lambda ()
              (setq emthin--workspace-switch-suppressed nil
                    emthin--workspace-switch-timer nil))))))

(defun emthin--on-workspace-switched (workspace-id)
  "Update active workspace tracking and re-sync geometry."
  (setq emthin--active-workspace-id workspace-id)
  (setq emthin--last-focused-wid 'unset)
  (emthin--resync-workspace)
  (emthin--suppress-workspace-switch 0.3)
  (emthin--sync-focus (selected-window)))

(defun emthin--on-workspace-destroyed (workspace-id)
  "Clean up frame-workspace mapping for destroyed workspace."
  (maphash (lambda (frame ws-id)
             (when (eql ws-id workspace-id)
               (emthin--unmap-frame frame)))
           emthin--frame-workspace-table))

;; ---------------------------------------------------------------------------
;; Resync
;; ---------------------------------------------------------------------------

(defun emthin--resync-workspace ()
  "Force full re-sync for the active workspace's frame.
Clears geometry cache only for windows in the active frame, then
delegates to `emthin--sync-frame'."
  (when-let* ((fr (emthin--active-frame)))
    (condition-case err
        (progn
          (walk-windows (lambda (win)
                          (let ((buf (window-buffer win)))
                            (when (buffer-local-value 'emthin--window-id buf)
                              (with-current-buffer buf
                                (setq-local emthin--last-geometry nil)))))
                        nil fr)
          (emthin--sync-frame fr))
      (error
       (message "emthin: resync error: %s" err)))))

(defun emthin--active-frame ()
  "Return the Emacs frame for the active workspace, or nil.
Uses the reverse mapping table for O(1) lookup."
  (gethash emthin--active-workspace-id emthin--ws-to-frame-table))

;; ---------------------------------------------------------------------------
;; Frame creation / deletion hooks
;; ---------------------------------------------------------------------------

(defun emthin--after-make-frame (frame)
  "Queue FRAME for workspace association when a non-child frame is created."
  (condition-case err
      (when (and emthin--process
                 emthin--active-workspace-id
                 (not (frame-parameter frame 'parent-frame)))
        (setq emthin--pending-frame-queue
              (nconc emthin--pending-frame-queue (list frame))))
    (error
     (message "emthin: after-make-frame error: %s" err))))

(defun emthin--delete-frame-hook (frame)
  "Clean up workspace mapping when a frame is deleted."
  (condition-case err
      (emthin--unmap-frame frame)
    (error
     (message "emthin: delete-frame error: %s" err))))

;; ---------------------------------------------------------------------------
;; Focus-change driven workspace switch
;; ---------------------------------------------------------------------------

(defun emthin--after-focus-change ()
  "Detect frame switch and request compositor workspace switch."
  (when (and emthin--process
             (not emthin--workspace-switch-suppressed))
    (condition-case err
        (let* ((frame (selected-frame))
               (ws-id (gethash frame emthin--frame-workspace-table)))
          (when (and ws-id
                     (not (eql ws-id emthin--active-workspace-id)))
            (emthin--suppress-workspace-switch 0.2)
            (emthin--call* 'switch-workspace :workspace_id ws-id)))
      (error
       (message "emthin: after-focus-change error: %s" err)))))

;; ---------------------------------------------------------------------------
;; other-frame advice
;; ---------------------------------------------------------------------------

(defun emthin--advise-other-frame (orig-fn &optional arg &rest args)
  "Switch compositor workspace around `other-frame'.
Suppresses `emthin--after-focus-change' before delegating to the
original, then sends `switch-workspace' based on the actual target
frame — no repeated frame-cycle logic."
  (when emthin--process
    (setq emthin--workspace-switch-suppressed t))
  (unwind-protect
      (apply orig-fn arg args)
    (when emthin--process
      (let* ((frame (selected-frame))
             (ws-id (gethash frame emthin--frame-workspace-table)))
        (emthin--suppress-workspace-switch 0.2)
        (when (and ws-id
                   (not (eql ws-id emthin--active-workspace-id)))
          (emthin--call* 'switch-workspace :workspace_id ws-id))))))

;; ---------------------------------------------------------------------------
;; Register hooks and advice
;; ---------------------------------------------------------------------------

(advice-add 'other-frame :around #'emthin--advise-other-frame)
(add-hook 'after-make-frame-functions #'emthin--after-make-frame)
(add-function :after after-focus-change-function #'emthin--after-focus-change)
(add-hook 'delete-frame-functions #'emthin--delete-frame-hook)

(provide 'emthin-workspace)
;;; emthin-workspace.el ends here
