;;; emthin-manage.el --- Embedded app management mode  -*- lexical-binding: t; -*-

(require 'cl-lib)
(require 'emthin-ipc)
(require 'emthin-sync)

;; ---------------------------------------------------------------------------
;; Management transient keymap
;; ---------------------------------------------------------------------------

(defvar emthin-manage-mode-map
  (let ((map (make-sparse-keymap)))
    (define-key map "q"      #'emthin-manage-exit)
    (define-key map "C-g"    #'emthin-manage-exit)
    (define-key map "n"      #'emthin-manage-next-app)
    (define-key map "p"      #'emthin-manage-prev-app)
    (define-key map "h"      #'emthin-manage-spatial-left)
    (define-key map "j"      #'emthin-manage-spatial-down)
    (define-key map "k"      #'emthin-manage-spatial-up)
    (define-key map "l"      #'emthin-manage-spatial-right)
    (define-key map "x"      #'emthin-manage-close-app)
    (define-key map "f"      #'emthin-manage-toggle-fullscreen)
    (define-key map "r"      #'emthin-manage-resync)
    (define-key map "1"      #'emthin-manage-workspace-1)
    (define-key map "2"      #'emthin-manage-workspace-2)
    (define-key map "3"      #'emthin-manage-workspace-3)
    (define-key map "4"      #'emthin-manage-workspace-4)
    (define-key map "5"      #'emthin-manage-workspace-5)
    (define-key map "6"      #'emthin-manage-workspace-6)
    (define-key map "7"      #'emthin-manage-workspace-7)
    (define-key map "8"      #'emthin-manage-workspace-8)
    (define-key map "9"      #'emthin-manage-workspace-9)
    map)
  "Keymap active during `emthin-manage' transient.")

;; ---------------------------------------------------------------------------
;; Entry / Exit
;; ---------------------------------------------------------------------------

(defun emthin-manage ()
  "Enter embedded app management mode.

While active, single keys control embedded apps:
  n/p     next/previous app window
  h/j/k/l navigate by spatial position
  x       close (kill) the focused embedded app
  f       toggle fullscreen
  r       re-sync geometry
  1-9     switch to workspace N
  q/C-g   exit management mode

The transient stays active until explicitly exited."
  (interactive)
  (when emthin--process
    (message "Manage: n/p=next/prev  h/j/k/l=move  x=close  f=fullscreen  1-9=ws  q=quit")
    (set-transient-map emthin-manage-mode-map t nil
                       "Manage: ")))

(defun emthin-manage-exit ()
  "Exit embedded app management mode."
  (interactive)
  (message "Manage: done"))

;; ---------------------------------------------------------------------------
;; Helpers
;; ---------------------------------------------------------------------------

(defun emthin--manage-embedded-windows ()
  "Return list of live Emacs windows displaying embedded app buffers."
  (cl-remove-if-not
   (lambda (w)
     (buffer-local-value 'emthin--window-id (window-buffer w)))
   (window-list nil 'no-minibuf)))

(defun emthin--manage-embedded-count ()
  "Number of embedded app windows currently visible."
  (length (emthin--manage-embedded-windows)))

(defun emthin--manage-window-center (window)
  "Return (CX . CY) of the center of WINDOW body area."
  (let ((edges (window-body-pixel-edges window)))
    (cons (+ (nth 0 edges) (/ (- (nth 2 edges) (nth 0 edges)) 2.0))
          (+ (nth 1 edges) (/ (- (nth 3 edges) (nth 1 edges)) 2.0)))))

;; ---------------------------------------------------------------------------
;; App cycling
;; ---------------------------------------------------------------------------

(defun emthin--manage-cycle (direction)
  "Select the next (DIRECTION=1) or previous (DIRECTION=-1) embedded app window."
  (let* ((windows (emthin--manage-embedded-windows))
         (cur (selected-window))
         (pos (cl-position cur windows)))
    (if (and windows pos)
        (select-window (nth (mod (+ pos direction) (length windows)) windows))
      (user-error "No other embedded app window"))))

(defun emthin-manage-next-app ()
  "Focus the next embedded app window."
  (interactive)
  (emthin--manage-cycle 1))

(defun emthin-manage-prev-app ()
  "Focus the previous embedded app window."
  (interactive)
  (emthin--manage-cycle -1))

;; ---------------------------------------------------------------------------
;; Spatial navigation
;; ---------------------------------------------------------------------------

(defun emthin--manage-find-spatial (direction)
  "Find the nearest embedded app window in DIRECTION (left/right/up/down).
Uses center-to-center distance with an angular cone to prefer
on-axis neighbors."
  (let* ((windows (emthin--manage-embedded-windows))
         (cur (selected-window))
         (cur-center (emthin--manage-window-center cur))
         (cx (car cur-center))
         (cy (cdr cur-center))
         best best-dist)
    (dolist (win windows)
      (unless (eq win cur)
        (let* ((center (emthin--manage-window-center win))
               (dx (- (car center) cx))
               (dy (- (cdr center) cy))
               (dist (+ (* dx dx) (* dy dy)))
               (ok (pcase direction
                     ('left   (and (< dx 0) (< (abs dy) (abs dx))))
                     ('right  (and (> dx 0) (< (abs dy) (abs dx))))
                     ('up     (and (< dy 0) (< (abs dx) (abs dy))))
                     ('down   (and (> dy 0) (< (abs dx) (abs dy)))))))
          (when (and ok (or (not best-dist) (< dist best-dist)))
            (setq best win best-dist dist)))))
    best))

(defmacro emthin--define-spatial-command (direction)
  "Define a management command to move focus in DIRECTION."
  (let ((doc (format "Move focus to the embedded app %s of the current window."
                     direction)))
    `(defun ,(intern (format "emthin-manage-spatial-%s" direction)) ()
       ,doc
       (interactive)
       (if-let* ((target (emthin--manage-find-spatial ',direction)))
           (select-window target)
         (user-error "No embedded app %s from here" ',direction)))))

(emthin--define-spatial-command left)
(emthin--define-spatial-command right)
(emthin--define-spatial-command up)
(emthin--define-spatial-command down)

;; ---------------------------------------------------------------------------
;; Close app
;; ---------------------------------------------------------------------------

(defun emthin-manage-close-app ()
  "Close the focused embedded app."
  (interactive)
  (when-let* ((buf (window-buffer (selected-window)))
              (wid (buffer-local-value 'emthin--window-id buf)))
    (with-current-buffer buf
      (emthin--send close :window_id wid))))

;; ---------------------------------------------------------------------------
;; Fullscreen toggle
;; ---------------------------------------------------------------------------

(defvar-local emthin--manage-saved-geo nil
  "Geometry saved before fullscreen, stored buffer-locally.")

(defun emthin-manage-toggle-fullscreen ()
  "Toggle the focused embedded app between full-frame and its previous geometry."
  (interactive)
  (if-let* ((buf (window-buffer (selected-window)))
            (wid (buffer-local-value 'emthin--window-id buf)))
      (with-current-buffer buf
        (let* ((current (buffer-local-value 'emthin--last-geometry buf))
               (full (make-emthin--rect)))
          (if (and emthin--manage-saved-geo (equal current full))
              (let ((geo emthin--manage-saved-geo))
                (setq emthin--manage-saved-geo nil)
                (emthin--send 'set-geometry
                  :window_id wid
                  :x (emthin--rect-x geo)
                  :y (emthin--rect-y geo)
                  :w (emthin--rect-w geo)
                  :h (emthin--rect-h geo))
                (message "Manage: restored"))
            (when current
              (setq emthin--manage-saved-geo current))
            (emthin--send 'set-geometry
              :window_id wid :x 0.0 :y 0.0 :w 1.0 :h 1.0)
            (message "Manage: fullscreen"))))
    (user-error "No embedded app focused")))

;; ---------------------------------------------------------------------------
;; Geometry re-sync
;; ---------------------------------------------------------------------------

(defun emthin-manage-resync ()
  "Re-sync geometry for all embedded apps in the current frame."
  (interactive)
  (when emthin--process
    (let ((frame (selected-frame))
          (count 0))
      (dolist (win (window-list frame 'no-minibuf))
        (when-let* ((buf (window-buffer win))
                    (wid (buffer-local-value 'emthin--window-id buf))
                    (app (emthin--find-app wid)))
          (with-current-buffer buf
            (setq emthin--last-geometry nil))
          (emthin--apply-geometry app win)
          (cl-incf count)))
      (message "Manage: re-synced %d app(s)" count))))

;; ---------------------------------------------------------------------------
;; Workspace switching
;; ---------------------------------------------------------------------------

(defun emthin-manage-workspace (n)
  "Switch to workspace N."
  (emthin--send 'switch-workspace :workspace_id n))

(defmacro emthin--define-workspace-command (n)
  "Define a management command to switch to workspace N."
  (let ((name (intern (format "emthin-manage-workspace-%d" n))))
    `(defun ,name ()
       ,(format "Switch to workspace %d." n)
       (interactive)
       (emthin-manage-workspace ,n))))

(emthin--define-workspace-command 1)
(emthin--define-workspace-command 2)
(emthin--define-workspace-command 3)
(emthin--define-workspace-command 4)
(emthin--define-workspace-command 5)
(emthin--define-workspace-command 6)
(emthin--define-workspace-command 7)
(emthin--define-workspace-command 8)
(emthin--define-workspace-command 9)

;; ---------------------------------------------------------------------------
;; Registration
;; ---------------------------------------------------------------------------

(provide 'emthin-manage)
;;; emthin-manage.el ends here
