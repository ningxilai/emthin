# Elisp Thunk Pattern Refactoring

## Goal

Replace the deferred thunk pattern (`*-thunks` factories + `emskin--exec-effects`)
with direct imperative calls, using `cl-lib` (`cl-flet`/`cl-labels`) for local
function organization where appropriate.

## Motivation

The current pattern collects lambdas into lists (thunks) and executes them
sequentially via `emskin--exec-effects`. This is unnecessary indirection for
side-effect code — the operations are always executed immediately after
collection, there's no genuine laziness or memoization. The collection +
`append` + `nreverse` + `dolist` machinery adds ~40 lines of boilerplate
with no benefit.

## Scope

Three files, ~50 lines net removal:

### `emskin-app.el`

| Current | → | After |
|---|---|---|
| `emskin--exec-effects` | remove | |
| `emskin--report-geometry-thunks` | inline into sole caller `emskin--report-geometry`; merge error handling | |
| `emskin--mirror-thunks` | inline into `emskin--sync-frame` body | |
| `emskin--wid-wins-decoration-thunks` | inline into `emskin--sync-frame` body | |
| `emskin--sync-focus-thunks` | inline into `emskin--sync-focus` body | |
| `emskin--sync-frame` body | straight-line: decoration → per-buffer sync → counter save | |

### `emskin-workspace.el`

| Current | → | After |
|---|---|---|
| `emskin--suppress-workspace-switch-thunks` | rename to `emskin--suppress-workspace-switch`; execute directly instead of returning thunks | |

Three callers of `emskin--suppress-workspace-switch-thunks` update to new
signature and remove surrounding `emskin--exec-effects` / `append`.

### `emskin-ipc.el`

| Current | → | After |
|---|---|---|
| `emskin--send-thunk` | remove (dead code, defined but never called) | |

## Error isolation

Current thunk pattern naturally isolates errors (one thunk fails, rest still
run). Replace with `ignore-errors` wrappers at the group level in
`emskin--sync-frame` for each operation group (decoration / per-buffer sync).

## Verification

- `byte-compile-error-on-warn`: zero warnings
- All 28 IPC Rust unit tests pass
- Current behavior preserved — no protocol or semantic change

## Non-goals

- No EIEIO classes. cl-lib local functions are sufficient.
- No change to Rust side.
- No change to IPC wire format.
- No change to external API (interactive commands, hooks).
