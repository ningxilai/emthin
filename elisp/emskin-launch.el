;;; emskin-launch.el --- XDG desktop app launcher for emskin  -*- lexical-binding: t; -*-

;;; Code:

(require 'xdg)
(require 'emskin-app)

(defvar emskin--app-list nil
  "Cached list of (NAME . EXEC) parsed from .desktop files.
Lazily populated by `emskin-open-app'.")

(defun emskin--desktop-parse (file)
  "Parse FILE into (NAME . EXEC) or nil."
  (when-let* ((entries (ignore-errors (xdg-desktop-read-file file)))
              (name (gethash "Name" entries))
              (exec (gethash "Exec" entries))
              ((equal (gethash "Type" entries) "Application"))
              ((not (equal (gethash "NoDisplay" entries) "true")))
              ((not (equal (gethash "Hidden" entries) "true")))
              ((not (equal (gethash "Terminal" entries) "true"))))
    (cons name exec)))

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
          (when-let* ((parsed (emskin--desktop-parse file)))
            (push parsed entries)))))
    entries))

(defun emskin-open-app (app)
  "Launch an application inside emskin.
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
    (setq emskin--pending-app-targets
          (nconc emskin--pending-app-targets (list target)))
    (condition-case err
        (let ((proc (apply #'start-process
                           (format "emskin-%s" (car args)) nil args)))
          (message "emskin: launched: %s (pid %d)" app (process-id proc)))
      (error
       (setq emskin--pending-app-targets old-targets)
       (signal (car err) (cdr err))))))

(provide 'emskin-launch)
;;; emskin-launch.el ends here
