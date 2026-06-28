# MigrationPolicy Design

Date: 2026-06-28
Status: Draft

## Problem

When Emacs switches workspaces (e.g. `C-x 5 o`), the compositor swaps the
active `Space<Window>` but does not ensure that apps whose `workspace_id`
matches the new active workspace are actually mapped in `active_space`.
Emacs must send `set_geometry` IPC for every app to fix up the mapping,
creating unnecessary IPC round-trips and a window where the compositor's
spatial state is inconsistent with its workspace-affinity metadata.

## Solution

A global `MigrationPolicy` enum on `EmthinState` that controls whether
`switch_workspace()` re-syncs app-to-space mappings automatically.

### Enum

```rust
pub enum MigrationPolicy {
    /// No auto-migration. Emacs drives migration via IPC `set_geometry`.
    /// The compositor does nothing beyond the existing Space swap.
    Manual,
    /// On workspace switch, re-sync all apps so that:
    ///   workspace_id == active_id  ⇒  mapped in active_space
    ///   workspace_id != active_id  ⇒  unmapped from active_space
    ByWorkspaceAffinity,
}
```

Serialized as strings over IPC: `"manual"` / `"by_workspace_affinity"`.

### State changes

- `EmthinState` gains `migration_policy: MigrationPolicy` field.
- Default value: `ByWorkspaceAffinity` (better out-of-box experience).
- `set_migration_policy(policy)` method on `EmthinState`.

### `switch_workspace` changes

After the Space swap + `active_id` update, before focus/ime/cursor reset:

```
for each app:
  check (workspace_id == active_id) vs (mapped in active_space)
  if mismatch:
    workspace_id == active_id && !in_active  →  map_element at (1,1)
    workspace_id != active_id && in_active   →  unmap_elem
```

Map position is `(1, 1)` — a placeholder; the actual geometry arrives
via the next `set_geometry` IPC from Emacs.

### IPC changes

New `IncomingMessage` variant:

```rust
SetMigrationPolicy { policy: MigrationPolicy },
```

Parsed from `:policy "manual"` or `:policy "by_workspace_affinity"`.
No outgoing message — the policy is global and the compositor does not
need to confirm.

### Elisp changes

`emthin-app.el`:

```elisp
(defun emthin--set-migration-policy (policy)
  (emthin--send 'set-migration-policy `(:policy ,(symbol-name policy))))
```

No dispatch changes needed (no outgoing notification).

### Edge cases

- **Workspace destruction:** existing behavior unchanged — apps in the
  destroyed workspace are cleaned up regardless of policy.
- **Race with `set_geometry`:** both paths call `migrate_app_to_active`
  (which is a no-op if `workspace_id == active_id`), then map. No
  conflict because all state changes happen on the same event-loop thread.
- **App with stale `workspace_id`:** if Emacs creates an app while a
  different workspace is active, the app gets `workspace_id = active_id`
  of the time of creation. On switch to the correct workspace, the re-sync
  maps it correctly.

## Files changed

| File | Change |
|---|---|
| `crates/emthin/src/state/migration.rs` | new — `MigrationPolicy` enum |
| `crates/emthin/src/state/mod.rs` | add field + method + re-sync in `switch_workspace` |
| `crates/emthin/src/state/apps.rs` | no changes needed — `windows()` / `windows_mut()` already exist |
| `crates/emthin/src/ipc/messages.rs` | new `IncomingMessage` variant + parse + test |
| `crates/emthin/src/ipc/dispatch.rs` | handle `SetMigrationPolicy` |
| `crates/emthin/src/lib.rs` | export `state::migration::MigrationPolicy` |
| `elisp/emthin-app.el` | new `emthin--set-migration-policy` |
