# Elisp Thunk Pattern Refactoring

## Goal

Replace the deferred thunk pattern (`*-thunks` factories + `emthin--exec-effects`)
with direct imperative calls, using `cl-lib` (`cl-flet`/`cl-labels`) for local
function organization where appropriate.

## Motivation

The current pattern collects lambdas into lists (thunks) and executes them
sequentially via `emthin--exec-effects`. This is unnecessary indirection for
side-effect code — the operations are always executed immediately after
collection, there's no genuine laziness or memoization. The collection +
`append` + `nreverse` + `dolist` machinery adds ~40 lines of boilerplate
with no benefit.

## Scope

Three files, ~50 lines net removal:

### `emthin-app.el`

| Current | → | After |
|---|---|---|
| `emthin--exec-effects` | remove | |
| `emthin--report-geometry-thunks` | inline into sole caller `emthin--report-geometry`; merge error handling | |
| `emthin--mirror-thunks` | inline into `emthin--sync-frame` body | |
| `emthin--wid-wins-decoration-thunks` | inline into `emthin--sync-frame` body | |
| `emthin--sync-focus-thunks` | inline into `emthin--sync-focus` body | |
| `emthin--sync-frame` body | straight-line: decoration → per-buffer sync → counter save | |

### `emthin-workspace.el`

| Current | → | After |
|---|---|---|
| `emthin--suppress-workspace-switch-thunks` | rename to `emthin--suppress-workspace-switch`; execute directly instead of returning thunks | |

Three callers of `emthin--suppress-workspace-switch-thunks` update to new
signature and remove surrounding `emthin--exec-effects` / `append`.

### `emthin-ipc.el`

| Current | → | After |
|---|---|---|
| `emthin--send-thunk` | remove (dead code, defined but never called) | |

## Error isolation

Current thunk pattern naturally isolates errors (one thunk fails, rest still
run). Replace with `ignore-errors` wrappers at the group level in
`emthin--sync-frame` for each operation group (decoration / per-buffer sync).

## Verification

- `byte-compile-error-on-warn`: zero warnings
- All 28 IPC Rust unit tests pass
- Current behavior preserved — no protocol or semantic change

## Non-goals

- No EIEIO classes. cl-lib local functions are sufficient.
- No change to Rust side.
- No change to IPC wire format.
- No change to external API (interactive commands, hooks).
