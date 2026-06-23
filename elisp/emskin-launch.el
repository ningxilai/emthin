;;; emskin-launch.el --- XDG desktop app launcher for emskin  -*- lexical-binding: t; -*-

;;; Code:

(require 'cl-lib)
(require 'xdg)

;; ---------------------------------------------------------------------------
;; App window target queue
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
;; .desktop file caching
;; ---------------------------------------------------------------------------

(defvar emskin--app-list nil
  "Cached list of (NAME . EXEC) parsed from .desktop files.
Lazily populated by `emskin-open-app'.")

(defvar emskin--wayland-cache (make-hash-table :test 'equal)
  "Cache of (BINARY-PATH . WAYLAND-P) for `emskin--exec-wayland-p'.")

(defun emskin--exec-wayland-p (exec)
  "Return non-nil if the binary in EXEC links to libwayland-client."
  (let* ((binary (car (split-string exec)))
         (path (executable-find binary)))
    (when path
      (or (gethash path emskin--wayland-cache)
          (let ((result (with-temp-buffer
                          (and (zerop (call-process "ldd" nil t nil path))
                               (progn
                                 (goto-char (point-min))
                                 (re-search-forward
                                  "libwayland-client" nil t))))))
            (puthash path result emskin--wayland-cache)
            result)))))

(defun emskin--desktop-parse (file)
  "Parse FILE (a .desktop file) into (NAME . EXEC) or nil.
Only reads the [Desktop Entry] section; skips Terminal, NoDisplay, Hidden."
  (with-temp-buffer
    (insert-file-contents file)
    (let (name exec begin end)
      (save-excursion
        (goto-char (point-min))
        (setq begin (and (re-search-forward (rx bol "[Desktop Entry]"
                                                 (zero-or-more " ") eol)
                                            nil t)
                         (point)))
        (when begin
          (setq end (if (re-search-forward (rx bol "[") nil t)
                        (match-beginning 0)
                      (point-max)))
          (save-restriction
            (narrow-to-region begin end)
            (goto-char (point-min))
            (setq name (and (re-search-forward "^Name=\\(.*\\)" nil t)
                            (match-string 1)))
            (goto-char (point-min))
            (setq exec (and (re-search-forward "^Exec=\\(.*\\)" nil t)
                            (match-string 1)))
            (when (and name exec
                       (not (re-search-forward "^Terminal=true" nil t))
                       (not (re-search-forward "^NoDisplay=true" nil t))
                       (not (re-search-forward "^Hidden=true" nil t)))
              (cons name exec))))))))

(defun emskin--desktop-scan ()
  "Scan all .desktop files from XDG data dirs, return ((NAME . EXEC) ...)."
  (let* ((data-home (or (getenv "XDG_DATA_HOME")
                        (expand-file-name ".local/share" (getenv "HOME"))))
         (data-dirs (or (getenv "XDG_DATA_DIRS")
                        "/usr/local/share:/usr/share"))
         (dirs (mapcar (lambda (d) (expand-file-name "applications" d))
                       (cons data-home (split-string data-dirs ":"))))
         entries)
    (dolist (dir dirs)
      (when (file-directory-p dir)
        (dolist (file (directory-files dir t "\\.desktop\\'"))
          (when-let ((parsed (emskin--desktop-parse file))
                     ((emskin--exec-wayland-p (cdr parsed))))
            (push parsed entries)))))
    entries))

;; ---------------------------------------------------------------------------
;; Interactive entry point
;; ---------------------------------------------------------------------------

(defun emskin-open-app (app)
  "Launch a Wayland application inside emskin.
With prefix argument, refresh the .desktop file cache.
Select from all installed XDG desktop applications via completion."
  (interactive
   (progn
     (when (or (null emskin--app-list) current-prefix-arg)
       (setq emskin--app-list (emskin--desktop-scan)))
     (let ((name (completing-read "Launch: " emskin--app-list nil t)))
       (list (cdr (assoc name emskin--app-list))))))
  (let* ((args (split-string-and-unquote app))
         (target (selected-window))
         (old-targets emskin--pending-app-targets))
    (setq emskin--pending-app-targets (nconc emskin--pending-app-targets (list target)))
    (condition-case err
        (progn
          (apply #'start-process (format "emskin-%s" (car args)) nil args)
          (message "emskin: launched: %s" app))
      (error
       (setq emskin--pending-app-targets old-targets)
       (signal (car err) (cdr err))))))

(provide 'emskin-launch)
;;; emskin-launch.el ends here
