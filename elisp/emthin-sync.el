;;; emthin-sync.el --- Frame sync pass for emthin  -*- lexical-binding: t; -*-

(require 'emthin-ipc)
(require 'emthin-geom)
(require 'emthin-app)
(require 'emthin-mirrors)
(require 'emthin-layout)

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
      (let* ((geo (emthin--compute-layout (oref app layout) window (emthin--frame-header-offset)))
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

(defun emthin--sync-mirrors (wid diff)
  "Apply mirror DIFF plist for WID synchronously."
  (condition-case err
      (let ((source-win (plist-get diff :source-win))
            (promote-vid (plist-get diff :promote-vid))
            (removals (plist-get diff :mirror-removals))
            (additions (plist-get diff :mirror-additions))
            (updates (plist-get diff :mirror-updates))
            (new-mirrors (plist-get diff :new-mirrors)))
        (dolist (vid removals)
          (emthin--send 'remove-mirror `(:window_id ,wid :view_id ,vid)))
        (when promote-vid
          (emthin--send 'promote-mirror `(:window_id ,wid :view_id ,promote-vid)))
        (dolist (pair additions)
          (emthin--send-mirror-geometry 'add-mirror wid (car pair) (cdr pair)))
        (dolist (pair updates)
          (emthin--send-mirror-geometry 'update-mirror-geometry wid (car pair) (cdr pair)))
        (when source-win
          (when-let* ((app (emthin--find-app wid)))
            (emthin--apply-geometry app source-win)))
        (if source-win
            (puthash wid new-mirrors emthin--mirror-table)
          (remhash wid emthin--mirror-table)))
    (error
     (message "emthin: mirror sync error for window %s: %s" wid err))))

;; ── Sync strategy (dispatched on frame-level layout) ──

(cl-defgeneric emthin--sync-apps (layout frame)
  "Sync visibility and geometry for all apps in FRAME under LAYOUT.")

(cl-defmethod emthin--sync-apps ((_layout emthin-layout-tab) frame)
  "Tab: iterate all apps, only actively displayed buffer visible."
  (maphash
   (lambda (_wid app)
     (let* ((buf (oref app buffer))
            (win (get-buffer-window buf frame))
            (vis (and win t))
            (prev (buffer-local-value 'emthin--visible buf)))
       (emthin--apply-visible (oref app window-id) vis prev)
       (when win
         (emthin--apply-geometry app win))))
   emthin--app-table))

(cl-defmethod emthin--sync-apps (_layout frame)
  "Default (nil / fill / float / unknown): wid-wins."
  (let ((wid-wins (emthin--wid-wins-data frame)))
    (maphash (lambda (wid wins)
               (let* ((now-visible (and wins t))
                      (app (emthin--find-app wid))
                      (was-visible (if app
                                     (buffer-local-value
                                      'emthin--visible (oref app buffer))
                                   nil)))
                 (emthin--apply-visible wid now-visible was-visible)
                 (when app
                   (emthin--apply-geometry app (car wins)))))
             wid-wins)))

;; ── Frame sync ──

(defun emthin--sync-frame (frame)
  "Sync visibility, geometry, and mirrors for embedded app buffers in FRAME.
Only processes the active workspace's frame."
  (when emthin--process
    (let ((ws-id (gethash frame emthin--frame-workspace-table)))
      (when (eql ws-id emthin--active-workspace-id)
        (let ((next-view-id emthin--next-view-id))
          (unwind-protect
              (progn
                (condition-case err
                    (emthin--sync-apps emthin--frame-layout frame)
                  (error
                   (message "emthin: sync-apps error: %s" err)))
                ;; Mirror sync: compute diffs and send IPC for each app.
                (condition-case err
                    (let ((wid-wins (emthin--wid-wins-data frame)))
                      (maphash
                       (lambda (wid wins)
                         (let* ((prev-state (gethash wid emthin--mirror-table))
                                (prev-source (car prev-state))
                                (prev-mirrors (cdr prev-state))
                                (mirror-result (emthin--mirror-diff
                                                wins prev-source prev-mirrors
                                                next-view-id)))
                           (setq next-view-id (cdr mirror-result))
                           (emthin--sync-mirrors wid (car mirror-result))))
                       wid-wins))
                  (error
                   (message "emthin: mirror sync error: %s" err))))
            (emthin--sync-focus frame)
            (setq emthin--next-view-id next-view-id)))))))

(add-hook 'window-size-change-functions  #'emthin--sync-frame)
(add-hook 'window-buffer-change-functions #'emthin--sync-frame)

;; ── Focus sync ──

(defun emthin--sync-focus (&optional _frame)
  "Sync focus if the focused window's app changed."
  (when emthin--process
    (condition-case err
        (let ((wid (and (local-variable-p 'emthin--window-id
                                           (window-buffer (selected-window)))
                        (buffer-local-value 'emthin--window-id
                                            (window-buffer (selected-window))))))
          (unless (eq wid emthin--last-focused-wid)
            (setq emthin--last-focused-wid wid)
            (emthin--send 'set-focus (and wid `(:window_id ,wid)))))
      (error
       (message "emthin: focus sync error: %s" err)))))

(add-hook 'window-selection-change-functions #'emthin--sync-focus)

(provide 'emthin-sync)
;;; emthin-sync.el ends here
