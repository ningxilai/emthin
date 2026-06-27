;;; emthin-mirrors.el --- Mirror management for emthin  -*- lexical-binding: t; -*-

(require 'cl-lib)
(require 'emthin-ipc)
(require 'emthin-geom)
(require 'emthin-app)

;; ── Mirror state ──

(defvar emthin--mirror-table (make-hash-table :test 'eql)
  "Tracks source and mirror windows per embedded app.
Key: window-id.  Value: (SOURCE-WIN . ((VIEW-ID . WIN) ...)).")

(defvar emthin--next-view-id 0
  "Counter for generating unique mirror view IDs.")

(defun emthin--alloc-view-id ()
  "Allocate a unique mirror view ID."
  (cl-incf emthin--next-view-id))

;; ── Mirror diff (pure) ──

(defun emthin--mirror-diff (wins prev-source prev-mirrors next-view-id)
  "Pure: compute mirror diff given WINS, PREV-SOURCE, PREV-MIRRORS, NEXT-VIEW-ID.

Returns (DIFF-PLIST . NEW-NEXT-VIEW-ID).  DIFF-PLIST:
  :source-win      — the window to use as source (or nil)
  :promote-vid     — vid of a mirror to promote, or nil
  :mirror-removals — vids of stale mirrors to remove
  :mirror-additions — ((VID . WIN) ...) for new mirrors
  :mirror-updates   — ((VID . WIN) ...) for existing mirrors
  :new-mirrors      — (SOURCE-WIN . ((VID . WIN) ...)) for mirror-table"
  (if (not wins)
      (cons (list :source-win nil :promote-vid nil
                  :mirror-removals (mapcar #'car prev-mirrors)
                  :mirror-additions nil :mirror-updates nil
                  :new-mirrors nil)
            next-view-id)
    (let* ((source-win (if (and prev-source (memq prev-source wins))
                           prev-source
                         (car wins)))
           (mirror-wins (remq source-win wins))
           (promote-vid (and prev-source (not (eq source-win prev-source))
                             (car (rassq source-win prev-mirrors))))
           (remaining (if promote-vid
                          (cl-remove promote-vid prev-mirrors :key #'car)
                        prev-mirrors))
           (old-by-win (make-hash-table :test 'eq))
           (new-mirrors nil) (removals nil)
           (additions nil) (updates nil))
      (dolist (m remaining)
        (puthash (cdr m) (car m) old-by-win))
      (dolist (mw mirror-wins)
        (if-let* ((vid (gethash mw old-by-win)))
            (progn
              (push (cons vid mw) updates)
              (push (cons vid mw) new-mirrors))
          (push (cons next-view-id mw) additions)
          (push (cons next-view-id mw) new-mirrors)
          (setq next-view-id (1+ next-view-id)))
        (remhash mw old-by-win))
      (maphash (lambda (_win vid) (push vid removals)) old-by-win)
      (when (and prev-source (not (eq source-win prev-source)) (not promote-vid))
        (setq removals (append (mapcar #'car prev-mirrors) removals)))
      (cons (list :source-win source-win
                  :promote-vid promote-vid
                  :mirror-removals removals
                  :mirror-additions (nreverse additions)
                  :mirror-updates (nreverse updates)
                  :new-mirrors (cons source-win (nreverse new-mirrors)))
            next-view-id))))

;; ── Mirror IPC send ──

(defun emthin--send-mirror-geometry (method wid view-id win)
  "Send mirror geometry IPC for METHOD (add/update) given WID, VIEW-ID, WIN."
  (condition-case err
      (let ((geo (emthin--window-geometry win)))
        (emthin--send method
          `(:window_id ,wid :view_id ,view-id
            :x ,(emthin--rect-x geo) :y ,(emthin--rect-y geo)
            :w ,(emthin--rect-w geo) :h ,(emthin--rect-h geo))))
    (error
     (message "emthin: mirror geometry error for %s:%s: %s" wid view-id err))))

;; ── Workspace-switch cleanup ──

(defun emthin--mirror-cleanup (frame)
  "Remove mirror-table entries whose source window is not on FRAME.
Sends `remove-mirror` IPC for each stale entry and forgets them.
Meant to be called before `emthin--sync-frame' after workspace switch."
  (condition-case err
      (maphash
       (lambda (wid state)
         (let ((source-win (car state))
               (mirrors (cdr state)))
           (unless (and (window-live-p source-win)
                        (eq (window-frame source-win) frame))
             (dolist (pair mirrors)
               (emthin--send 'remove-mirror
                 `(:window_id ,wid :view_id ,(car pair))))
             (remhash wid emthin--mirror-table))))
       emthin--mirror-table)
    (error
     (message "emthin: mirror-cleanup error: %s" err))))

;; ── Dispatch hook handlers ──

(defun emthin--on-focus-view (window-id view-id)
  "Select the Emacs window displaying WINDOW-ID / VIEW-ID.
VIEW-ID 0 means the source window; otherwise look up the mirror alist."
  (let* ((state (gethash window-id emthin--mirror-table))
         (mirror-win (and state
                          (if (= view-id 0)
                              (car state)
                            (cdr (assq view-id (cdr state))))))
         (target (or (and mirror-win
                          (window-live-p mirror-win)
                          (eq (window-frame mirror-win) (selected-frame))
                          mirror-win)
                     (when-let* ((app (emthin--find-app window-id))
                                 (buf (oref app buffer)))
                       (get-buffer-window buf nil)))))
    (when (and target (window-live-p target))
      (select-window target))))

(add-hook 'emthin--focus-view-hook #'emthin--on-focus-view)

;; ── Interactive mirror promotion ──

(defun emthin-promote-mirror ()
  "Promote the mirror in the selected window to become the app source."
  (interactive)
  (let* ((buf (window-buffer))
         (wid (buffer-local-value 'emthin--window-id buf))
         (state (and wid (gethash wid emthin--mirror-table)))
         (mirrors (and state (cdr state)))
         (source-win (and state (car state)))
         (vid (and mirrors (car (rassq (selected-window) mirrors)))))
    (if (not state)
        (message "emthin: no embedded app in current window")
      (if (not vid)
          (message "emthin: current window is not a mirror")
        (emthin--send 'promote-mirror `(:window_id ,wid :view_id ,vid))
        (let ((new-view-id (emthin--alloc-view-id)))
          (puthash wid
                   (cons (selected-window)
                         (cons (cons new-view-id source-win)
                               (cl-remove vid mirrors :key #'car)))
                   emthin--mirror-table)
          (message "emthin: promoted mirror %d" vid))))))

(provide 'emthin-mirrors)
;;; emthin-mirrors.el ends here
