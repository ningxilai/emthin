;;; emthin-layout.el --- Layout policy generic functions  -*- lexical-binding: t; -*-

(require 'cl-lib)
(require 'emthin-geom)

;; ── Layout policy base class ──

(defclass emthin-layout ()
  ()
  :abstract t
  :documentation "Base class for layout policies.  Each subclass
implements `emthin--compute-layout' to return an emthin--rect.")

(defclass emthin-layout-fill (emthin-layout)
  ()
  :documentation "Fill the entire parent Emacs window (default).")

(defclass emthin-layout-float (emthin-layout)
   ((saved-rect :initarg :saved-rect :initform nil
                :type (or null emthin--rect)
                :documentation
                "Fixed fraction rect for this app."))
  :documentation "Fixed position within the parent frame.")

;; ── Generic layout computation ──

(cl-defgeneric emthin--compute-layout (layout window app)
  "Return an emthin--rect for APP in WINDOW under LAYOUT policy.")

(cl-defmethod emthin--compute-layout ((layout emthin-layout-fill) window _app)
  "Fill the Emacs window: collect body-pixel-edges as fractions."
  (emthin--window-geometry window (emthin--frame-header-offset)))

(cl-defmethod emthin--compute-layout ((layout emthin-layout-float) window _app)
  "Use saved-rect if available, otherwise fall back to fill."
  (or (oref layout saved-rect)
      (emthin--window-geometry window (emthin--frame-header-offset))))

(cl-defmethod emthin--compute-layout ((layout (eql nil)) window app)
  "Fallback when no layout object is attached."
  (emthin--compute-layout (make-instance 'emthin-layout-fill) window app))

(provide 'emthin-layout)
;;; emthin-layout.el ends here
