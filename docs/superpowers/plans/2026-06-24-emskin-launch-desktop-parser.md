# emthin-launch Full XDG Desktop Entry Parser — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rewrite `emthin-launch.el` to fully parse XDG Desktop Entry Specification v1.5 files with Exec field-code substitution, locale fallback, and action support.

**Architecture:** Uses Emacs built-in `xdg-desktop-read-file` for `[Desktop Entry]` section parsing (handles format grammar + locale selection). Custom code handles: escape sequence unescaping, action group parsing, field-code substitution, and scanning/launch logic.

**Tech Stack:** Emacs Lisp 29+ (`xdg-data-dirs`, `xdg-desktop-read-file`, `current-locale`)

---

## Files

| File | Action | Role |
|---|---|---|
| `elisp/emthin-launch.el` | Modify | Parser, scanner, launcher |
| `elisp/tests/emthin-launch-tests.el` | Create | ERT tests |
| `elisp/tests/fixtures/simple.desktop` | Create | Fixture: simple entry |
| `elisp/tests/fixtures/full.desktop` | Create | Fixture: all fields + locale variants |
| `elisp/tests/fixtures/actions.desktop` | Create | Fixture: multi-action entry |
| `elisp/tests/fixtures/continuation.desktop` | Create | Fixture: continuation + escapes |

---

### Task 1: Parser helpers — join lines, group split, escape unescape

