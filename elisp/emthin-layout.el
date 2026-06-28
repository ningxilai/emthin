;;; emthin-layout.el --- Layout policy generic functions  -*- lexical-binding: t; -*-

(require 'cl-lib)
(require 'emthin-geom)

;; ── Layout policy base class ──

(defclass emthin-layout ()
  ()
  :abstract t
  :documentation "Base class for layout policies.  Subclasses can
override `emthin--compute-layout'; the default returns window-body
geometry.")

(defclass emthin-layout-fill (emthin-layout)
  ()
  :documentation "Fill the Emacs window (default per-app layout).")

(defclass emthin-layout-float (emthin-layout)
   ((saved-rect :initarg :saved-rect :initform nil
                :type (or null emthin--rect)
                :documentation
                "Fixed fraction rect for this app."))
  :documentation "Fixed position within the parent frame.")

(defclass emthin-layout-tab (emthin-layout)
  ()
  :documentation "Tab layout: only the actively displayed app buffer
gets compositor visibility.  Sync behavior is driven by
`emthin--frame-layout', not per-app layout.")

(defclass emthin-layout-side-by-side (emthin-layout)
  ((source-ratio :initarg :source-ratio :initform 0.7 :type float
                 :documentation
                 "Fraction of width for the source window (0..1).
Side panel gets `(- 1 source-ratio)' width, divided among mirrors."))
  :documentation "Source window in main area, mirrors stacked on the right.")

;; ── Generic layout computation ──

(cl-defgeneric emthin--compute-layout (layout window header-offset)
  "Return an emthin--rect for WINDOW under LAYOUT policy.
HEADER-OFFSET is the pixel height of GTK bars (0 if unknown).")

(cl-defmethod emthin--compute-layout ((_layout emthin-layout) window header-offset)
  "Default: window-body geometry as fractions."
  (emthin--window-geometry window header-offset))

(cl-defmethod emthin--compute-layout ((layout emthin-layout-float) window header-offset)
  "Float: use saved-rect if available, otherwise fill."
  (or (oref layout saved-rect)
      (emthin--window-geometry window header-offset)))

(cl-defmethod emthin--compute-layout ((_layout (eql nil)) window header-offset)
  "Fallback when no layout object is attached."
  (emthin--compute-layout (make-instance 'emthin-layout-fill) window header-offset))

(cl-defmethod emthin--compute-layout ((layout emthin-layout-side-by-side) window header-offset)
  "Source window uses source-ratio of frame width; mirror geometry via sync-apps."
  (let ((base (emthin--window-geometry window header-offset)))
    (make-emthin--rect
     :x (emthin--rect-x base)
     :y (emthin--rect-y base)
     :w (* (emthin--rect-w base) (oref layout source-ratio))
     :h (emthin--rect-h base))))

;; ── Frame-level sync (data-driven, no global access) ──
;; Concrete `emthin--sync-apps' methods live in emthin-sync.el where
;; all required modules are available.

(cl-defgeneric emthin--sync-apps (layout wid-wins mirror-table)
  "Sync visibility/geometry for all apps under LAYOUT.
WID-WINS: hash wid→(win...). MIRROR-TABLE: hash wid→(src . ((vid . win)...)).
Layout methods must only use arguments; no global variable access.")

(cl-defmethod emthin--sync-apps ((_layout emthin-layout) _wid-wins _mirror-table)
  nil)

(cl-defgeneric emthin--layout-manages-mirror-geometry (layout)
  "Return t if LAYOUT handles mirror geometry itself (no-op caller).")

(cl-defmethod emthin--layout-manages-mirror-geometry ((_layout emthin-layout))
  nil)

(cl-defmethod emthin--layout-manages-mirror-geometry ((_layout emthin-layout-side-by-side))
  t)

(provide 'emthin-layout)
;;; emthin-layout.el ends here
