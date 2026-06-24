;;; emskin-app.el --- Embedded app lifecycle, geometry, and mirrors  -*- lexical-binding: t; -*-

(require 'cl-lib)
(require 'subr-x)
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

(defvar emskin--ws-to-frame-table (make-hash-table :test 'eql)
  "Reverse mapping: workspace-id to frame.
Complements `emskin--frame-workspace-table'.")

(defvar emskin--active-workspace-id nil
  "Currently active workspace ID in the compositor.")

(defun emskin--map-frame-to-workspace (frame workspace-id)
  "Map FRAME to WORKSPACE-ID in both forward and reverse tables."
  (puthash frame workspace-id emskin--frame-workspace-table)
  (puthash workspace-id frame emskin--ws-to-frame-table))

(defun emskin--unmap-frame (frame)
  "Remove FRAME from workspace tables.  Idempotent."
  (let ((ws-id (gethash frame emskin--frame-workspace-table)))
    (remhash frame emskin--frame-workspace-table)
    (when ws-id
      (remhash ws-id emskin--ws-to-frame-table))))

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
;; Data accessors
;; ---------------------------------------------------------------------------

(defsubst emskin--rect (x y w h)
  "Create a rect from X Y W H."
  (list x y w h))

(defsubst emskin--rect-x (r) (nth 0 r))
(defsubst emskin--rect-y (r) (nth 1 r))
(defsubst emskin--rect-w (r) (nth 2 r))
(defsubst emskin--rect-h (r) (nth 3 r))

(defsubst emskin--mirror-source (table wid)
  "Return source window for WID in mirror TABLE."
  (car (gethash wid table)))

(defsubst emskin--mirror-mirrors (table wid)
  "Return mirror alist for WID in mirror TABLE."
  (cdr (gethash wid table)))

;; ---------------------------------------------------------------------------
;; IPC call helpers
;; ---------------------------------------------------------------------------

(defmacro emskin--call (method &rest plist)
  "Send a METHOD notification with alternating keyword-value PLIST.
METHOD is a kebab-case or snake_case symbol.  Example:

    (emskin--call set-focus :window_id 42)

expands to

    (emskin--send \\='set-focus (list :window_id 42))"
  (declare (indent 1))
  `(emskin--send ',method (list ,@plist)))

(defun emskin--call* (method &rest plist)
  "Runtime version of `emskin--call' macro.
Send a METHOD notification with alternating keyword-value PLIST."
  (emskin--send method plist))

;; ---------------------------------------------------------------------------
;; Message dispatch
;; ---------------------------------------------------------------------------

(defun emskin--dispatch (method params)
  "Dispatch a parsed METHOD with PARAMS plist from emskin."
  (pcase method
    ('connected
     (message "emskin: connected (version %s)"
              (or (plist-get params :version) "?"))
     (setq emskin--active-workspace-id 1)
     (emskin--map-frame-to-workspace (selected-frame) 1)
     (run-hooks 'emskin-connected-hook))
    ('error
     (message "emskin error: %s" (plist-get params :msg)))
    ('window_created
     (emskin--on-window-created (plist-get params :window_id)
                                 (or (plist-get params :title) "")))
    ('window_destroyed
     (emskin--on-window-destroyed (plist-get params :window_id)))
    ('title_changed
     (emskin--on-title-changed (plist-get params :window_id)
                                (or (plist-get params :title) "")))
    ('focus_view
     (emskin--on-focus-view (plist-get params :window_id)
                             (plist-get params :view_id)))
    ('surface_size
     (let* ((w (plist-get params :width))
            (h (plist-get params :height))
            (frame-h (frame-pixel-height))
            (offset (or emskin--header-offset
                        (max 0 (- h frame-h)))))
       (setq emskin--header-offset offset)
       (message "emskin: surface=%sx%s bars=%dpx" w h offset)
       (dolist (frame (frame-list))
         (emskin--sync-frame frame))))
    ('x_wayland_ready
     nil)
    (_
     (message "emskin: unknown message type %s" method))))

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
      (if next-wid
          (emskin--call set-focus :window_id next-wid)
        (emskin--call set-focus)))
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
    (emskin--call close :window_id emskin--window-id)))

(defun emskin--post-command-prefix-done ()
  "After a command completes in an embedded app buffer, ask the
compositor to restore keyboard focus to the embedded app (clearing
prefix state along the way). Registered buffer-locally — only fires
when the post-command tick runs while the app buffer is still current."
  (when emskin--process
    (emskin--call prefix-done)))

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
    (emskin--call prefix-clear)))

(add-hook 'post-command-hook #'emskin--post-command-prefix-clear)

;; ---------------------------------------------------------------------------
;; Geometry reporting
;; ---------------------------------------------------------------------------

(defun emskin--frame-header-offset (&optional _frame)
  "Pixel height of external GTK bars (menu-bar + tool-bar).
Computed once when the compositor reports the surface size."
  (or emskin--header-offset 0))

(defun emskin--edges->rect (offset edges)
  "Convert pixel EDGES (X1 Y1 X2 Y2) to rect with header OFFSET."
  (emskin--rect (nth 0 edges)
                (+ (nth 1 edges) offset)
                (- (nth 2 edges) (nth 0 edges))
                (- (nth 3 edges) (nth 1 edges))))

(defun emskin--window-geometry (window)
  "Return rect for Emacs WINDOW body area in surface-local pixels.
Pipeline: window-body-pixel-edges → edges->rect."
  (thread-last
    (window-body-pixel-edges window)
    (emskin--edges->rect (emskin--frame-header-offset (window-frame window)))))

(defun emskin--alloc-view-id ()
  "Allocate a unique mirror view ID."
  (cl-incf emskin--next-view-id))

;; ---------------------------------------------------------------------------
;; Per-frame sync helpers — pure collection + diff, then apply
;; ---------------------------------------------------------------------------

(defun emskin--wid-wins-data (frame)
  "Return hash-table wid→(win...) for FRAME (pure collection)."
  (let ((wid-wins (make-hash-table :test 'eql)))
    (dolist (win (window-list frame 'no-minibuf))
      (when-let* ((wid (buffer-local-value 'emskin--window-id
                                          (window-buffer win))))
        (puthash wid (cons win (gethash wid wid-wins)) wid-wins)))
    wid-wins))

(defun emskin--mirror-diff (wins prev-source prev-mirrors next-view-id)
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
                  :new-mirrors (list source-win (nreverse new-mirrors)))
            next-view-id))))

(defun emskin--send-mirror-geometry* (wid view-id win msg-type)
  "Send mirror geometry IPC (utility, callable from thunks)."
  (let ((geo (emskin--window-geometry win))
        (method (intern msg-type)))
    (emskin--call* method
      :window_id wid :view_id view-id
      :x (emskin--rect-x geo) :y (emskin--rect-y geo)
      :w (emskin--rect-w geo) :h (emskin--rect-h geo))))

(defun emskin--sync-mirrors (wid diff)
  "Apply mirror DIFF plist for WID synchronously."
  (let ((source-win (plist-get diff :source-win))
        (promote-vid (plist-get diff :promote-vid))
        (removals (plist-get diff :mirror-removals))
        (additions (plist-get diff :mirror-additions))
        (updates (plist-get diff :mirror-updates))
        (new-mirrors (plist-get diff :new-mirrors)))
    (dolist (vid removals)
      (emskin--call* 'remove-mirror :window_id wid :view_id vid))
    (when promote-vid
      (emskin--call* 'promote-mirror :window_id wid :view_id promote-vid))
    (dolist (pair additions)
      (emskin--send-mirror-geometry* wid (car pair) (cdr pair) "add_mirror"))
    (dolist (pair updates)
      (emskin--send-mirror-geometry* wid (car pair) (cdr pair) "update_mirror_geometry"))
    (when source-win
      (emskin--report-geometry wid source-win))
    (if source-win
        (puthash wid new-mirrors emskin--mirror-table)
      (remhash wid emskin--mirror-table))))

(defun emskin--report-geometry (window-id window)
  "Send set_geometry for WINDOW-ID if geometry changed."
  (condition-case err
      (let* ((geo (emskin--window-geometry window))
             (buf (window-buffer window))
             (old-geo (buffer-local-value 'emskin--last-geometry buf)))
        (message "emskin: window %s geo=%s" window-id geo)
        (unless (equal geo old-geo)
          (with-current-buffer buf
            (setq emskin--last-geometry geo))
          (emskin--call* 'set-geometry
            :window_id window-id
            :x (emskin--rect-x geo)
            :y (emskin--rect-y geo)
            :w (emskin--rect-w geo)
            :h (emskin--rect-h geo))))
    (error
     (message "emskin: geometry error for window %s: %s" window-id err))))

;; ---------------------------------------------------------------------------
;; Per-frame sync (geometry + visibility + mirrors)
;; ---------------------------------------------------------------------------

(defun emskin--sync-frame (frame)
  "Sync visibility, geometry, and mirrors for embedded app buffers in FRAME."
  (when emskin--process
    (let ((ws-id (gethash frame emskin--frame-workspace-table)))
      (when (eql ws-id emskin--active-workspace-id)
        (let ((wid-wins (emskin--wid-wins-data frame))
              (next-view-id emskin--next-view-id))
          ;; 1. Window decorations
          (ignore-errors
            (maphash (lambda (_wid wins)
                       (dolist (win wins)
                         (set-window-scroll-bars win 0 nil 0 nil)
                         (set-window-fringes win 0 0)
                         (set-window-margins win 0 0)))
                     wid-wins))
          ;; 2. Per-buffer sync (visibility, geometry, mirrors)
          (ignore-errors
            (dolist (buf (buffer-list))
              (when-let* ((wid (buffer-local-value 'emskin--window-id buf)))
                (let* ((wins (gethash wid wid-wins))
                       (now-visible (and wins t))
                       (was-visible (buffer-local-value 'emskin--visible buf))
                       (prev-state (gethash wid emskin--mirror-table))
                       (mirror-result (emskin--mirror-diff
                                       wins (car prev-state) (cdr prev-state)
                                       next-view-id)))
                  (setq next-view-id (cdr mirror-result))
                  ;; Visibility
                  (unless (eq now-visible was-visible)
                    (with-current-buffer buf
                      (setq emskin--visible now-visible))
                    (emskin--call* 'set-visibility
                                   :window_id wid
                                   :visible (if now-visible t :json-false)))
                  ;; Mirrors
                  (emskin--sync-mirrors wid (car mirror-result))))))
          ;; 3. Save updated view-id counter
          (setq emskin--next-view-id next-view-id))))))

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
              (emskin--call promote-mirror
                :window_id wid
                :view_id vid)
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
  "Sync focus if the focused window's app changed."
  (when emskin--process
    (let ((wid (buffer-local-value 'emskin--window-id
                                    (window-buffer (selected-window)))))
      (unless (eq wid emskin--last-focused-wid)
        (setq emskin--last-focused-wid wid)
        (if wid
            (emskin--call* 'set-focus :window_id wid)
          (emskin--call* 'set-focus))))))

(add-hook 'window-selection-change-functions #'emskin--sync-focus)

(provide 'emskin-app)
;;; emskin-app.el ends here
