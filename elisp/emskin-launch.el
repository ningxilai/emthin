;;; emskin-launch.el --- XDG desktop app launcher for emskin  -*- lexical-binding: t; -*-

;;; Code:

(require 'xdg)
(require 'emskin-app)

(defvar emskin--app-list nil
  "Cached list of (NAME ICON EXEC COMMENT FILE) parsed from .desktop files.")

(defvar emskin--max-name-len 0
  "Max app name length, for padding completion annotations.")

;; ---------------------------------------------------------------------------
;; Low-level .desktop parser
;; ---------------------------------------------------------------------------

(defun emskin--section-bounds ()
  "Return (BEG . END) of the `[Desktop Entry]' section in current buffer.
Return nil if no `[Desktop Entry]' header is found."
  (let ((beg (and (re-search-forward (rx bol "[Desktop Entry]"
                                        (zero-or-more " ") eol)
                                     nil t)
                  (match-end 0))))
    (when beg
      (let ((end (if (re-search-forward (rx bol "[" (not "]")) nil t)
                     (match-beginning 0)
                   (point-max))))
        (cons beg end)))))

(defun emskin--desktop-get (key bounds)
  "Get string value of KEY within BOUNDS in current buffer."
  (let ((beg (car bounds))
        (end (cdr bounds)))
    (save-excursion
      (goto-char beg)
      (when (re-search-forward
             (rx bol (literal key) (zero-or-more " ") "="
                 (zero-or-more " ") (group (zero-or-more not-newline))
                 eol)
             end t)
        (match-string 1)))))

(defun emskin--tryexec-ok (tryexec)
  "Return t if TRYEXEC is executable."
  (if (file-name-absolute-p tryexec)
      (file-executable-p tryexec)
    (locate-file tryexec exec-path nil #'file-executable-p)))

(defun emskin--desktop-parse (file)
  "Parse FILE into (NAME ICON EXEC COMMENT FILE) or nil."
  (with-temp-buffer
    (insert-file-contents file)
    (let ((bounds (emskin--section-bounds))
          name icon command comment)
      (when bounds
        (let ((get (lambda (k) (emskin--desktop-get k bounds))))
          (unless (or (string= (funcall get "Hidden") "true")
                      (string= (funcall get "NoDisplay") "true"))
            ;; TryExec: skip if the binary is not found.
            (if-let* ((tryexec (funcall get "TryExec"))
                      ((not (emskin--tryexec-ok tryexec))))
                (setq bounds nil)
              ;; Only expand fields if the entry is valid.
              (setq name (funcall get "Name")
                    icon (funcall get "Icon")
                    command (funcall get "Exec")
                    comment (funcall get "Comment"))
              (when (or (not command) (string-empty-p command)
                        (string= (funcall get "Terminal") "true"))
                (setq bounds nil)))))
        (when (and name command)
          (list name icon command comment file))))))

;; ---------------------------------------------------------------------------
;; Exec field code expansion
;; ---------------------------------------------------------------------------

(defun emskin--format-exec (name icon command file)
  "Expand Exec field codes in COMMAND.
%% → %;  %u %U %f %F and deprecated codes → empty.
%i → --icon <ICON>;  %c → NAME;  %k → FILE."
  (string-trim
   (format-spec command
                `((?f . "")
                  (?F . "")
                  (?u . "")
                  (?U . "")
                  (?d . "")
                  (?D . "")
                  (?n . "")
                  (?N . "")
                  (?v . "")
                  (?m . "")
                  (?i . ,(if (and icon (not (string-empty-p icon)))
                             (format "--icon %s" icon)
                           ""))
                  (?c . ,name)
                  (?k . ,file)))))

;; ---------------------------------------------------------------------------
;; Scanning
;; ---------------------------------------------------------------------------

(defun emskin--desktop-scan ()
  "Scan all .desktop files from XDG data dirs, return spec list.
Each element is (NAME ICON EXEC COMMENT FILE).
Deduplicated by desktop file ID (first in XDG data-dir order wins)."
  (let* ((dirs (mapcar (lambda (d) (expand-file-name "applications" d))
                       (cons (xdg-data-home) (xdg-data-dirs))))
         (seen (make-hash-table :test 'equal))
         specs)
    (setq emskin--max-name-len 0)
    (dolist (dir dirs (nreverse specs))
      (when (file-directory-p dir)
        (dolist (file (directory-files-recursively dir "\\.desktop\\'"))
          (let ((id (string-remove-suffix
                     ".desktop"
                     (string-remove-prefix
                      (file-name-as-directory dir) file))))
            (unless (gethash id seen)
              (puthash id t seen)
              (when-let* ((parsed (emskin--desktop-parse file)))
                (let ((n (length (car parsed))))
                  (when (> n emskin--max-name-len)
                    (setq emskin--max-name-len n)))
                (push parsed specs)))))))))

;; ---------------------------------------------------------------------------
;; Completion annotation
;; ---------------------------------------------------------------------------

(defun emskin--annotate (cand)
  "Return annotation for candidate CAND."
  (let ((spec (assoc cand emskin--app-list)))
    (when-let* ((spec)
                (comment (nth 3 spec))
                ((not (string-empty-p comment))))
      (concat
       (make-string (max 1 (- emskin--max-name-len (length cand) 2)) ?\s)
       comment))))

;; ---------------------------------------------------------------------------
;; Interactive
;; ---------------------------------------------------------------------------

(defun emskin-open-app (app)
  "Launch an application inside emskin.
With prefix argument, refresh the .desktop file cache."
  (interactive
   (progn
     (when (or (null emskin--app-list) current-prefix-arg)
       (setq emskin--app-list (emskin--desktop-scan)))
     (let ((name (completing-read
                  "Launch: "
                  (lambda (string pred action)
                    (if (eq action 'metadata)
                        `(metadata
                          (annotation-function . emskin--annotate))
                      (complete-with-action action
                        (mapcar #'car emskin--app-list)
                        string pred)))
                  nil t)))
       (list (assoc name emskin--app-list)))))
  (unless app (error "Unknown application"))
  (let* ((name (nth 0 app))
         (icon (nth 1 app))
         (command (nth 2 app))
         (file (nth 4 app))
         (exec (emskin--format-exec name icon command file))
         (args (split-string-and-unquote exec))
         (target (selected-window))
         (old-targets emskin--pending-app-targets))
    (setq emskin--pending-app-targets
          (nconc emskin--pending-app-targets (list target)))
    (condition-case err
        (let ((proc (apply #'start-process
                           (format "emskin-%s" (car args)) nil args)))
          (message "emskin: launched: %s (pid %d)" name (process-id proc)))
      (error
       (setq emskin--pending-app-targets old-targets)
       (signal (car err) (cdr err))))))

(provide 'emskin-launch)
;;; emskin-launch.el ends here
