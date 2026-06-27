# Layout Generic Functions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add pluggable layout policies via `cl-defgeneric` on EIEIO layout objects, one per app.

**Architecture:** New `emthin-layout.el` defines base/subclasses + generic; `emthin--app` gains `layout` slot; `emthin-sync.el`'s `apply-geometry` dispatches via generic instead of calling `window-geometry` directly.

**Tech Stack:** Emacs Lisp, EIEIO, cl-generic

---

### Task 1: Create `emthin-layout.el`

**Files:**
- Create: `elisp/emthin-layout.el`

- [ ] **Step 1: Write the file**

```elisp
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
```

- [ ] **Step 2: Verify it loads**

Run: `emacs --batch --eval "(progn (push \"$(pwd)/elisp\" load-path) (require 'emthin-layout) (message \"OK\"))"`
Expected: prints "OK"

- [ ] **Step 3: Commit**

```bash
git add elisp/emthin-layout.el
git commit -m "feat(elisp): add emthin-layout.el with emthin--compute-layout generic"
```

---

### Task 2: Add `layout` slot to `emthin--app`

**Files:**
- Modify: `elisp/emthin-app.el`

- [ ] **Step 1: Insert `layout` slot after `saved-geometry`**

In the `defclass emthin--app` form, after the `saved-geometry` slot, add:

```elisp
   (layout :initform (make-instance 'emthin-layout-fill)
           :type emthin-layout
           :documentation "Layout policy object for this app.")
```

- [ ] **Step 2: Add require for emthin-layout**

At the top of the file, after `(require 'emthin-geom)`:

```elisp
(require 'emthin-layout)
```

- [ ] **Step 3: Verify it loads**

Run: `emacs --batch --eval "(progn (push \"$(pwd)/elisp\" load-path) (require 'emthin-app) (message \"OK\"))"`
Expected: prints "OK"

- [ ] **Step 4: Commit**

```bash
git add elisp/emthin-app.el
git commit -m "feat(elisp): add layout slot to emthin--app"
```

---

### Task 3: Update `emthin-sync.el` to use generic dispatch

**Files:**
- Modify: `elisp/emthin-sync.el`

- [ ] **Step 1: Change `emthin--apply-geometry` to take an app object**

Replace the old function:

```elisp
(defun emthin--apply-geometry (app window)
  "Send set_geometry for APP if its geometry changed.
Computes geometry via `emthin--compute-layout' on the app's layout object."
  (condition-case err
      (let* ((geo (emthin--compute-layout (oref app layout) window app))
             (old (oref app last-geometry)))
        (unless (equal geo old)
          (oset app last-geometry geo)
          (emthin--send 'set-geometry
            `(:window_id ,(oref app window-id)
              :x ,(emthin--rect-x geo)
              :y ,(emthin--rect-y geo)
              :w ,(emthin--rect-w geo)
              :h ,(emthin--rect-h geo)))))
    (error
     (message "emthin: geometry error for window %s: %s"
              (oref app window-id) err))))
```

- [ ] **Step 2: Update callers in `emthin--sync-frame`**

In the `maphash` lambda inside `emthin--sync-frame`, replace:

```elisp
;; OLD:
(emthin--apply-geometry wid (car wins))
```

with:

```elisp
;; NEW:
(emthin--apply-geometry app (car wins))
```

The call to `emthin--apply-geometry` should appear once. Make sure the variable `app` is in scope (it should be — it's already bound in the `let*` from Step 2's change).

- [ ] **Step 3: Verify it loads**

Run: `emacs --batch --eval "(progn (push \"$(pwd)/elisp\" load-path) (require 'emthin-sync) (message \"OK\"))"`
Expected: prints "OK"

- [ ] **Step 4: Commit**

```bash
git add elisp/emthin-sync.el
git commit -m "feat(elisp): use emthin--compute-layout generic in apply-geometry"
```

---

### Task 4: Update `emthin-manage.el` caller

**Files:**
- Modify: `elisp/emthin-manage.el`

- [ ] **Step 1: Update `emthin-manage-resync` to use new signature**

Change the `(emthin--apply-geometry wid win)` call to `(emthin--apply-geometry app win)`.

The resync function needs to find the app object. Replace the relevant section:

```elisp
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
```

- [ ] **Step 2: Verify it loads**

Run: `emacs --batch --eval "(progn (push \"$(pwd)/elisp\" load-path) (require 'emthin-manage) (message \"OK\"))"`
Expected: prints "OK"

- [ ] **Step 3: Commit**

```bash
git add elisp/emthin-manage.el
git commit -m "feat(elisp): update emthin-manage-resync for new apply-geometry signature"
```

---

### Task 5: Add `require` to `emthin.el`

**Files:**
- Modify: `elisp/emthin.el`

- [ ] **Step 1: Insert `(require 'emthin-layout)` after geometry**

In the require chain, between `emthin-geom` and `emthin-dispatch`, add:

```elisp
(require 'emthin-layout)
```

- [ ] **Step 2: Verify all modules load**

Run: `emacs --batch --eval "(progn (push \"$(pwd)/elisp\" load-path) (require 'emthin-manage) (message \"ALL OK\"))"`
Expected: prints "ALL OK"

- [ ] **Step 3: Commit**

```bash
git add elisp/emthin.el
git commit -m "feat(elisp): add emthin-layout require to emthin.el"
```

---

### Task 6: Full load verification

**Files:**
- All

- [ ] **Step 1: Full chain load test**

```bash
emacs --batch \
  --eval "(push \"$(pwd)/elisp\" load-path)" \
  --eval "(require 'emthin)" \
  --eval "(message \"FULL CHAIN OK\")"
```
Expected: prints "FULL CHAIN OK"

- [ ] **Step 2: Verify generic dispatch works**

```bash
emacs --batch \
  --eval "(push \"$(pwd)/elisp\" load-path)" \
  --eval "(require 'emthin-layout)" \
  --eval "(let ((fill (make-instance 'emthin-layout-fill))) (message \"fill class: %s\" (eieio-object-class fill)))" \
  --eval "(let ((fl (make-instance 'emthin-layout-float :saved-rect (make-emthin--rect :x 0.1 :y 0.2 :w 0.3 :h 0.4)))) (message \"float saved: %s\" (oref fl saved-rect)))"
```
Expected: prints class name + rect

- [ ] **Step 3: Commit**

```bash
git commit --allow-empty -m "chore: verify layout generic chain loads correctly"
```
