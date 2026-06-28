;;; emthin-workspace.el --- Workspace management for emthin  -*- lexical-binding: t; -*-

(require 'emthin-app)
(require 'emthin-sync)
(require 'emthin-mirrors)
(require 'emthin-ipc)

;; ── Workspace-local state ──
;; emthin--frame-workspace-table, emthin--ws-to-frame-table, and
;; emthin--active-workspace-id are defined in emthin-app.el.

(defvar emthin--pending-frame-queue nil
  "Frames awaiting workspace_created IPC confirmation (FIFO).")

(defvar emthin--last-command-frame nil
  "Frame at the end of the last command.  Used for change detection.")

;; ── Frame ↔ workspace mapping ──

(defun emthin--map-frame-to-workspace (frame workspace-id)
  "Map FRAME to WORKSPACE-ID in both tables."
  (puthash frame workspace-id emthin--frame-workspace-table)
  (puthash workspace-id frame emthin--ws-to-frame-table))

(defun emthin--unmap-frame (frame)
  "Remove FRAME from workspace tables. Idempotent."
  (let ((ws-id (gethash frame emthin--frame-workspace-table)))
    (remhash frame emthin--frame-workspace-table)
    (when ws-id
      (remhash ws-id emthin--ws-to-frame-table))))

(defun emthin--active-frame ()
  "Return the Emacs frame for the active workspace."
  (gethash emthin--active-workspace-id emthin--ws-to-frame-table))

;; ── Workspace lifecycle (dispatch hook handlers) ──

(defun emthin--on-workspace-created (workspace-id)
  "Associate the most recently created frame with WORKSPACE-ID."
  (if emthin--pending-frame-queue
      (let ((frame (pop emthin--pending-frame-queue)))
        (when (frame-live-p frame)
          (emthin--map-frame-to-workspace frame workspace-id)
          (message "emthin: frame → workspace %d" workspace-id)
          (emthin--sync-frame frame)))
    (emthin--map-frame-to-workspace (selected-frame) workspace-id)))

(defun emthin--on-workspace-switched (workspace-id)
  "Update active workspace tracking and re-sync."
  (setq emthin--active-workspace-id workspace-id)
  (setq emthin--last-focused-wid 'unset)
  (setq emthin--last-command-frame (selected-frame))
  (emthin--resync-workspace)
  (emthin--sync-focus (selected-window)))

(defun emthin--on-workspace-destroyed (workspace-id)
  "Clean up frame-workspace mapping for destroyed workspace."
  (maphash (lambda (frame ws-id)
             (when (eql ws-id workspace-id)
               (emthin--unmap-frame frame)))
           emthin--frame-workspace-table))

;; ── Resync ──

(defun emthin--resync-workspace ()
  "Force full re-sync for the active workspace's frame."
  (when-let* ((fr (emthin--active-frame)))
    (condition-case err
        (progn
          (emthin--mirror-cleanup fr)
          (walk-windows (lambda (win)
                          (let* ((buf (window-buffer win))
                                 (wid (buffer-local-value 'emthin--window-id buf)))
                            (when-let* ((app (emthin--find-app wid)))
                              (oset app last-geometry nil))))
                        nil fr)
          (emthin--sync-frame fr))
      (error
       (message "emthin: resync error: %s" err)))))

;; ── Frame creation / deletion hooks ──

(defun emthin--after-make-frame (frame)
  "Queue FRAME for workspace association."
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

;; ── Frame switch detection via post-command-hook ──

(defun emthin--detect-frame-switch ()
  "Detect frame change and notify compositor.
Runs on `post-command-hook', avoids any advice or focus-change hook."
  (when emthin--process
    (condition-case err
        (let ((frame (selected-frame)))
          (when (and emthin--last-command-frame
                     (not (eq frame emthin--last-command-frame)))
            (when-let* ((ws-id (gethash frame emthin--frame-workspace-table)))
              (unless (eql ws-id emthin--active-workspace-id)
                (emthin--send 'switch-workspace `(:workspace_id ,ws-id)))))
          (setq emthin--last-command-frame frame))
      (error
       (message "emthin: frame-switch error: %s" err)))))

(add-hook 'post-command-hook #'emthin--detect-frame-switch)

;; ── Register hooks ──

(add-hook 'after-make-frame-functions #'emthin--after-make-frame)
(add-hook 'delete-frame-functions #'emthin--delete-frame-hook)

(add-hook 'emthin--workspace-created-hook  #'emthin--on-workspace-created)
(add-hook 'emthin--workspace-switched-hook #'emthin--on-workspace-switched)
(add-hook 'emthin--workspace-destroyed-hook #'emthin--on-workspace-destroyed)

(provide 'emthin-workspace)
;;; emthin-workspace.el ends here
