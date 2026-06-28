# MigrationPolicy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `MigrationPolicy` enum controlling whether workspace switch auto-syncs app-to-space mappings.

**Architecture:** Two-variant enum (`Manual` / `ByWorkspaceAffinity`) on `EmthinState`. New module `state/migration.rs`. In `switch_workspace()`, after Space swap + `active_id` update, if policy is `ByWorkspaceAffinity`, iterate all apps and fix up space mappings. IPC message `set_migration_policy` lets Elisp set the policy. Default: `ByWorkspaceAffinity`.

**Tech Stack:** Rust (smithay, serde_json), Elisp (cl-lib, eieio)

---

### Task 1: `MigrationPolicy` enum in new `state/migration.rs`

**Files:**
- Create: `crates/emthin/src/state/migration.rs`
- Modify: `crates/emthin/src/state/mod.rs` (add `pub mod migration;`)

- [ ] **Step 1: Create `state/migration.rs`**

Write the `MigrationPolicy` enum with `Display` + `FromStr` for IPC string serialization:

```rust
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationPolicy {
    /// No auto-migration. Emacs drives migration via IPC `set_geometry`.
    /// The compositor does nothing beyond the existing Space swap.
    Manual,
    /// On workspace switch, re-sync all apps so that:
    ///   workspace_id == active_id  ⇒  mapped in active_space
    ///   workspace_id != active_id  ⇒  unmapped from active_space
    ByWorkspaceAffinity,
}

impl MigrationPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::ByWorkspaceAffinity => "by_workspace_affinity",
        }
    }
}

impl fmt::Display for MigrationPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MigrationPolicy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "manual" => Ok(Self::Manual),
            "by_workspace_affinity" => Ok(Self::ByWorkspaceAffinity),
            other => Err(format!("unknown migration policy: {other}")),
        }
    }
}
```

- [ ] **Step 2: Add `pub mod migration;` to `state/mod.rs`**

Insert near the top alongside the existing module declarations:

```rust
pub mod migration;
```

Put it after the `pub mod focus;` line (after line 5).

- [ ] **Step 3: Verify it compiles**

```bash
cargo check -p emthin 2>&1 | head -20
```
Expected: adds `migration` module, no warnings.

---

### Task 2: Add `migration_policy` field to `EmthinState` + setter

**Files:**
- Modify: `crates/emthin/src/state/mod.rs`

- [ ] **Step 1: Add the field to `EmthinState` struct**

After the `needs_redraw` field (after line 229), add:

```rust
    /// Whether to auto-migrate apps on workspace switch.
    pub migration_policy: migration::MigrationPolicy,
```

- [ ] **Step 2: Initialize in the constructor**

Find the `EmthinState::new(...)` or builder pattern. Search for where `needs_redraw` is initialized:

```bash
grep -n "needs_redraw" crates/emthin/src/state/mod.rs
```

Add `migration_policy: migration::MigrationPolicy::ByWorkspaceAffinity,` right before or after the `needs_redraw` line.

- [ ] **Step 3: Add setter method on `EmthinState`**

Find a suitable location (after the `switch_workspace` method, around line 668):

```rust
    pub fn set_migration_policy(&mut self, policy: migration::MigrationPolicy) {
        self.migration_policy = policy;
        tracing::info!("migration policy set to {policy}");
    }
```

- [ ] **Step 4: Verify**

```bash
cargo check -p emthin 2>&1
```
Expected: succeeds.

---

### Task 3: Re-sync in `switch_workspace`

**Files:**
- Modify: `crates/emthin/src/state/mod.rs`

- [ ] **Step 1: Replace the old comment with re-sync logic**

Replace lines 617-619:

```
        // App migration is handled by IPC set_geometry from Emacs (sync-all).
        // The compositor does NOT auto-migrate because it doesn't know which
        // apps are displayed in which Emacs frame.
```

With:

```rust
        // Auto-migrate: ensure every app is mapped in the right space.
        if self.migration_policy == migration::MigrationPolicy::ByWorkspaceAffinity {
            let active_id = self.workspace.active_id;
            let mut to_map = Vec::new();
            let mut to_unmap = Vec::new();
            for app in self.apps.windows() {
                let in_active = self.workspace.active_space
                    .elements()
                    .any(|w| w == &app.window);
                match (app.workspace_id == active_id, in_active) {
                    (true, false) => to_map.push(app.window.clone()),
                    (false, true) => to_unmap.push(app.window.clone()),
                    _ => {}
                }
            }
            for w in &to_unmap {
                self.workspace.active_space.unmap_elem(w);
                dismiss_popups_for_window(w);
            }
            for w in &to_map {
                self.workspace.active_space.map_element(
                    w.clone(),
                    (1, 1).into(),
                    false,
                );
            }
            if !to_map.is_empty() || !to_unmap.is_empty() {
                tracing::debug!(
                    "auto-migrated: mapped={} unmapped={}",
                    to_map.len(),
                    to_unmap.len()
                );
            }
        }
```

- [ ] **Step 2: Verify**

