;;; emthin-sync.el --- Frame sync pass for emthin  -*- lexical-binding: t; -*-

(require 'emthin-ipc)
(require 'emthin-geom)
(require 'emthin-app)

;; ── Global state ──

(defvar emthin--last-focused-wid 'unset
  "Last window-id sent via set_focus IPC.  Change-detection guard.")

;; ── Data collection ──

(defun emthin--wid-wins-data (frame)
  "Return hash-table wid→(win...) for FRAME (pure collection)."
  (let ((wid-wins (make-hash-table :test 'eql)))
    (dolist (win (window-list frame 'no-minibuf))
      (when-let* ((wid (buffer-local-value 'emthin--window-id
                                            (window-buffer win))))
        (puthash wid (cons win (gethash wid wid-wins)) wid-wins)))
    wid-wins))

;; ── Geometry apply ──

(defun emthin--apply-geometry (app window)
  "Send set_geometry for APP if its geometry changed.
Computes geometry via `emthin--compute-layout' on the app's layout object."
  (condition-case err
      (let* ((geo (emthin--compute-layout (oref app layout) window app))
             (old (oref app last-geometry)))
        (unless (equal geo old)
          (oset app last-geometry geo)
          (emthin--send 'set-geometry
            `(:window_id ,(oref app window-id)
              :x ,(emthin--rect-x geo)
              :y ,(emthin--rect-y geo)
              :w ,(emthin--rect-w geo)
              :h ,(emthin--rect-h geo)))))
    (error
     (message "emthin: geometry error for window %s: %s"
              (oref app window-id) err))))

;; ── Visibility apply ──

(defun emthin--apply-visible (window-id now-visible was-visible)
  "Send set_visibility if visibility changed."
  (unless (eq now-visible was-visible)
    (when-let* ((app (emthin--find-app window-id)))
      (with-current-buffer (oref app buffer)
        (setq-local emthin--visible now-visible)))
    (emthin--send 'set-visibility
      `(:window_id ,window-id
        :visible ,(if now-visible t :json-false)))))

;; ── Frame sync ──

(defun emthin--sync-frame (frame)
  "Sync visibility and geometry for embedded app buffers in FRAME.
Only processes the active workspace's frame."
  (when emthin--process
    (let ((ws-id (gethash frame emthin--frame-workspace-table)))
      (when (eql ws-id emthin--active-workspace-id)
        (condition-case err
            (let ((wid-wins (emthin--wid-wins-data frame)))
              (maphash (lambda (wid wins)
                         (let* ((now-visible (and wins t))
                                (app (emthin--find-app wid))
                                (was-visible (if app
                                               (buffer-local-value
                                                'emthin--visible (oref app buffer))
                                             nil)))
                           (emthin--apply-visible wid now-visible was-visible)
                           (when wins
                             (emthin--apply-geometry app (car wins)))))
                       wid-wins))
          (error
           (message "emthin: sync-frame error: %s" err)))
        (emthin--sync-focus frame)))))

(add-hook 'window-size-change-functions  #'emthin--sync-frame)
(add-hook 'window-buffer-change-functions #'emthin--sync-frame)

;; ── Focus sync ──

(defun emthin--sync-focus (&optional _frame)
  "Sync focus if the focused window's app changed."
  (when emthin--process
    (condition-case err
        (let ((wid (buffer-local-value 'emthin--window-id
                                        (window-buffer (selected-window)))))
          (unless (eq wid emthin--last-focused-wid)
            (setq emthin--last-focused-wid wid)
            (if wid
                (emthin--send 'set-focus `(:window_id ,wid))
              (emthin--send 'set-focus))))
      (error
       (message "emthin: focus sync error: %s" err)))))

(add-hook 'window-selection-change-functions #'emthin--sync-focus)

(provide 'emthin-sync)
;;; emthin-sync.el ends here
