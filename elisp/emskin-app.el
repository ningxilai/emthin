;;; emskin-app.el --- Embedded app lifecycle, geometry, and mirrors  -*- lexical-binding: t; -*-

(require 'cl-lib)
(require 'emskin-ipc)

;; ---------------------------------------------------------------------------
;; Application state
;; ---------------------------------------------------------------------------

(defvar emskin--header-offset nil
  "Pixel height of external GTK bars (menu-bar + tool-bar).
Seeded once on the first compositor SurfaceSize event and kept
constant thereafter — it's a property of the Emacs GTK frame, not of
the compositor's surface, so re-measuring on every resize would race
with GTK and break app placement when a layer-shell bar appears or
disappears.")

(defvar-local emskin--window-id nil
  "emskin window_id for the embedded app in this buffer.")

(defvar-local emskin--visible nil
  "Whether this embedded app buffer is currently displayed in an Emacs window.")

(defvar-local emskin--last-geometry nil
  "Last geometry sent for this buffer's embedded app window, to skip no-op updates.")

(defvar emskin--mirror-table (make-hash-table :test 'eql)
  "Tracks source and mirror windows per embedded app.
Key: window-id.  Value: (SOURCE-WIN . ((VIEW-ID . EMACS-WIN) ...)).")

(defvar emskin--last-focused-wid 'unset
  "Last window-id sent via set_focus IPC.  Used as change-detection guard.")

(defvar emskin--next-view-id 0
  "Counter for generating unique mirror view IDs.")

;; ---------------------------------------------------------------------------
;; Workspace tracking
;; ---------------------------------------------------------------------------

(defvar emskin--frame-workspace-table (make-hash-table :test 'eq)
  "Maps Emacs frame objects to compositor workspace IDs.")

(defvar emskin--active-workspace-id nil
  "Currently active workspace ID in the compositor.")

;; ---------------------------------------------------------------------------
;; App window target queue (producer: emskin-launch, consumer: here)
;; ---------------------------------------------------------------------------

(defvar emskin--pending-app-targets nil
  "FIFO queue of windows reserved for newly created app buffers.
Each `emskin-open-app' call appends the currently selected window.
When the compositor later emits `window_created', emskin tries to display
the new app buffer in the oldest still-live queued window before falling
back to the generic `display-buffer' path.")

(defun emskin--take-app-target-window ()
  "Return and dequeue the next live target window for an app."
  (let (target)
    (while (and emskin--pending-app-targets
                (not (window-live-p target)))
      (setq target (pop emskin--pending-app-targets)))
    (when (window-live-p target)
      target)))

;; ---------------------------------------------------------------------------
;; Message dispatch
;; ---------------------------------------------------------------------------

(defun emskin--dispatch (msg)
  "Dispatch a parsed MSG hash-table from emskin."
  (let ((type (gethash "type" msg "")))
    (cond
     ((string= type "connected")
      (message "emskin: connected (version %s)" (gethash "version" msg "?"))
      (setq emskin--active-workspace-id 1)
      (puthash (selected-frame) 1 emskin--frame-workspace-table)
      (run-hooks 'emskin-connected-hook))
     ((string= type "error")
      (message "emskin error: %s" (gethash "msg" msg "")))
     ((string= type "window_created")
      (emskin--on-window-created (gethash "window_id" msg)
                                  (gethash "title" msg "")))
     ((string= type "window_destroyed")
      (emskin--on-window-destroyed (gethash "window_id" msg)))
     ((string= type "title_changed")
      (emskin--on-title-changed (gethash "window_id" msg)
                                 (gethash "title" msg "")))
     ((string= type "focus_view")
      (emskin--on-focus-view (gethash "window_id" msg)
                                 (gethash "view_id" msg)))
     ((string= type "surface_size")
      (let* ((w (gethash "width" msg))
             (h (gethash "height" msg))
             (frame-h (frame-pixel-height))
             (offset (or emskin--header-offset
                         (max 0 (- h frame-h)))))
        (setq emskin--header-offset offset)
        (message "emskin: surface=%sx%s bars=%dpx" w h offset)
        (dolist (frame (frame-list))
          (emskin--sync-frame frame))))
     ((string= type "x_wayland_ready")
      nil)
     (t
      (message "emskin: unknown message type %s" type)))))

(add-hook 'emskin--message-hook #'emskin--dispatch)

;; ---------------------------------------------------------------------------
;; Window lifecycle
;; ---------------------------------------------------------------------------

(defun emskin--on-window-created (window-id title)
  "Create/display a buffer for the new embedded app and send initial geometry."
  (let* ((buf-name (format "*emskin: %s*" (if (string-empty-p title) "app" title)))
         (buf (generate-new-buffer buf-name)))
    (with-current-buffer buf
      (setq-local emskin--window-id window-id)
      (setq-local mode-name "emskin")
      (setq-local buffer-read-only t)
      (setq-local left-fringe-width 0)
      (setq-local right-fringe-width 0)
      (setq-local left-margin-width 0)
      (setq-local right-margin-width 0)
      (setq-local cursor-type nil)
      (add-hook 'kill-buffer-hook #'emskin--kill-buffer-hook nil t)
      (add-hook 'post-command-hook #'emskin--post-command-prefix-done nil t))
    (let ((target (emskin--take-app-target-window)))
      (if target
          (set-window-buffer target buf)
        (display-buffer buf '((display-buffer-pop-up-window
                               display-buffer-use-some-window)
                              (inhibit-same-window . t)
                              (reusable-frames . nil)))))
    (when-let* ((win (get-buffer-window buf t)))
      (set-window-scroll-bars win 0 nil 0 nil)
      (emskin--report-geometry window-id win))
    (emskin--sync-focus)
    (message "emskin: embedded app ready (id=%s)" window-id)))

(defun emskin--find-buffer (window-id)
  "Return the buffer whose `emskin--window-id' equals WINDOW-ID, or nil."
  (seq-find (lambda (buf)
              (equal (buffer-local-value 'emskin--window-id buf) window-id))
            (buffer-list)))

(defun emskin--on-window-destroyed (window-id)
  "Close all Emacs windows/buffer for WINDOW-ID and restore focus."
  (when-let* ((buf (emskin--find-buffer window-id)))
    (with-current-buffer buf
      (setq-local emskin--window-id nil))
    (dolist (win (reverse (get-buffer-window-list buf nil t)))
      (when (window-deletable-p win)
        (delete-window win)))
    (kill-buffer buf)
    (remhash window-id emskin--mirror-table)
    (let ((next-wid (buffer-local-value 'emskin--window-id
                                        (window-buffer (selected-window)))))
      (emskin--send (if next-wid
                        `((type . "set_focus") (window_id . ,next-wid))
                      '((type . "set_focus")))))
    (message "emskin: window %s destroyed" window-id)))

(defun emskin--on-title-changed (window-id title)
  "Rename the embedded app buffer when the app title changes."
  (when-let* ((buf (emskin--find-buffer window-id)))
    (with-current-buffer buf
      (rename-buffer (format "*emskin: %s*" title) t))))

(defun emskin--on-focus-view (window-id view-id)
  "Select the Emacs window that corresponds to WINDOW-ID / VIEW-ID.
VIEW-ID 0 means the source window; otherwise look up the mirror alist."
  (let* ((state (gethash window-id emskin--mirror-table))
         (target (when state
                   (if (= view-id 0)
                       (car state)
                     (cdr (assq view-id (cdr state)))))))
    (unless (and target (window-live-p target)
                 (eq (window-frame target) (selected-frame)))
      (when-let* ((buf (emskin--find-buffer window-id)))
        (setq target (get-buffer-window buf nil))))
    (when (and target (window-live-p target))
      (select-window target))))

(defun emskin--kill-buffer-hook ()
  "Notify emskin to close the app when its Emacs buffer is killed."
  (when emskin--window-id
    (emskin--send `((type . "close")
                        (window_id . ,emskin--window-id)))))

(defun emskin--post-command-prefix-done ()
  "After a command completes in an embedded app buffer, ask the
compositor to restore keyboard focus to the embedded app (clearing
prefix state along the way). Registered buffer-locally — only fires
when the post-command tick runs while the app buffer is still current."
  (when emskin--process
    (emskin--send '((type . "prefix_done")))))

(defun emskin--post-command-prefix-clear ()
  "Clear the compositor's `prefix_active' flag after every Emacs
command, in any buffer.

emSkin disables host IME for the duration of an Emacs prefix chord
(C-x ...) so the chord doesn't get eaten by fcitx5. Without a global
clear, plain Emacs chords like `C-x b' (which switch to a non-app
buffer, so the buffer-local `prefix_done' hook above doesn't fire)
would leave host IME disabled until the user clicks out and back.

Unlike `prefix_done', this signal does NOT restore focus — focus
follows whatever Emacs's prefix command did. The IPC handler is a
no-op when no prefix is active, so per-command firing is cheap."
  (when emskin--process
    (emskin--send '((type . "prefix_clear")))))

(add-hook 'post-command-hook #'emskin--post-command-prefix-clear)

;; ---------------------------------------------------------------------------
;; Geometry reporting
;; ---------------------------------------------------------------------------

(defun emskin--frame-header-offset (&optional _frame)
  "Pixel height of external GTK bars (menu-bar + tool-bar).
Computed once when the compositor reports the surface size."
  (or emskin--header-offset 0))

(defun emskin--window-geometry (window)
  "Return (x y w h) in pixels for Emacs WINDOW.
Coordinates are relative to the top-left of the Wayland surface.
Covers the body area (excludes fringes, margins, header-line, mode-line)."
  (let* ((body (window-body-pixel-edges window))
         (off (emskin--frame-header-offset (window-frame window)))
         (x (nth 0 body))
         (raw-y (nth 1 body))
         (y (+ raw-y off))
         (w (- (nth 2 body) x))
         (h (- (nth 3 body) raw-y)))
    (list x y w h)))

(defun emskin--report-geometry (window-id window)
  "Send set_geometry for WINDOW-ID, logging geometry to *Messages*."
  (condition-case err
      (let* ((frame (selected-frame))
             (geom (frame-geometry frame))
             (mb-h (or (cdr (alist-get 'menu-bar-size geom)) 0))
             (tb-h (or (cdr (alist-get 'tool-bar-size geom)) 0))
             (geo (emskin--window-geometry window)))
        (message "emskin: window %s geo=%s mb=%s tb=%s" window-id geo mb-h tb-h)
        (unless (equal geo (buffer-local-value 'emskin--last-geometry
                                               (window-buffer window)))
          (with-current-buffer (window-buffer window)
            (setq-local emskin--last-geometry geo))
          (emskin--send `((type . "set_geometry")
                          (window_id . ,window-id)
                          (x . ,(nth 0 geo))
                          (y . ,(nth 1 geo))
                          (w . ,(nth 2 geo))
                          (h . ,(nth 3 geo))))))
    (error
     (message "emskin: geometry error for window %s: %s" window-id err))))

(defun emskin--alloc-view-id ()
  "Allocate a unique mirror view ID."
  (cl-incf emskin--next-view-id))

(defun emskin--send-mirror-geometry (wid view-id win msg-type)
  "Send mirror geometry IPC for WID/VIEW-ID at Emacs WIN position."
  (let ((geo (emskin--window-geometry win)))
    (emskin--send `((type . ,msg-type)
                    (window_id . ,wid)
                    (view_id . ,view-id)
                    (x . ,(nth 0 geo))
                    (y . ,(nth 1 geo))
                    (w . ,(nth 2 geo))
                    (h . ,(nth 3 geo))))))

;; ---------------------------------------------------------------------------
;; Per-frame sync (geometry + visibility + mirrors)
;; ---------------------------------------------------------------------------

(defun emskin--sync-frame (frame)
  "Sync visibility, geometry, and mirrors for embedded app buffers in FRAME.
Only processes FRAME — never iterates other frames.  Skips the sync
if FRAME does not belong to the active workspace, which eliminates
race conditions during workspace switches."
  (when emskin--process
    (let ((ws-id (gethash frame emskin--frame-workspace-table)))
      (when (eql ws-id emskin--active-workspace-id)
        (let ((wid-wins (make-hash-table :test 'eql)))
          (dolist (win (window-list frame 'no-minibuf))
            (when-let* ((wid (buffer-local-value 'emskin--window-id
                                                (window-buffer win))))
              (set-window-scroll-bars win 0 nil 0 nil)
              (set-window-fringes win 0 0)
              (set-window-margins win 0 0)
              (puthash wid (append (gethash wid wid-wins) (list win)) wid-wins)))
          (dolist (buf (buffer-list))
            (when-let* ((wid (buffer-local-value 'emskin--window-id buf)))
              (let* ((wins (gethash wid wid-wins))
                     (now-visible (and wins t))
                     (was-visible (buffer-local-value 'emskin--visible buf))
                     (prev-state (gethash wid emskin--mirror-table))
                     (prev-source (car prev-state))
                     (prev-mirrors (cdr prev-state)))
                (unless (eq now-visible was-visible)
                  (with-current-buffer buf
                    (setq-local emskin--visible now-visible))
                  (emskin--send `((type . "set_visibility")
                                  (window_id . ,wid)
                                  (visible . ,(if now-visible t :json-false)))))
                (if (not wins)
                    (progn
                      (dolist (m prev-mirrors)
                        (emskin--send `((type . "remove_mirror")
                                        (window_id . ,wid)
                                        (view_id . ,(car m)))))
                      (remhash wid emskin--mirror-table))
                  (let* ((source-win (if (and prev-source (memq prev-source wins))
                                         prev-source
                                       (car wins)))
                         (mirror-wins (remq source-win wins))
                         (new-mirrors nil))
                    (when (and prev-source (not (eq source-win prev-source)))
                      (if-let* ((promoted-id (car (rassq source-win prev-mirrors))))
                          (progn
                            (emskin--send `((type . "promote_mirror")
                                            (window_id . ,wid)
                                            (view_id . ,promoted-id)))
                            (setq prev-mirrors (cl-remove promoted-id prev-mirrors
                                                          :key #'car)))
                        (dolist (m prev-mirrors)
                          (emskin--send `((type . "remove_mirror")
                                          (window_id . ,wid)
                                          (view_id . ,(car m)))))
                        (setq prev-mirrors nil)))
                    (emskin--report-geometry wid source-win)
                    (let ((old-by-win (make-hash-table :test 'eq)))
                      (dolist (m prev-mirrors)
                        (puthash (cdr m) (car m) old-by-win))
                      (dolist (mw mirror-wins)
                        (let ((vid (or (gethash mw old-by-win)
                                       (emskin--alloc-view-id))))
                          (push (cons vid mw) new-mirrors)
                          (if (gethash mw old-by-win)
                              (emskin--send-mirror-geometry
                               wid vid mw "update_mirror_geometry")
                            (emskin--send-mirror-geometry
                             wid vid mw "add_mirror"))
                          (remhash mw old-by-win)))
                      (maphash (lambda (_win vid)
                                 (emskin--send `((type . "remove_mirror")
                                                 (window_id . ,wid)
                                                 (view_id . ,vid))))
                               old-by-win))
                    (puthash wid (cons source-win (nreverse new-mirrors))
                             emskin--mirror-table)))))))))))

;; ---------------------------------------------------------------------------
;; Mirror promotion (interactive)
;; ---------------------------------------------------------------------------

(defun emskin-promote-mirror ()
  "Promote the mirror in the selected window to become the app source.
The compositor will adopt the mirror's geometry as the app's new
position; the old source window becomes a mirror in its place."
  (interactive)
  (let* ((buf (window-buffer))
         (wid (buffer-local-value 'emskin--window-id buf))
         (state (gethash wid emskin--mirror-table)))
    (if (not (and wid state))
        (message "emskin: no embedded app in current window")
      (let ((mirrors (cdr state))
            (source-win (car state)))
        (if-let* ((vid (car (rassq (selected-window) mirrors))))
            (progn
              (emskin--send `((type . "promote_mirror")
                              (window_id . ,wid)
                              (view_id . ,vid)))
              (let ((new-view-id (emskin--alloc-view-id)))
                (puthash wid
                         (cons (selected-window)
                               (cons (cons new-view-id source-win)
                                     (cl-remove vid mirrors :key #'car)))
                         emskin--mirror-table))
              (message "emskin: promoted mirror %d" vid))
        (message "emskin: current window is not a mirror"))))))

(add-hook 'window-size-change-functions #'emskin--sync-frame)
(add-hook 'window-buffer-change-functions #'emskin--sync-frame)

;; ---------------------------------------------------------------------------
;; Focus sync
;; ---------------------------------------------------------------------------

(defun emskin--sync-focus (&optional _frame)
  "Tell the compositor which surface should have keyboard focus.
When the selected window shows an embedded app buffer, focus the app;
otherwise focus Emacs.  Skips IPC when focus hasn't changed."
  (when emskin--process
    (let ((wid (buffer-local-value 'emskin--window-id
                                   (window-buffer (selected-window)))))
      (unless (eq wid emskin--last-focused-wid)
        (setq emskin--last-focused-wid wid)
        (emskin--send (if wid
                          `((type . "set_focus") (window_id . ,wid))
                        '((type . "set_focus"))))))))

(add-hook 'window-selection-change-functions #'emskin--sync-focus)

(provide 'emskin-app)
;;; emskin-app.el ends here