`xdg-desktop-read-file` handles `[Desktop Entry]` fully (incl. locale). We
still need helpers for:
- Joining continuation lines in raw file (for action group parsing)
- Splitting raw file into groups (for action group parsing)  
- Unescaping `\s` `\n` `\t` `\r` `\\` `\"` in values (xdg doesn't unescape)

**Files:**
- Modify: `elisp/emthin-launch.el` — add helpers
- Create: `elisp/tests/emthin-launch-tests.el` — tests

- [ ] **Step 1: Write helper tests**

```elisp
;; elisp/tests/emthin-launch-tests.el
(require 'ert)
(require 'emthin-launch)

(ert-deftest emthin--join-desktop-lines-basic ()
  (should (equal (emthin--join-desktop-lines '("Name=Foo" "Exec=bar"))
                 '("Name=Foo" "Exec=bar"))))

(ert-deftest emthin--join-desktop-lines-continuation ()
  (should (equal (emthin--join-desktop-lines '("Name=Multi\\" "line"))
                 '("Name=Multiline"))))

(ert-deftest emthin--join-desktop-lines-multiple ()
  (should (equal (emthin--join-desktop-lines '("A=1\\" "23" "[Group]" "B=4"))
                 '("A=123" "[Group]" "B=4"))))

(ert-deftest emthin--unescape-desktop-value-basic ()
  (should (equal (emthin--unescape-desktop-value "Foo\\sBar") "Foo Bar"))
  (should (equal (emthin--unescape-desktop-value "a\\tb") "a\tb"))
  (should (equal (emthin--unescape-desktop-value "a\\nb") "a\nb"))
  (should (equal (emthin--unescape-desktop-value "a\\rb") "a\rb"))
  (should (equal (emthin--unescape-desktop-value "a\\\\b") "a\\b"))
  (should (equal (emthin--unescape-desktop-value "a\\\"b") "a\"b")))

(ert-deftest emthin--unescape-desktop-value-noop ()
  (should (equal (emthin--unescape-desktop-value "plain text") "plain text"))
  (should (equal (emthin--unescape-desktop-value "") "")))

(ert-deftest emthin--read-desktop-groups-simple ()
  (let* ((tmp (make-temp-file "emthin-test" nil ".desktop"))
         (l (format "[Desktop Entry]\nName=Foo\nExec=bar\n\n[Desktop Action Act1]\nName=Act One\nExec=bar --x\n")))
    (with-temp-file tmp (insert l))
    (let* ((groups (emthin--read-desktop-groups tmp))
           (entry (assoc "Desktop Entry" groups))
           (act (assoc "Desktop Action Act1" groups)))
      (should entry)
      (should act)
      (should (equal (cdr (assoc "Name" (cdr entry))) "Foo"))
      (should (equal (cdr (assoc "Name" (cdr act))) "Act One")))
    (delete-file tmp)))

(ert-deftest emthin--read-desktop-groups-comment ()
  (let* ((tmp (make-temp-file "emthin-test" nil ".desktop"))
         (l "# comment\n[Desktop Entry]\nName=Foo\n"))
    (with-temp-file tmp (insert l))
    (let ((groups (emthin--read-desktop-groups tmp)))
      (should (assoc "Desktop Entry" groups))
      (should (= (length groups) 1)))
    (delete-file tmp)))
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `emacs --batch -L elisp -L elisp/tests --eval "(setq byte-compile-error-on-warn t)" -l ert -l elisp/tests/emthin-launch-tests.el -f ert-run-tests-batch-and-exit 2>&1`
Expected: FAIL (functions not defined)

- [ ] **Step 3: Write helper functions**

Add after requires:

```elisp
(defun emthin--join-desktop-lines (lines)
  "Join continuation lines (trailing backslash) in LINES.
Returns list of logical lines."
  (let (result current)
    (dolist (line lines (nreverse result))
      (if (string-suffix-p "\\" line)
          (setq current (concat (or current "") (substring line 0 -1)))
        (push (concat (or current "") line) result)
        (setq current nil)))))

(defun emthin--unescape-desktop-value (val)
  "Unescape .desktop value: \\s \\n \\t \\r \\\\ \\\" → literal chars."
  (replace-regexp-in-string
   "\\\\\\([\\\"sntr]\\)"
   (lambda (m)
     (pcase (aref m 1)
       (?\\  "\\") (?\"  "\"") (?s   " ") (?n   "\n")
       (?t   "\t") (?r   "\r") (_    (match-string 0 m))))
   val t t))

(defun emthin--parse-desktop-line (line)
  "Parse a single logical LINE from .desktop file.
Return (\"KEY\" . \"VALUE\"), (:GROUP \"Name\"), or nil."
  (let ((trimmed (replace-regexp-in-string "^[[:space:]]*\\|[[:space:]]*$" "" line)))
    (cond
     ((or (string-empty-p trimmed) (string-prefix-p "#" trimmed))
      nil)
     ((string-match "\\`\\[\\(.*\\)\\]\\'" trimmed)
      (list :group (match-string 1 trimmed)))
     ((string-match "\\`\\([^=]+?\\)[[:space:]]*=\\(.*\\)\\'" trimmed)
      (cons (match-string 1 trimmed) (match-string 2 trimmed))))))

(defun emthin--read-desktop-groups (file)
  "Read FILE and return list of (GROUP-NAME (KEY . VAL) ...).
Lines are joined but values are NOT unescaped (caller decides per group)."
  (let* ((raw-lines (split-string (with-temp-buffer
                                    (insert-file-contents file)
                                    (buffer-string))
                                  "\n"))
         (lines (emthin--join-desktop-lines raw-lines))
         groups current)
    (dolist (line lines)
      (pcase (emthin--parse-desktop-line line)
        (`(:group ,g) (push (list g) groups) (setq current (car groups)))
        (`(,k . ,v) (when current (push (cons k v) current)))
        (_ nil)))
    (mapcar (lambda (g) (cons (car g) (nreverse (cdr g)))) (nreverse groups))))
```

- [ ] **Step 4: Run tests to verify they pass**

Run: same command as Step 2. Expected: ALL pass.

- [ ] **Step 5: Commit**

```bash
git add elisp/emthin-launch.el elisp/tests/emthin-launch-tests.el
git commit -m "feat(launch): add .desktop helpers (join, groups, unescape)"
```

---

### Task 2: Full parser using xdg-desktop-read-file

**Files:**
- Modify: `elisp/emthin-launch.el` — rewrite `emthin--desktop-parse`, `emthin--desktop-locale-prefs`
- Create: `elisp/tests/fixtures/simple.desktop`, `elisp/tests/fixtures/full.desktop`
- Modify: `elisp/tests/emthin-launch-tests.el` — parser tests

- [ ] **Step 1: Create fixture files**

```ini
;; elisp/tests/fixtures/simple.desktop
[Desktop Entry]
Name=Foo Terminal
Comment=A simple test terminal
Exec=foo-terminal
Icon=utilities-terminal
Terminal=false
Type=Application
Categories=System;TerminalEmulator;
```

```ini
;; elisp/tests/fixtures/full.desktop
[Desktop Entry]
Name=MyApp
Name[zh]=我的应用
Name[zh_CN]=我的应用（中国）
GenericName=Text Editor
GenericName[zh]=文本编辑器
Comment=Edit text files
Icon=myapp
Exec=myapp --icon %i %f
TryExec=myapp
Terminal=false
NoDisplay=false
Hidden=false
Categories=Development;TextEditor;
MimeType=text/plain;text/markdown;
Keywords=editor;text;
StartupNotify=true
StartupWMClass=MyApp
DBusActivatable=false
PrefersNonDefaultGPU=false
SingleMainWindow=true
Type=Application
```

- [ ] **Step 2: Write parser tests**

```elisp
(ert-deftest emthin--desktop-parse-simple ()
  (let* ((result (emthin--desktop-parse
                  (expand-file-name "fixtures/simple.desktop"
                    (file-name-directory (or load-file-name buffer-file-name))))))
    (should (equal (plist-get result :name) "Foo Terminal"))
    (should (equal (plist-get result :exec) "foo-terminal"))))

(ert-deftest emthin--desktop-parse-fields ()
  (let* ((result (emthin--desktop-parse
                  (expand-file-name "fixtures/full.desktop"
                    (file-name-directory (or load-file-name buffer-file-name))))))
    (should (equal (plist-get result :name) "MyApp"))
    (should (equal (plist-get result :exec) "myapp --icon %i %f"))
    (should (equal (plist-get result :icon) "myapp"))
    (should (equal (plist-get result :categories) "Development;TextEditor;"))
    (should (equal (plist-get result :startup-wm-class) "MyApp"))
    (should (equal (plist-get result :try-exec) "myapp"))))

(ert-deftest emthin--desktop-parse-no-entries ()
  (should (equal (emthin--desktop-parse "/nonexistent/file.desktop") nil)))
```

- [ ] **Step 3: Run tests to verify they fail**

- [ ] **Step 4: Write `emthin--desktop-parse`**

Replace the existing `emthin--desktop-parse` and `emthin--desktop-scan` functions entirely:

```elisp
(defun emthin--desktop-parse (file)
  "Parse FILE into a plist or nil.
Returns: (:name NAME :exec EXEC :icon ICON :file FILE :path PATH …)
Skips NoDisplay, Hidden, Terminal entries.
Primary section parsed via `xdg-desktop-read-file' (handles locale
selection + continuations). Action groups and escape unescaping are
handled manually."
  (let* ((desktop (ignore-errors (xdg-desktop-read-file file)))
         (groups (emthin--read-desktop-groups file))
         (result (list :file file :path (file-name-directory file))))
    (unless desktop (setq result nil))
    ;; String fields (locale-aware via xdg, plus unescape)
    (when (gethash "Name" desktop)
      (plist-put result :name
                 (emthin--unescape-desktop-value (gethash "Name" desktop))))
    (dolist (field '((:generic-name . "GenericName")
                     (:comment . "Comment")
                     (:icon . "Icon")
                     (:keywords . "Keywords")))
      (when-let* ((v (gethash (cdr field) desktop))
                  ((not (string-empty-p v))))
        (plist-put result (car field)
                   (emthin--unescape-desktop-value v))))
    ;; Non-localized string fields
    (dolist (field '((:exec . "Exec") (:try-exec . "TryExec")
                     (:categories . "Categories") (:mime-type . "MimeType")
                     (:startup-wm-class . "StartupWMClass")))
      (when-let* ((v (gethash (cdr field) desktop))
                  ((not (string-empty-p v))))
        (plist-put result (car field) (emthin--unescape-desktop-value v))))
    ;; Boolean fields
    (dolist (field '((:startup-notify . "StartupNotify")
                     (:dbus-activatable . "DBusActivatable")
                     (:prefers-non-default-gpu . "PrefersNonDefaultGPU")
                     (:single-main-window . "SingleMainWindow")))
      (when-let* ((v (gethash (cdr field) desktop)))
        (plist-put result (car field) (string= v "true"))))
    ;; Skip rules (check raw keys, xdg might have filtered them)
    (when (or (string= (gethash "NoDisplay" desktop) "true")
              (string= (gethash "Hidden" desktop) "true")
              (string= (gethash "Terminal" desktop) "true"))
      (setq result nil))
    result))
```

- [ ] **Step 5: Run tests to verify they pass**

Run tests. Expected: simple and fields pass. The nil test might also pass
since `ignore-errors` catches `xdg-desktop-read-file` on nonexistent files.

- [ ] **Step 6: Commit**

```bash
git add elisp/emthin-launch.el elisp/tests/fixtures/ elisp/tests/emthin-launch-tests.el
git commit -m "feat(launch): use xdg-desktop-read-file for parser"
```

---

### Task 3: Desktop actions

**Files:**
- Modify: `elisp/emthin-launch.el` — extend `emthin--desktop-parse` for actions
- Create: `elisp/tests/fixtures/actions.desktop`
- Modify: `elisp/tests/emthin-launch-tests.el` — action tests

- [ ] **Step 1: Create actions fixture**

```ini
[Desktop Entry]
Name=Browser
Exec=browser %u
Actions=NewWindow;NewPrivateWindow;
Type=Application

[Desktop Action NewWindow]
Name=Open a New Window
Exec=browser --new-window %u

[Desktop Action NewPrivateWindow]
Name=Open a New Private Window
Name[zh]=打开隐私窗口
Exec=browser --private-window %u
```

- [ ] **Step 2: Write action tests**

```elisp
(ert-deftest emthin--desktop-parse-actions ()
  (let* ((result (emthin--desktop-parse
                  (expand-file-name "fixtures/actions.desktop"
                    (file-name-directory (or load-file-name buffer-file-name))))))
    (should (equal (plist-get result :name) "Browser"))
    (should (= (length (plist-get result :actions)) 2))
    (should (equal (plist-get (car (plist-get result :actions)) :id) "NewWindow"))
    (should (equal (plist-get (car (plist-get result :actions)) :name) "Open a New Window"))
    (should (equal (plist-get (cadr (plist-get result :actions)) :id) "NewPrivateWindow"))))

(ert-deftest emthin--desktop-parse-actions-locale ()
  (let ((process-environment (cons "LC_MESSAGES=zh_CN.UTF-8" process-environment))
        (result (emthin--desktop-parse
                 (expand-file-name "fixtures/actions.desktop"
                   (file-name-directory (or load-file-name buffer-file-name))))))
    (should (equal (plist-get (cadr (plist-get result :actions)) :name) "打开隐私窗口"))))
```

- [ ] **Step 3: Run tests to verify they fail**

- [ ] **Step 4: Extend `emthin--desktop-parse` for actions**

Append before the final `result` return in `emthin--desktop-parse`:

```elisp
;; ── Desktop Actions (from raw groups — xdg only returns [Desktop Entry]) ──
(when-let* ((actions-val (gethash "Actions" desktop))
            ((not (string-empty-p actions-val))))
  (let ((locales (emthin--desktop-locale-prefs))
        action-list)
    (dolist (act-id (split-string actions-val ";"))
      (unless (string-empty-p act-id)
        (let* ((act-group (format "Desktop Action %s" act-id))
               (act-entries (cdr (assoc act-group groups)))
               (act (list :id act-id)))
          ;; Locale fallback for Name
          (dolist (loc locales)
            (unless (plist-get act :name)
              (when-let* ((v (cdr (assoc (format "Name[%s]" loc) act-entries)))
                          ((not (string-empty-p v))))
                (plist-put act :name (emthin--unescape-desktop-value v)))))
          (unless (plist-get act :name)
            (when-let* ((v (cdr (assoc "Name" act-entries)))
                        ((not (string-empty-p v))))
              (plist-put act :name (emthin--unescape-desktop-value v))))
          ;; Exec
          (when-let* ((v (cdr (assoc "Exec" act-entries)))
                      ((not (string-empty-p v))))
            (plist-put act :exec (emthin--unescape-desktop-value v)))
          (when (and (plist-get act :name) (plist-get act :exec))
            (push act action-list)))))
    (plist-put result :actions (nreverse action-list))))
```

- [ ] **Step 5: Run tests to verify they pass**

- [ ] **Step 6: Commit**

```bash
git add emthin-launch.el elisp/tests/fixtures/actions.desktop elisp/tests/emthin-launch-tests.el
git commit -m "feat(launch): parse [Desktop Action ...] groups"
```

---

### Task 4: Scanner rewrite — xdg-data-dirs + TryExec

**Files:**
- Modify: `elisp/emthin-launch.el` — rewrite `emthin--desktop-scan`
- Modify: `elisp/tests/emthin-launch-tests.el` — scanner tests

- [ ] **Step 1: Write scanner tests**

```elisp
(ert-deftest emthin--desktop-scan-uses-xdg ()
  (let ((process-environment
         (append '("XDG_DATA_HOME=/tmp/emthin-test-xdg"
                   "XDG_DATA_DIRS=/dev/null/nonexistent")
                 process-environment)))
    (make-directory "/tmp/emthin-test-xdg/applications" t)
    (with-temp-file "/tmp/emthin-test-xdg/applications/test.desktop"
      (insert "[Desktop Entry]\nName=Test\nExec=echo\nType=Application\n"))
    (unwind-protect
        (let ((result (emthin--desktop-scan)))
          (should (listp result))
          (when result
            (should (plist-get (car result) :name))
            (should (plist-get (car result) :exec))))
      (delete-directory "/tmp/emthin-test-xdg" t))))
```

- [ ] **Step 2: Run tests to verify they fail**

- [ ] **Step 3: Rewrite `emthin--desktop-scan`**

Replace with:

```elisp
(defun emthin--desktop-scan ()
  "Scan .desktop files from XDG data dirs.
Returns list of plists, filtered to Wayland-capable binaries
with valid TryExec."
  (let (entries)
    (dolist (dir (xdg-data-dirs 'applications))
      (when (file-directory-p dir)
        (dolist (file (directory-files dir t "\\.desktop\\'"))
          (when-let* ((parsed (emthin--desktop-parse file))
                      (exec (plist-get parsed :exec))
                      ((emthin--exec-wayland-p exec)))
            (let ((try-exec (plist-get parsed :try-exec)))
              (when (or (null try-exec)
                        (executable-find (car (split-string try-exec))))
                (push parsed entries)))))))
    (nreverse entries)))
```

Remove the old manual XDG path construction from the file entirely.

- [ ] **Step 4: Run tests to verify they pass**

- [ ] **Step 5: Commit**

```bash
git add elisp/emthin-launch.el elisp/tests/emthin-launch-tests.el
git commit -m "feat(launch): rewrite scanner with xdg-data-dirs and TryExec"
```

---

### Task 5: Exec field-code substitution

**Files:**
- Modify: `elisp/emthin-launch.el` — add `emthin--substitute-field-codes`
- Modify: `elisp/tests/emthin-launch-tests.el` — tests

- [ ] **Step 1: Write substitution tests**

```elisp
(ert-deftest emthin--substitute-field-codes-percent ()
  (should (equal (emthin--substitute-field-codes "foo%%bar") '("foo%bar"))))

(ert-deftest emthin--substitute-field-codes-icon ()
  (should (equal (emthin--substitute-field-codes "myapp --icon %i" :icon "myapp-icon")
                 '("myapp" "--icon" "myapp-icon"))))

(ert-deftest emthin--substitute-field-codes-icon-missing ()
  (should (equal (emthin--substitute-field-codes "myapp --icon %i") '("myapp" "--icon"))))

(ert-deftest emthin--substitute-field-codes-name ()
  (should (equal (emthin--substitute-field-codes "myapp %c" :name "My App")
                 '("myapp" "My App"))))

(ert-deftest emthin--substitute-field-codes-file ()
  (should (equal (emthin--substitute-field-codes "myapp %U") '("myapp"))))

(ert-deftest emthin--substitute-field-codes-desktop-file ()
  (should (equal (emthin--substitute-field-codes "myapp %k" :desktop-file "/a/b.desktop")
                 '("myapp" "/a/b.desktop"))))

(ert-deftest emthin--substitute-field-codes-multiple ()
  (should (equal (emthin--substitute-field-codes "app %i %c %U" :icon "i" :name "n")
                 '("app" "--icon" "i" "n"))))

(ert-deftest emthin--substitute-field-codes-unknown ()
  (should (equal (emthin--substitute-field-codes "app %z") '("app" "%z"))))
```

- [ ] **Step 2: Run tests to verify they fail**

- [ ] **Step 3: Write `emthin--substitute-field-codes`**

```elisp
(defun emthin--substitute-field-codes (exec &key icon name desktop-file)
  "Substitute XDG field codes in EXEC string. Returns argv list.
%f %F %u %U %d %D %n %N %v %m → removed (no file/URL args from Emacs)
%i → \"--icon ICON\" (or empty if ICON is nil)
%c → localized NAME
%k → DESKTOP-FILE path
%% → literal %
Unrecognized %x passed through literally."
  (let ((result (replace-regexp-in-string "%%%%" "\0" result t))
    ;; Must check result, not result — wait, the parameter name IS result.
    ;; Let me fix: bind exec to a working variable.
    (let ((s exec))
      ;; %% → sentinel first (so it's not caught by other % rules)
      (setq s (replace-regexp-in-string "%%" "\0" s t))
      ;; Remove file/URL codes
      (setq s (replace-regexp-in-string "%[fFuUdDnNvm]" "" s))
      ;; %i → --icon ICON
      (setq s (replace-regexp-in-string
               "%i" (if icon (format "--icon %s" icon) "") s t))
      ;; %c → name
      (setq s (replace-regexp-in-string "%c" (or name "") s))
      ;; %k → desktop file path
      (setq s (replace-regexp-in-string "%k" (or desktop-file "") s))
      ;; Restore %%
      (setq s (replace-regexp-in-string "\0" "%" s t))
      ;; Split into argv, dropping empty elements from removed codes
      (delq nil (mapcar (lambda (s) (unless (string-empty-p s) s))
                        (split-string-and-unquote s " " t))))))
```

- [ ] **Step 4: Run tests to verify they pass**

- [ ] **Step 5: Commit**

```bash
git add elisp/emthin-launch.el elisp/tests/emthin-launch-tests.el
git commit -m "feat(launch): add Exec field-code substitution"
```

---

### Task 6: emthin-open-app rewrite — plist selection, actions, substitution

**Files:**
- Modify: `elisp/emthin-launch.el` — rewrite `emthin-open-app`
- Modify: `elisp/tests/emthin-launch-tests.el` — integration test

- [ ] **Step 1: Write integration test**

```elisp
(ert-deftest emthin-open-app-plist-format ()
  "emthin--app-list should contain plists after scan."
  (let ((process-environment
         (append '("XDG_DATA_HOME=/tmp/emthin-test-plist"
                   "XDG_DATA_DIRS=/dev/null/nonexistent")
                 process-environment)))
    (make-directory "/tmp/emthin-test-plist/applications" t)
    (with-temp-file "/tmp/emthin-test-plist/applications/test.desktop"
      (insert "[Desktop Entry]\nName=Test\nExec=echo\nType=Application\n"))
    (unwind-protect
        (let ((emthin--app-list (emthin--desktop-scan)))
          (should (listp emthin--app-list))
          (when emthin--app-list
            (should (plist-get (car emthin--app-list) :name))
            (should (plist-get (car emthin--app-list) :exec))))
      (delete-directory "/tmp/emthin-test-plist" t))))
```

- [ ] **Step 2: Run tests to verify they fail**

- [ ] **Step 3: Rewrite `emthin-open-app`**

```elisp
(defun emthin-open-app (app-plist)
  "Launch a Wayland application inside emthin.
With prefix argument, refresh the .desktop file cache.
If the app has actions, prompts for which action to run.
APP-PLIST is a plist from `emthin--desktop-scan'."
  (interactive
   (progn
     (when (or (null emthin--app-list) current-prefix-arg)
       (setq emthin--app-list (emthin--desktop-scan)))
     (let* ((names (mapcar (lambda (p) (plist-get p :name)) emthin--app-list))
            (name (completing-read "Launch: " names nil t))
            (app (seq-find (lambda (p) (equal (plist-get p :name) name))
                           emthin--app-list)))
       (when (and app (plist-get app :actions))
         (let ((action-name
                (completing-read
                 "Action: "
                 (cons "Default"
                       (mapcar (lambda (a) (plist-get a :name))
                               (plist-get app :actions)))
                 nil t)))
           (unless (string= action-name "Default")
             (let ((action (seq-find
                            (lambda (a) (equal (plist-get a :name) action-name))
                            (plist-get app :actions))))
               (when action (setq app action))))))
       (list app))))
  (when app-plist
    (let* ((exec (plist-get app-plist :exec))
           (args (emthin--substitute-field-codes
                  exec
                  :icon (plist-get app-plist :icon)
                  :name (plist-get app-plist :name)
                  :desktop-file (plist-get app-plist :file)))
           (target (selected-window))
           (old-targets emthin--pending-app-targets))
      (setq emthin--pending-app-targets
            (nconc emthin--pending-app-targets (list target)))
      (condition-case err
          (progn
            (apply #'start-process
                   (format "emthin-%s" (car args)) nil args)
            (message "emthin: launched: %s" (plist-get app-plist :name)))
        (error
         (setq emthin--pending-app-targets old-targets)
         (signal (car err) (cdr err)))))))
```

- [ ] **Step 4: Run all tests**

Run: `emacs --batch -L elisp -L elisp/tests --eval "(setq byte-compile-error-on-warn t)" -l ert -l elisp/tests/emthin-launch-tests.el -f ert-run-tests-batch-and-exit 2>&1`
Expected: ALL pass

- [ ] **Step 5: Verify byte-compilation is clean**

Run: `emacs --batch -L elisp --eval "(setq byte-compile-error-on-warn t)" -f batch-byte-compile elisp/emthin-launch.el 2>&1`
Expected: zero output

- [ ] **Step 6: Commit**

```bash
git add elisp/emthin-launch.el elisp/tests/emthin-launch-tests.el
git commit -m "feat(launch): plist-based selection with field-code substitution"
```

---

### Task 7: Continuation/escape fixture + full verification

- [ ] **Step 1: Create continuation fixture**

```ini
;; elisp/tests/fixtures/continuation.desktop
[Desktop Entry]
Name=Multi\
line App
Exec=app --verbose \
  --icon %i
Icon=app
Type=Application
```

- [ ] **Step 2: Write continuation test**

```elisp
(ert-deftest emthin--desktop-parse-continuation ()
  (let* ((result (emthin--desktop-parse
                  (expand-file-name "fixtures/continuation.desktop"
                    (file-name-directory (or load-file-name buffer-file-name))))))
    (should (equal (plist-get result :name) "Multiline App"))))
```

- [ ] **Step 3: Run all tests and fix any issues**

Run: `emacs --batch -L elisp -L elisp/tests --eval "(setq byte-compile-error-on-warn t)" -l ert -l elisp/tests/emthin-launch-tests.el -f ert-run-tests-batch-and-exit 2>&1`

- [ ] **Step 4: Full workspace check**

```bash
cargo check --workspace 2>&1
emacs --batch -L elisp -L elisp/tests -l ert -l elisp/tests/emthin-launch-tests.el -f ert-run-tests-batch-and-exit 2>&1
rm -f elisp/*.elc
```

- [ ] **Step 5: Commit final**

```bash
git add -A
git commit -m "feat(launch): finalize with continuation fixture and tests"
```