```bash
cargo check -p emthin 2>&1
```
Expected: succeeds.

---

### Task 4: IPC `IncomingMessage::SetMigrationPolicy`

**Files:**
- Modify: `crates/emthin/src/ipc/messages.rs`

- [ ] **Step 1: Add the variant to `IncomingMessage`**

After `DbusRouterListRules` (line 72), add:

```rust
    /// Set the compositor's app migration policy.
    SetMigrationPolicy {
        policy: crate::state::migration::MigrationPolicy,
    },
```

- [ ] **Step 2: Add `from_jsonrpc` branch**

Inside the `from_jsonrpc` match, after the `"dbus_router_list_rules"` arm (line 207), add:

```rust
            "set_migration_policy" => {
                let policy_str = params_get_string(params, "policy")?;
                let policy: crate::state::migration::MigrationPolicy = policy_str
                    .parse()
                    .map_err(|e: String| format!("invalid policy: {e}"))?;
                Self::SetMigrationPolicy { policy }
            }
```

- [ ] **Step 3: Add test**

In the `tests` module at the bottom of the file, before `rejects_unknown_method` (around line 438), add:

```rust
    #[test]
    fn parses_set_migration_policy() {
        let params = serde_json::json!({"policy":"by_workspace_affinity"});
        let msg = IncomingMessage::from_jsonrpc("set_migration_policy", &params).unwrap();
        assert!(matches!(
            msg,
            IncomingMessage::SetMigrationPolicy {
                policy: crate::state::migration::MigrationPolicy::ByWorkspaceAffinity,
            }
        ));
    }

    #[test]
    fn parses_set_migration_policy_manual() {
        let params = serde_json::json!({"policy":"manual"});
        let msg = IncomingMessage::from_jsonrpc("set_migration_policy", &params).unwrap();
        assert!(matches!(
            msg,
            IncomingMessage::SetMigrationPolicy {
                policy: crate::state::migration::MigrationPolicy::Manual,
            }
        ));
    }

    #[test]
    fn rejects_invalid_migration_policy() {
        let params = serde_json::json!({"policy":"auto"});
        let result = IncomingMessage::from_jsonrpc("set_migration_policy", &params);
        assert!(result.is_err());
    }
```

- [ ] **Step 4: Verify tests pass**

```bash
cargo test -p emthin ipc::messages 2>&1
```
Expected: all IPC message tests pass, including the 3 new ones.

---

### Task 5: Handle `SetMigrationPolicy` in dispatch

**Files:**
- Modify: `crates/emthin/src/ipc/dispatch.rs`

- [ ] **Step 1: Add match arm**

In `handle_ipc_message`, after the `DbusRouterListRules` arm (line 68), add:

```rust
        IncomingMessage::SetMigrationPolicy { policy } => {
            tracing::debug!("IPC set_migration_policy {policy}");
            state.set_migration_policy(policy);
        }
```

- [ ] **Step 2: Verify**

```bash
cargo check -p emthin 2>&1
```
Expected: succeeds.

---

### Task 6: Export from `lib.rs`

**Files:**
- Modify: `crates/emthin/src/lib.rs`

- [ ] **Step 1: Add `migration` to the re-export list**

In `lib.rs` line 20, change:

```rust
pub use state::{apps, cursor, emacs, focus, ime, workspace, xwayland};
```

To:

```rust
pub use state::{apps, cursor, emacs, focus, ime, migration, workspace, xwayland};
```

- [ ] **Step 2: Verify**

```bash
cargo check -p emthin 2>&1
```
Expected: succeeds.

---

### Task 7: Elisp `emthin--set-migration-policy`

**Files:**
- Modify: `elisp/emthin-app.el`

- [ ] **Step 1: Add function**

Find a good location in `emthin-app.el` — after `emthin--on-surface-size` (around line 152) would fit since it's a simple action function. Add:

```elisp
(defun emthin--set-migration-policy (policy)
  "Set compositor migration policy to POLICY (symbol 'manual or 'by-workspace-affinity).
POLICY is sent to the compositor which controls whether embedded apps
auto-follow their workspace on workspace switch."
  (interactive "SMigration policy (manual/by-workspace-affinity): ")
  (emthin--send 'set-migration-policy `(:policy ,(symbol-name policy))))
```

- [ ] **Step 2: Byte-compile check**

```bash
emacs --batch --eval "(setq byte-compile-error-on-warn t)" -f batch-byte-compile elisp/emthin-app.el 2>&1
```
Expected: no errors or warnings.

---

### Task 8: Final verification

- [ ] **Step 1: Full crate check**

```bash
cargo fmt --all && cargo clippy --workspace -- -D warnings && cargo test --workspace 2>&1
```

Expected: all pass, zero warnings, 159+ tests pass (3 new IPC tests).

- [ ] **Step 2: Elisp verification**

```bash
emacs --batch -L elisp --eval "(setq byte-compile-error-on-warn t)" -f batch-byte-compile elisp/*.el 2>&1
```

Expected: all `.el` files compile without errors or warnings.
