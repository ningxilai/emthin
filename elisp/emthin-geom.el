;;; emthin-geom.el --- Pure geometry primitives for emthin  -*- lexical-binding: t; -*-

(require 'cl-lib)

(cl-defstruct emthin--rect
  "Float rectangle relative to Emacs frame (0..1)."
  (x 0.0 :type float)
  (y 0.0 :type float)
  (w 1.0 :type float)
  (h 1.0 :type float))

(defun emthin--px->fl (px dim)
  "Convert pixel PX to a fraction of DIM."
  (/ (float px) (float dim)))

(defun emthin--fl->px (fl dim)
  "Convert float FL to integer pixel within DIM."
  (round (* fl (float dim))))

(defun emthin--window-geometry (window &optional header-offset)
  "Return emthin--rect for WINDOW body area (fractions 0..1).
HEADER-OFFSET (pixels) is added to y to compensate for GTK external bars."
  (let* ((edges (window-body-pixel-edges window))
         (offset (or header-offset 0))
         (x (nth 0 edges))
         (y (+ (nth 1 edges) offset))
         (w (- (nth 2 edges) (nth 0 edges)))
         (h (- (nth 3 edges) (nth 1 edges)))
         (fw (float (frame-pixel-width (window-frame window))))
         (fh (float (frame-pixel-height (window-frame window)))))
    (make-emthin--rect
     :x (emthin--px->fl x fw)
     :y (emthin--px->fl y fh)
     :w (emthin--px->fl w fw)
     :h (emthin--px->fl h fh))))

(defun emthin--rect-scale (rect fw fh)
  "Convert RECT to pixel coordinates (list x y w h)."
  (list (emthin--fl->px (emthin--rect-x rect) fw)
        (emthin--fl->px (emthin--rect-y rect) fh)
        (emthin--fl->px (emthin--rect-w rect) fw)
        (emthin--fl->px (emthin--rect-h rect) fh)))

(defun emthin--rect-center (rect)
  "Return (CX . CY) center of RECT."
  (cons (+ (emthin--rect-x rect) (/ (emthin--rect-w rect) 2.0))
        (+ (emthin--rect-y rect) (/ (emthin--rect-h rect) 2.0))))

(provide 'emthin-geom)
;;; emthin-geom.el ends here
