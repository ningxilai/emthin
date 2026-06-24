# emthin-launch: full XDG Desktop Entry parser

## Purpose

Current `emthin-launch.el` extracts only `Name` and `Exec` from `.desktop`
files and passes the raw `Exec=` string to `start-process` without
substituting XDG field codes (`%f`, `%U`, `%i`, `%c`, …). This means many
real-world `.desktop` files produce incorrect command lines.

This spec covers a full Desktop Entry Specification v1.5 parser in Emacs
Lisp, including field-code substitution, locale fallback, complete field
extraction, and multi-section support.

## Data format

Replace the current `(NAME . EXEC)` cons cell with a plist:

```elisp
(:name          "Firefox"
 :exec          "firefox %u"
 :icon          "firefox"
 :comment       "Browse the web"
 :generic-name  "Web Browser"
 :categories    "Network;WebBrowser;"
 :mime-type     "text/html;x-scheme-handler/http;"
 :keywords      "internet;browser;"
 :try-exec      nil
 :terminal      nil
 :startup-notify t
 :startup-wm-class "Firefox"
 :dbus-activatable nil
 :prefers-non-default-gpu nil
 :single-main-window nil
 :actions       nil               ; list of action plists, same shape
 :file          "/usr/share/applications/firefox.desktop"
 :path          "/usr/share/applications")
```

Only `:name`, `:exec`, `:file`, and `:path` are guaranteed non-nil.

The `emthin--app-list` cache stores these plists instead of cons cells.
`completing-read` displays `:name` (optionally annotated with `:categories`
or `:comment` via `annotation-function`).

## Parser rewrite (`emthin--desktop-parse`)

### Grammar handled

- **Line continuation**: trailing `\` joins with the next line
- **Escape sequences**: `\s` `\n` `\t` `\r` `\\` `\"` in values
- **Groups**: `[Group Name]` — parse `[Desktop Entry]` + any
  `[Desktop Action <id>]`
- **Comments**: lines starting with `#` are ignored
- **Key/Value split**: first `=` in the line; whitespace stripped

### Fields extracted

All meaningful keys under `[Desktop Entry]`:

| Key | Stored as | Behaviour |
|---|---|---|
| `Name` | `:name` | locale-fallback (see below) |
| `GenericName` | `:generic-name` | locale-fallback |
| `Comment` | `:comment` | locale-fallback |
| `Icon` | `:icon` | locale-fallback |
| `Keywords` | `:keywords` | locale-fallback |
| `Exec` | `:exec` | raw — substitution happens in `emthin-open-app` |
| `TryExec` | `:try-exec` | validated at scan time |
| `Terminal` | `:terminal` | not stored; entry skipped if `true` |
| `NoDisplay` | | not stored; entry skipped if `true` |
| `Hidden` | | not stored; entry skipped if `true` |
| `Categories` | `:categories` | stored for future filtering |
| `MimeType` | `:mime-type` | stored |
| `StartupNotify` | `:startup-notify` | stored |
| `StartupWMClass` | `:startup-wm-class` | stored |
| `DBusActivatable` | `:dbus-activatable` | stored |
| `PrefersNonDefaultGPU` | `:prefers-non-default-gpu` | stored |
| `SingleMainWindow` | `:single-main-window` | stored |
| `Actions` | `:actions` | list of action plists (see below) |

### Locale fallback

Derive user's preferred locale from `(locale-name (current-locale))` or
`(getenv "LC_MESSAGES")`. Match in priority order:

1. Exact match: `Name[zh_CN]`
2. Language match: `Name[zh]`
3. No suffix: `Name`
4. First available suffix: `Name[en]` (any english)

Apply the same fallback to `GenericName`, `Comment`, `Icon`, `Keywords`.

### Multi-value fields (`Categories`, `MimeType`, `Keywords`, `Actions`)

Stored as semicolon-delimited strings (spec canonical form). Callers split
with `(split-string val ";")` when needed.

### Desktop actions

When the `Actions` key is present, parse each named action group as a
separate plist:

```elisp
(:actions ((:id "NewWindow" :name "Open a New Window" :exec "firefox --new-window %u")
           (:id "NewPrivateWindow" :name "..." :exec "...")))
```

Store each action's `:name`, `:exec`, and any locale-fallback fields.

## Scan rewrite (`emthin--desktop-scan`)

- Use Emacs built-in `(xdg-data-dirs)` instead of manual env var parsing
  (the `(require 'xdg)` is already present but unused)
- Skip entries where `TryExec` binary is not found via `executable-find`
- Keep the `emthin--exec-wayland-p` filter (ldd check) unchanged
- Result is a list of plists, stored in `emthin--app-list`

## Exec field-code substitution (`emthin--substitute-field-codes`)

Applied in `emthin-open-app` after selecting the app and action (if any).
Takes (EXEC &key ICON NAME DESKTOP-FILE) → list of argument strings.

| Code | Replaced with |
|---|---|
| `%%` | literal `%` |
| `%i` | `--icon <icon>` if icon present, else removed |
| `%c` | localized `:name` |
| `%k` | URI (`file://` path) of `.desktop` file |
| `%f` `%F` `%u` `%U` | removed (no file/URL arguments from Emacs) |
| `%d` `%D` `%n` `%N` `%v` `%m` | removed (deprecated, no dir/file args) |
| unrecognised `%x` | passed through literally |

Substitution replaces the code **including any surrounding whitespace
and quoting the shell would interpret**, because we are NOT going through
a shell — we pass args directly to `start-process` (which uses `execve`).
The spec assumes shell invocation; for direct-exec we:

1. Split the substituted string with `split-string-and-unquote`
2. That gives us the final argv

This means a `.desktop` file with `Exec=foot %U` becomes just `("foot")`
(no trailing empty string after splitting `"foot "`) when no URL is given.
`Exec=myapp --icon %i` with icon `"myapp"` becomes `("myapp" "--icon" "myapp")`.

## `emthin-open-app` update

1. If app has `Actions`, offer a second completing-read to pick an action
   (first item is "Default" for the main entry)
2. Substitute field codes on the chosen `:exec`
3. Validate `TryExec` at selection time (cache-check from scan)
4. Launch with `(apply #'start-process ...)` as before

## Dependencies

No new libraries. Parser uses:
- `(xdg-data-dirs)` — built-in Emacs 29+, already required
- `(current-locale)` / `(locale-name)` — built-in
- `split-string-and-unquote` — built-in
- `executable-find` — built-in

## File structure

All code stays in `emthin-launch.el`. Expected growth:

| Section | Current lines | Target lines |
|---|---|---|
| Requires + caches | 24 | 20 |
| Parser (`emthin--desktop-parse`) | 29 | 120 |
| Scanner (`emthin--desktop-scan`) | 16 | 25 |
| Field-code substitution | 0 | 50 |
| Launcher (`emthin-open-app`) | 20 | 35 |
| **Total** | **109** | **~300** |

## Testing

Add test file `elisp/tests/emthin-launch-tests.el` with fixture
`.desktop` files covering:

- Simple entries (Name, Exec only)
- All field codes in Exec
- Locale-matching order (zh_CN, zh, bare)
- Line continuations
- Escape sequences
- Actions ([Desktop Action ...])
- TryExec failure → skipped
- Terminal/NoDisplay/Hidden → skipped
- DBusActivatable (stored, not executed differently)

## Non-goals

- No MIME-type filtering at launch time (stored field only)
- No D-Bus activation — `DBusActivatable` entries still launch via exec
- No `%f`/`%F`/`%u`/`%U` passing from Emacs context (no file associations)
- No `X-` extension handling (stored in a generic `:x-keys` plist if needed later)
