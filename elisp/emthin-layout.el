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

(cl-defmethod emthin--compute-layout ((layout (eql nil)) window header-offset)
  "Fallback when no layout object is attached."
  (emthin--compute-layout (make-instance 'emthin-layout-fill) window header-offset))

(provide 'emthin-layout)
;;; emthin-layout.el ends here
