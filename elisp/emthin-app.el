;;; emthin-app.el --- Embedded app lifecycle, geometry, and mirrors  -*- lexical-binding: t; -*-

(require 'cl-lib)
(require 'subr-x)
(require 'emthin-ipc)

;; ---------------------------------------------------------------------------
;; Application state
;; ---------------------------------------------------------------------------

(defvar emthin--header-offset nil
  "Pixel height of external GTK bars (menu-bar + tool-bar).
Seeded once on the first compositor SurfaceSize event and kept
constant thereafter — it's a property of the Emacs GTK frame, not of
the compositor's surface, so re-measuring on every resize would race
with GTK and break app placement when a layer-shell bar appears or
disappears.")

(defvar-local emthin--window-id nil
  "emthin window_id for the embedded app in this buffer.")

(defvar-local emthin--visible nil
  "Whether this embedded app buffer is currently displayed in an Emacs window.")

(defvar-local emthin--last-geometry nil
  "Last geometry sent for this buffer's embedded app window, to skip no-op updates.")

(defvar emthin--mirror-table (make-hash-table :test 'eql)
  "Tracks source and mirror windows per embedded app.
Key: window-id.  Value: (SOURCE-WIN . ((VIEW-ID . EMACS-WIN) ...)).")

(defvar emthin--last-focused-wid 'unset
  "Last window-id sent via set_focus IPC.  Used as change-detection guard.")

(defvar emthin--next-view-id 0
  "Counter for generating unique mirror view IDs.")

;; ---------------------------------------------------------------------------
;; Workspace tracking
;; ---------------------------------------------------------------------------

(defvar emthin--frame-workspace-table (make-hash-table :test 'eq)
  "Maps Emacs frame objects to compositor workspace IDs.")

(defvar emthin--ws-to-frame-table (make-hash-table :test 'eql)
  "Reverse mapping: workspace-id to frame.
Complements `emthin--frame-workspace-table'.")

(defvar emthin--active-workspace-id nil
  "Currently active workspace ID in the compositor.")

(defun emthin--map-frame-to-workspace (frame workspace-id)
  "Map FRAME to WORKSPACE-ID in both forward and reverse tables."
  (puthash frame workspace-id emthin--frame-workspace-table)
  (puthash workspace-id frame emthin--ws-to-frame-table))

(defun emthin--unmap-frame (frame)
  "Remove FRAME from workspace tables.  Idempotent."
  (let ((ws-id (gethash frame emthin--frame-workspace-table)))
    (remhash frame emthin--frame-workspace-table)
    (when ws-id
      (remhash ws-id emthin--ws-to-frame-table))))

;; ---------------------------------------------------------------------------
;; App window target queue (producer: emthin-launch, consumer: here)
;; ---------------------------------------------------------------------------

(defvar emthin--pending-app-targets nil
  "FIFO queue of windows reserved for newly created app buffers.
Each `emthin-open-app' call appends the currently selected window.
When the compositor later emits `window_created', emthin tries to display
the new app buffer in the oldest still-live queued window before falling
back to the generic `display-buffer' path.")

(defun emthin--take-app-target-window ()
  "Return and dequeue the next live target window for an app."
  (let (target)
    (while (and emthin--pending-app-targets
                (not (window-live-p target)))
      (setq target (pop emthin--pending-app-targets)))
    (when (window-live-p target)
      target)))

;; ---------------------------------------------------------------------------
;; Data accessors
;; ---------------------------------------------------------------------------

(defsubst emthin--rect (x y w h)
  "Create a rect from X Y W H."
  (list x y w h))

(defsubst emthin--rect-x (r) (nth 0 r))
(defsubst emthin--rect-y (r) (nth 1 r))
(defsubst emthin--rect-w (r) (nth 2 r))
(defsubst emthin--rect-h (r) (nth 3 r))

(defsubst emthin--mirror-source (table wid)
  "Return source window for WID in mirror TABLE."
  (car (gethash wid table)))

(defsubst emthin--mirror-mirrors (table wid)
  "Return mirror alist for WID in mirror TABLE."
  (cdr (gethash wid table)))

;; ---------------------------------------------------------------------------
;; IPC call helpers
;; ---------------------------------------------------------------------------

(defmacro emthin--call (method &rest plist)
  "Send a METHOD notification with alternating keyword-value PLIST.
METHOD is a kebab-case or snake_case symbol.  Example:

    (emthin--call set-focus :window_id 42)

expands to

    (emthin--send \\='set-focus (list :window_id 42))"
  (declare (indent 1))
  `(emthin--send ',method (list ,@plist)))

(defun emthin--call* (method &rest plist)
  "Runtime version of `emthin--call' macro.
Send a METHOD notification with alternating keyword-value PLIST."
  (emthin--send method plist))

;; ---------------------------------------------------------------------------
;; Message dispatch
;; ---------------------------------------------------------------------------

(defun emthin--dispatch (method params)
  "Dispatch a parsed METHOD with PARAMS plist from emthin."
  (pcase method
    ('connected
     (message "emthin: connected (version %s)"
              (or (plist-get params :version) "?"))
     (setq emthin--active-workspace-id 1)
     (emthin--map-frame-to-workspace (selected-frame) 1)
     (run-hooks 'emthin-connected-hook))
    ('error
     (message "emthin error: %s" (plist-get params :msg)))
    ('window_created
     (emthin--on-window-created (plist-get params :window_id)
                                 (or (plist-get params :title) "")))
    ('window_destroyed
     (emthin--on-window-destroyed (plist-get params :window_id)))
    ('title_changed
     (emthin--on-title-changed (plist-get params :window_id)
                                (or (plist-get params :title) "")))
    ('focus_view
     (emthin--on-focus-view (plist-get params :window_id)
                             (plist-get params :view_id)))
    ('surface_size
     (let* ((w (plist-get params :width))
            (h (plist-get params :height))
            (frame-h (frame-pixel-height))
            (offset (or emthin--header-offset
                        (max 0 (- h frame-h)))))
       (setq emthin--header-offset offset)
       (message "emthin: surface=%sx%s bars=%dpx" w h offset)
       (dolist (frame (frame-list))
         (emthin--sync-frame frame))))
    ('x_wayland_ready
     nil)
    ;; Handled by emthin-workspace.el hooks — silence the catch-all.
    ((or 'workspace_created 'workspace_switched 'workspace_destroyed)
     nil)
    (_
     (message "emthin: unknown message type %s" method))))

(add-hook 'emthin--message-hook #'emthin--dispatch)

;; ---------------------------------------------------------------------------
;; Window lifecycle
;; ---------------------------------------------------------------------------

(defun emthin--on-window-created (window-id title)
  "Create/display a buffer for the new embedded app and send initial geometry."
  (condition-case err
      (let* ((buf-name (format "*emthin: %s*" (if (string-empty-p title) "app" title)))
             (buf (generate-new-buffer buf-name)))
        (with-current-buffer buf
          (setq-local emthin--window-id window-id)
          (setq-local mode-name "emthin")
          (setq-local buffer-read-only t)
          (setq-local left-fringe-width 0)
          (setq-local right-fringe-width 0)
          (setq-local left-margin-width 0)
          (setq-local right-margin-width 0)
          (setq-local cursor-type nil)
          (add-hook 'kill-buffer-hook #'emthin--kill-buffer-hook nil t)
          (add-hook 'post-command-hook #'emthin--post-command-prefix-done nil t))
        (let ((target (emthin--take-app-target-window)))
          (if target
              (set-window-buffer target buf)
            (display-buffer buf '((display-buffer-pop-up-window
                                   display-buffer-use-some-window)
                                  (inhibit-same-window . t)
                                  (reusable-frames . nil)))))
        (when-let* ((win (get-buffer-window buf t)))
          (set-window-scroll-bars win 0 nil 0 nil)
          (emthin--report-geometry window-id win))
        (emthin--sync-focus)
        (message "emthin: embedded app ready (id=%s)" window-id))
    (error
     (message "emthin: window-created error (id=%s): %s" window-id err))))

(defun emthin--find-buffer (window-id)
  "Return the buffer whose `emthin--window-id' equals WINDOW-ID, or nil."
  (seq-find (lambda (buf)
              (equal (buffer-local-value 'emthin--window-id buf) window-id))
            (buffer-list)))

(defun emthin--on-window-destroyed (window-id)
  "Close all Emacs windows/buffer for WINDOW-ID and restore focus."
  (condition-case err
      (when-let* ((buf (emthin--find-buffer window-id)))
        (with-current-buffer buf
          (setq-local emthin--window-id nil))
        (dolist (win (reverse (get-buffer-window-list buf nil t)))
          (when (window-deletable-p win)
            (delete-window win)))
        (kill-buffer buf)
        (remhash window-id emthin--mirror-table)
        (let ((next-wid (buffer-local-value 'emthin--window-id
                                            (window-buffer (selected-window)))))
          (if next-wid
              (emthin--call set-focus :window_id next-wid)
            (emthin--call set-focus)))
        (message "emthin: window %s destroyed" window-id))
    (error
     (message "emthin: window-destroyed error (id=%s): %s" window-id err))))

(defun emthin--on-title-changed (window-id title)
  "Rename the embedded app buffer when the app title changes."
  (when-let* ((buf (emthin--find-buffer window-id)))
    (with-current-buffer buf
      (rename-buffer (format "*emthin: %s*" title) t))))

(defun emthin--on-focus-view (window-id view-id)
  "Select the Emacs window that corresponds to WINDOW-ID / VIEW-ID.
VIEW-ID 0 means the source window; otherwise look up the mirror alist."
  (let* ((state (gethash window-id emthin--mirror-table))
         (target (when state
                   (if (= view-id 0)
                       (car state)
                     (cdr (assq view-id (cdr state)))))))
    (unless (and target (window-live-p target)
                 (eq (window-frame target) (selected-frame)))
      (when-let* ((buf (emthin--find-buffer window-id)))
        (setq target (get-buffer-window buf nil))))
    (when (and target (window-live-p target))
      (select-window target))))

(defun emthin--kill-buffer-hook ()
  "Notify emthin to close the app when its Emacs buffer is killed."
  (condition-case err
      (when emthin--window-id
        (emthin--call close :window_id emthin--window-id))
    (error
     (message "emthin: kill-buffer-hook error: %s" err))))

(defun emthin--post-command-prefix-done ()
  "After a command completes in an embedded app buffer, ask the
compositor to restore keyboard focus to the embedded app (clearing
prefix state along the way). Registered buffer-locally — only fires
when the post-command tick runs while the app buffer is still current."
  (condition-case err
      (when emthin--process
        (emthin--call prefix-done))
    (error
     (message "emthin: prefix-done error: %s" err))))

(defun emthin--post-command-prefix-clear ()
  "Clear the compositor's `prefix_active' flag after every Emacs
command, in any buffer.

emthin disables host IME for the duration of an Emacs prefix chord
(C-x ...) so the chord doesn't get eaten by fcitx5. Without a global
clear, plain Emacs chords like `C-x b' (which switch to a non-app
buffer, so the buffer-local `prefix_done' hook above doesn't fire)
would leave host IME disabled until the user clicks out and back.

Unlike `prefix_done', this signal does NOT restore focus — focus
follows whatever Emacs's prefix command did. The IPC handler is a
no-op when no prefix is active, so per-command firing is cheap."
  (condition-case err
      (when emthin--process
        (emthin--call prefix-clear))
    (error
     (message "emthin: prefix-clear error: %s" err))))

(add-hook 'post-command-hook #'emthin--post-command-prefix-clear)

;; ---------------------------------------------------------------------------
;; Geometry reporting
;; ---------------------------------------------------------------------------

(defun emthin--frame-header-offset (&optional _frame)
  "Pixel height of external GTK bars (menu-bar + tool-bar).
Computed once when the compositor reports the surface size."
  (or emthin--header-offset 0))

(defun emthin--edges->rect (offset edges)
  "Convert pixel EDGES (X1 Y1 X2 Y2) to rect with header OFFSET."
  (emthin--rect (nth 0 edges)
                (+ (nth 1 edges) offset)
                (- (nth 2 edges) (nth 0 edges))
                (- (nth 3 edges) (nth 1 edges))))

(defun emthin--window-geometry (window)
  "Return rect for Emacs WINDOW body area in surface-local pixels.
Pipeline: window-body-pixel-edges → edges->rect."
  (thread-last
    (window-body-pixel-edges window)
    (emthin--edges->rect (emthin--frame-header-offset (window-frame window)))))

(defun emthin--alloc-view-id ()
  "Allocate a unique mirror view ID."
  (cl-incf emthin--next-view-id))

;; ---------------------------------------------------------------------------
;; Per-frame sync helpers — pure collection + diff, then apply
;; ---------------------------------------------------------------------------

(defun emthin--wid-wins-data (frame)
  "Return hash-table wid→(win...) for FRAME (pure collection)."
  (let ((wid-wins (make-hash-table :test 'eql)))
    (dolist (win (window-list frame 'no-minibuf))
      (when-let* ((wid (buffer-local-value 'emthin--window-id
                                          (window-buffer win))))
        (puthash wid (cons win (gethash wid wid-wins)) wid-wins)))
    wid-wins))

(defun emthin--mirror-diff (wins prev-source prev-mirrors next-view-id)
  "Pure: compute mirror diff given WINS, PREV-SOURCE, PREV-MIRRORS, NEXT-VIEW-ID.

Returns (DIFF-PLIST . NEW-NEXT-VIEW-ID).  DIFF-PLIST has:
  :source-win   — the window to use as source (or nil)
  :promote-vid  — vid of a mirror to promote, or nil
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

(defun emthin--send-mirror-geometry* (wid view-id win msg-type)
  "Send mirror geometry IPC (utility, callable from thunks)."
  (let ((geo (emthin--window-geometry win))
        (method (intern msg-type)))
    (emthin--call* method
      :window_id wid :view_id view-id
      :x (emthin--rect-x geo) :y (emthin--rect-y geo)
      :w (emthin--rect-w geo) :h (emthin--rect-h geo))))

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
          (emthin--call* 'remove-mirror :window_id wid :view_id vid))
        (when promote-vid
          (emthin--call* 'promote-mirror :window_id wid :view_id promote-vid))
        (dolist (pair additions)
          (emthin--send-mirror-geometry* wid (car pair) (cdr pair) "add_mirror"))
        (dolist (pair updates)
          (emthin--send-mirror-geometry* wid (car pair) (cdr pair) "update_mirror_geometry"))
        (when source-win
          (emthin--report-geometry wid source-win))
        (if source-win
            (puthash wid new-mirrors emthin--mirror-table)
          (remhash wid emthin--mirror-table)))
    (error
     (message "emthin: mirror sync error for window %s: %s" wid err))))

(defun emthin--report-geometry (window-id window)
  "Send set_geometry for WINDOW-ID if geometry changed."
  (condition-case err
      (let* ((geo (emthin--window-geometry window))
             (buf (window-buffer window))
             (old-geo (buffer-local-value 'emthin--last-geometry buf)))
        (message "emthin: window %s geo=%s" window-id geo)
        (unless (equal geo old-geo)
          (with-current-buffer buf
            (setq emthin--last-geometry geo))
          (emthin--call* 'set-geometry
            :window_id window-id
            :x (emthin--rect-x geo)
            :y (emthin--rect-y geo)
            :w (emthin--rect-w geo)
            :h (emthin--rect-h geo))))
    (error
     (message "emthin: geometry error for window %s: %s" window-id err))))

;; ---------------------------------------------------------------------------
;; Per-frame sync (geometry + visibility + mirrors)
;; ---------------------------------------------------------------------------

(defun emthin--sync-frame (frame)
  "Sync visibility, geometry, and mirrors for embedded app buffers in FRAME."
  (when emthin--process
    (let ((ws-id (gethash frame emthin--frame-workspace-table)))
      (when (eql ws-id emthin--active-workspace-id)
        (let ((wid-wins (emthin--wid-wins-data frame))
              (next-view-id emthin--next-view-id))
          (unwind-protect
              (progn
                ;; 1. Window decorations
                (condition-case err
                    (maphash (lambda (_wid wins)
                               (dolist (win wins)
                                 (set-window-scroll-bars win 0 nil 0 nil)
                                 (set-window-fringes win 0 0)
                                 (set-window-margins win 0 0)))
                             wid-wins)
                  (error
                   (message "emthin: decoration error: %s" err)))
                ;; 2. Per-buffer sync (visibility, geometry, mirrors)
                (condition-case err
                    (dolist (buf (buffer-list))
                      (when-let* ((wid (buffer-local-value 'emthin--window-id buf)))
                        (let* ((wins (gethash wid wid-wins))
                               (now-visible (and wins t))
                               (was-visible (buffer-local-value 'emthin--visible buf))
                               (prev-state (gethash wid emthin--mirror-table))
                               (mirror-result (emthin--mirror-diff
                                               wins (car prev-state) (cdr prev-state)
                                               next-view-id)))
                          (setq next-view-id (cdr mirror-result))
                          ;; Visibility
                          (unless (eq now-visible was-visible)
                            (with-current-buffer buf
                              (setq emthin--visible now-visible))
                            (emthin--call* 'set-visibility
                                           :window_id wid
                                           :visible (if now-visible t :json-false)))
                          ;; Mirrors
                          (emthin--sync-mirrors wid (car mirror-result)))))
                  (error
                   (message "emthin: per-buffer sync error: %s" err)))
                )
            (emthin--sync-focus frame)
            (setq emthin--next-view-id next-view-id)))))))

;; ---------------------------------------------------------------------------
;; Mirror promotion (interactive)
;; ---------------------------------------------------------------------------

(defun emthin-promote-mirror ()
  "Promote the mirror in the selected window to become the app source.
The compositor will adopt the mirror's geometry as the app's new
position; the old source window becomes a mirror in its place."
  (interactive)
  (let* ((buf (window-buffer))
         (wid (buffer-local-value 'emthin--window-id buf))
         (state (gethash wid emthin--mirror-table)))
    (if (not (and wid state))
        (message "emthin: no embedded app in current window")
      (let ((mirrors (cdr state))
            (source-win (car state)))
        (if-let* ((vid (car (rassq (selected-window) mirrors))))
            (progn
              (emthin--call promote-mirror
                :window_id wid
                :view_id vid)
              (let ((new-view-id (emthin--alloc-view-id)))
                (puthash wid
                         (cons (selected-window)
                               (cons (cons new-view-id source-win)
                                     (cl-remove vid mirrors :key #'car)))
                         emthin--mirror-table))
              (message "emthin: promoted mirror %d" vid))
        (message "emthin: current window is not a mirror"))))))

(add-hook 'window-size-change-functions #'emthin--sync-frame)
(add-hook 'window-buffer-change-functions #'emthin--sync-frame)

;; ---------------------------------------------------------------------------
;; Focus sync
;; ---------------------------------------------------------------------------

(defun emthin--sync-focus (&optional _frame)
  "Sync focus if the focused window's app changed."
  (when emthin--process
    (condition-case err
        (let ((wid (buffer-local-value 'emthin--window-id
                                        (window-buffer (selected-window)))))
          (unless (eq wid emthin--last-focused-wid)
            (setq emthin--last-focused-wid wid)
            (if wid
                (emthin--call* 'set-focus :window_id wid)
              (emthin--call* 'set-focus))))
      (error
       (message "emthin: focus sync error: %s" err)))))

(add-hook 'window-selection-change-functions #'emthin--sync-focus)

(provide 'emthin-app)
;;; emthin-app.el ends here
