;;; emthin-sync.el --- Frame sync pass for emthin  -*- lexical-binding: t; -*-

(require 'emthin-ipc)
(require 'emthin-geom)
(require 'emthin-app)
(require 'emthin-mirrors)
(require 'emthin-layout)

;; ── Global state ──

(defvar emthin--last-focused-wid 'unset
  "Last window-id sent via set_focus IPC.  Change-detection guard.")

;; ── Interactive layout switching ──

(defun emthin-set-layout (layout)
  "Set frame-level layout strategy.
LAYOUT is a symbol: `fill', `tab', `side-by-side', or `float'."
  (interactive
   (list (intern (completing-read "Layout: "
                                  '("fill" "tab" "side-by-side" "float")
                                  nil t))))
  (setq emthin--frame-layout
        (pcase layout
          ('fill (make-instance 'emthin-layout-fill))
          ('tab (make-instance 'emthin-layout-tab))
          ('side-by-side (make-instance 'emthin-layout-side-by-side))
          ('float (make-instance 'emthin-layout-float))
          (_ (user-error "Unknown layout: %s" layout))))
  (message "emthin: layout set to %s" layout))

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
      (let* ((geo (emthin--compute-layout (oref app layout) window
                                          (emthin--frame-header-offset)))
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

;; ── Mirror IPC sync ──

(defun emthin--sync-mirrors (wid diff &optional no-update-geometry)
  "Apply mirror DIFF plist for WID synchronously.
When NO-UPDATE-GEOMETRY is non-nil, skip update-mirror-geometry IPC
(the frame-level layout has already set correct mirror geometry)."
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
        (unless no-update-geometry
          (dolist (pair updates)
            (emthin--send-mirror-geometry 'update-mirror-geometry
              wid (car pair) (cdr pair))))
        (when source-win
          (when-let* ((app (emthin--find-app wid)))
            (emthin--apply-geometry app source-win)))
        (if source-win
            (puthash wid new-mirrors emthin--mirror-table)
          (remhash wid emthin--mirror-table)))
    (error
     (message "emthin: mirror sync error for window %s: %s" wid err))))

;; ── Sync strategy (dispatched on frame-level layout) ──

(cl-defmethod emthin--sync-apps ((_layout emthin-layout-tab) _wid-wins _mirror-table)
  "Tab: iterate all apps, only actively displayed buffer visible."
  (maphash
   (lambda (_wid app)
     (let* ((frame (selected-frame))
            (buf (oref app buffer))
            (win (get-buffer-window buf frame))
            (vis (and win t))
            (prev (buffer-local-value 'emthin--visible buf)))
       (emthin--apply-visible (oref app window-id) vis prev)
       (when win
         (emthin--apply-geometry app win))))
   emthin--app-table))

(cl-defmethod emthin--sync-apps ((layout emthin-layout-side-by-side)
                                  wid-wins mirror-table)
  "Side-by-side: source in main area, mirrors as thumbnails on the right."
  (maphash
   (lambda (wid wins)
     (let* ((state (gethash wid mirror-table))
            (source-win (if state (car state) (car wins)))
            (mirrors (and state (cdr state)))
            (num-mirrors (length mirrors)))
       (when source-win
         (let ((app (emthin--find-app wid)))
           (when app
             (emthin--apply-visible wid (and wins t)
               (buffer-local-value 'emthin--visible (oref app buffer)))
             (emthin--apply-geometry app source-win)))
         (let ((side-x (oref layout source-ratio))
               (side-w (- 1.0 (oref layout source-ratio)))
               (mirror-h (if (zerop num-mirrors) 0 (/ 1.0 num-mirrors))))
           (cl-loop for pair in mirrors
                    for i from 0
                    for vid = (car pair)
                    do
                    (let ((rect (make-emthin--rect
                                  :x side-x :y (* i mirror-h)
                                  :w side-w :h mirror-h)))
                      (emthin--send 'update-mirror-geometry
                        `(:window_id ,wid :view_id ,vid
                          :x ,(emthin--rect-x rect)
                          :y ,(emthin--rect-y rect)
                          :w ,(emthin--rect-w rect)
                          :h ,(emthin--rect-h rect)))))))))
   wid-wins))

;; ── Frame sync ──

(defun emthin--sync-frame (frame)
  "Sync visibility, geometry, and mirrors for embedded app buffers in FRAME.
Only processes the active workspace's frame."
  (when emthin--process
    (let ((ws-id (gethash frame emthin--frame-workspace-table)))
      (when (eql ws-id emthin--active-workspace-id)
        (let ((next-view-id emthin--next-view-id)
              (wid-wins (emthin--wid-wins-data frame)))
          (unwind-protect
              (progn
                (condition-case err
                    (emthin--sync-apps emthin--frame-layout wid-wins
                                       emthin--mirror-table)
                  (error
                   (message "emthin: sync-apps error: %s" err)))
                (condition-case err
                    (maphash
                     (lambda (wid wins)
                       (let* ((prev-state (gethash wid emthin--mirror-table))
                              (prev-source (car prev-state))
                              (prev-mirrors (cdr prev-state))
                              (mirror-result (emthin--mirror-diff
                                              wins prev-source prev-mirrors
                                              next-view-id)))
                         (setq next-view-id (cdr mirror-result))
                         (emthin--sync-mirrors wid (car mirror-result)
                           (emthin--layout-manages-mirror-geometry
                            emthin--frame-layout))))
                     wid-wins)
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
