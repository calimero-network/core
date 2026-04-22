# calimero-client-py: reparent bindings + cascade-delete signature

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Mirror core's group-API reshape in the calimero-client-py pyo3 bindings: drop `nest_group` / `unnest_group` Python methods, add `reparent_group`, update `create_group` to require `parent_id`. Restore green build + tests once core master has the new Rust API.

**Spec:** `../core/docs/superpowers/specs/2026-04-22-strict-group-tree-and-cascade-delete.md` (in the core repo)

**Architecture:** calimero-client-py is a pyo3+maturin wrapper. Its `src/client.rs` (Rust) defines `#[pymethods]` that delegate to the underlying `calimero_client::Client` from core (pinned via `git = ".../core", branch = "master"`). When core's API changes, this crate's master build breaks until the wrappers are updated to match.

**Tech Stack:** Rust 2021 (pyo3 0.22 / 0.23, maturin), Python 3.11+, pytest with asyncio.

**Landing order:** This PR is **#2** in the sequence:
1. core PR (lands first; breaks this repo's master build)
2. **THIS PR** (lands within minutes of core)
3. merobox PR (lands after this)

Repo path: `/Users/ronitchawla/Developer/Calimero/calimero-client-py`

---

## File structure

### Files modified

- `src/client.rs` — drop `nest_group()` and `unnest_group()` `#[pymethods]`; add `reparent_group()`; update `create_group()` to accept `parent_id`.
- `calimero/__init__.py` — re-export shape may need adjustment if any python helpers reference the dropped methods.
- `tests/test_basic.py` — replace nest/unnest tests with reparent tests; update create_group tests.
- `tests/conftest.py` — fixtures may need updating if they pre-create groups.
- `docs/namespaces.html` — update docs to mention `reparent` (cosmetic but shipped with releases).
- `README.md` — same.
- `example_usage.py` — same.

### No new files needed.

### Files deleted

- None — all changes are in-place.

---

## Tasks

### Task 1: Pin the working core branch in Cargo.toml

**Files:**
- Modify: `Cargo.toml`

> Rationale: Cargo.toml currently pins `branch = "master"`. Until core merges its PR, you need to point at the core feature branch (`feat/strict-group-tree-cascade-delete`) for local testing. After core merges, revert to master before merging this PR.

- [ ] **Step 1: Repoint to the core feature branch**

In `Cargo.toml`, change every occurrence of:

```toml
calimero-client = { git = "https://github.com/calimero-network/core", branch = "master" }
```

(and the three sibling lines for `calimero-primitives`, `calimero-server-primitives`, `calimero-context-config`)

To:

```toml
calimero-client = { git = "https://github.com/calimero-network/core", branch = "feat/strict-group-tree-cascade-delete" }
```

(and same for the other three)

- [ ] **Step 2: Update lock**

```bash
cargo update -p calimero-client -p calimero-primitives -p calimero-server-primitives -p calimero-context-config
```

- [ ] **Step 3: Confirm build still fails (because client.rs hasn't been updated yet)**

```bash
cargo check 2>&1 | tail -20
```

Expected: errors `no method named 'nest_group' found for ...` (or similar). These confirm we're now compiling against the new core API. Subsequent tasks fix them.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: pin to core feat/strict-group-tree-cascade-delete branch

Temporary pin during coordinated landing. Revert to master in the
final commit of this PR (after core PR merges)."
```

---

### Task 2: Drop `nest_group` and `unnest_group` pymethods, add `reparent_group`

**Files:**
- Modify: `src/client.rs`
- Test: `tests/test_basic.py`

- [ ] **Step 1: Locate the methods**

```bash
grep -n "fn nest_group\|fn unnest_group\|fn create_group\|fn delete_group" src/client.rs
```

Note the line ranges so you can see the whole method bodies.

- [ ] **Step 2: Write the failing test**

Append to `tests/test_basic.py`:

```python
def test_client_has_reparent_group_method():
    """The pyo3 wrapper must expose reparent_group()."""
    from calimero_client_py import CalimeroClient
    assert hasattr(CalimeroClient, "reparent_group"), \
        "CalimeroClient.reparent_group missing — pyo3 binding not registered"

def test_client_does_not_have_nest_group_method():
    """nest_group has been removed in the strict-tree refactor."""
    from calimero_client_py import CalimeroClient
    assert not hasattr(CalimeroClient, "nest_group"), \
        "CalimeroClient.nest_group should be removed"

def test_client_does_not_have_unnest_group_method():
    from calimero_client_py import CalimeroClient
    assert not hasattr(CalimeroClient, "unnest_group"), \
        "CalimeroClient.unnest_group should be removed"
```

These are introspection tests — they don't need a running node, so they're safe unit tests.

- [ ] **Step 3: Run failing tests**

```bash
maturin develop && pytest tests/test_basic.py::test_client_has_reparent_group_method -v
```

Expected: FAIL — either `maturin develop` fails (compile error from Task 1's pin) or the test fails because `reparent_group` doesn't exist.

- [ ] **Step 4: Delete `nest_group` and `unnest_group` `#[pymethods]`**

In `src/client.rs`, delete the entire `pub fn nest_group(...)` and `pub fn unnest_group(...)` blocks (you noted line ranges in Step 1).

- [ ] **Step 5: Add `reparent_group`**

Add immediately after `delete_group`:

```rust
pub fn reparent_group(
    &self,
    group_id: &str,
    new_parent_id: &str,
) -> PyResult<PyObject> {
    let inner = self.inner.clone();
    let group_id = group_id.to_string();
    let new_parent_id = new_parent_id.to_string();

    Python::with_gil(|py| {
        let result = self.runtime.block_on(async move {
            inner
                .reparent_group(
                    &group_id,
                    admin::ReparentGroupApiRequest {
                        new_parent_id: parse_group_id(&new_parent_id)?,
                        requester: None,
                    },
                )
                .await
        });
        match result {
            Ok(data) => {
                let json_data = serde_json::to_value(data).map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                        "Failed to serialize response: {}",
                        e
                    ))
                })?;
                Ok(json_to_python(py, &json_data))
            }
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                "Client error: {}",
                e
            ))),
        }
    })
}
```

If `parse_group_id` doesn't exist as a helper in `src/client.rs`, look at how the existing `nest_group` (now-deleted) parsed `child_group_id` from the input string — copy that pattern.

If `admin::ReparentGroupApiRequest` doesn't have a `new_parent_id: ContextGroupId` field but rather `[u8; 32]`, adjust the call accordingly. Check by grepping the core source: `cd ../core && grep -A5 "struct ReparentGroupApiRequest" crates/server/primitives/`.

- [ ] **Step 6: Rebuild + run the test**

```bash
maturin develop && pytest tests/test_basic.py::test_client_has_reparent_group_method tests/test_basic.py::test_client_does_not_have_nest_group_method tests/test_basic.py::test_client_does_not_have_unnest_group_method -v
```

Expected: PASS, 3 tests.

- [ ] **Step 7: Commit**

```bash
git add src/client.rs tests/test_basic.py
git commit -m "feat: drop nest_group/unnest_group pymethods, add reparent_group

Mirrors core's strict-tree refactor. nest_group and unnest_group are
removed entirely; reparent_group(group_id, new_parent_id) replaces them
with a single atomic edge-swap call."
```

---

### Task 3: Update `create_group` to accept `parent_id`

**Files:**
- Modify: `src/client.rs`
- Test: `tests/test_basic.py`

- [ ] **Step 1: Write the failing test**

Append to `tests/test_basic.py`:

```python
def test_create_group_signature_includes_parent_id():
    """create_group's signature must include a parent_id parameter."""
    from calimero_client_py import CalimeroClient
    import inspect
    sig = inspect.signature(CalimeroClient.create_group)
    params = list(sig.parameters.keys())
    assert "parent_id" in params, \
        f"parent_id missing from create_group signature: {params}"
```

- [ ] **Step 2: Run the failing test**

```bash
pytest tests/test_basic.py::test_create_group_signature_includes_parent_id -v
```

Expected: FAIL.

- [ ] **Step 3: Update `create_group` pymethod**

In `src/client.rs`, find `pub fn create_group(...)`. Add `parent_id: &str` parameter and pass it through:

```rust
pub fn create_group(
    &self,
    parent_id: &str,                  // NEW
    application_id: &str,
    // ... other existing params ...
) -> PyResult<PyObject> {
    let inner = self.inner.clone();
    let parent_id = parent_id.to_string();
    // ... rest of existing setup ...

    Python::with_gil(|py| {
        let result = self.runtime.block_on(async move {
            inner
                .create_group(
                    admin::CreateGroupApiRequest {
                        parent_id: parse_group_id(&parent_id)?,   // NEW
                        application_id: parse_application_id(&application_id)?,
                        // ... other existing fields ...
                    },
                )
                .await
        });
        // ... rest unchanged ...
    })
}
```

The exact existing param list for `create_group` will vary; adjust around what's already there.

- [ ] **Step 4: Update any usage in the rest of the binding**

```bash
grep -n "create_group(" src/client.rs
```

Wherever the binding internally calls `create_group` (rare; usually only the pymethod itself), update to pass the new arg.

- [ ] **Step 5: Rebuild + test**

```bash
maturin develop && pytest tests/test_basic.py::test_create_group_signature_includes_parent_id -v
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/client.rs tests/test_basic.py
git commit -m "feat: create_group requires parent_id

Mirrors core's CreateGroupApiRequest change. The parent_id must
reference an existing group in the namespace (the namespace root or
a previously-created subgroup)."
```

---

### Task 4: Update docs and example usage

**Files:**
- Modify: `docs/namespaces.html`
- Modify: `README.md`
- Modify: `example_usage.py`

- [ ] **Step 1: Update `docs/namespaces.html`**

Replace the `nest_group` and `unnest_group` doc sections with a single `reparent_group` section:

```html
<h4>reparent_group(group_id, new_parent_id)</h4>
<p>
  Atomically move <code>group_id</code> from its current parent to
  <code>new_parent_id</code>. Both groups must already exist in the
  same namespace. Rejects: namespace root, cycles
  (new_parent is a descendant of group_id), nonexistent new parent.
</p>
<pre><code><span class="field">client</span>.<span class="kw">reparent_group</span>(<span class="comment">"child-id"</span>, <span class="comment">"new-parent-id"</span>)</code></pre>
```

- [ ] **Step 2: Update `README.md`**

Find the section that mentions `nest_group` / `unnest_group`. Replace with `reparent_group` examples.

- [ ] **Step 3: Update `example_usage.py`**

```bash
grep -n "nest_group\|unnest_group" example_usage.py
```

For each occurrence, replace with `reparent_group` calls. If a workflow specifically demonstrated detaching (which is now forbidden), replace it with a reparent demo.

- [ ] **Step 4: Commit**

```bash
git add docs/namespaces.html README.md example_usage.py
git commit -m "docs: replace nest/unnest docs with reparent_group

Update docs/namespaces.html, README.md, and example_usage.py to
reflect the new reparent_group API. Removes references to the
removed nest_group / unnest_group methods."
```

---

### Task 5: Run the full pytest suite

- [ ] **Step 1: Run all tests**

```bash
maturin develop && pytest -v 2>&1 | tail -30
```

Expected: all tests pass. If integration tests in `test_integration_simple.py` reference `nest_group` / `unnest_group`, they'll fail; update them to use `reparent_group`.

- [ ] **Step 2: Run pre-commit hooks**

```bash
pre-commit run --all-files 2>&1 | tail -20
```

Expected: all hooks pass (formatting, lint).

- [ ] **Step 3: Commit any stray fixes**

```bash
git add -u
git commit -m "test+style: run pytest and pre-commit"
```

---

### Task 6: Restore master pin and open PR

- [ ] **Step 1: Wait for core PR to merge** (coordination point)

Confirm the core PR has merged to `master`:

```bash
gh pr view <core-pr-number> --repo calimero-network/core | head -10
```

Expected: state `MERGED`.

- [ ] **Step 2: Revert Cargo.toml branch pin to `master`**

In `Cargo.toml`, change all four `branch = "feat/strict-group-tree-cascade-delete"` lines back to `branch = "master"`.

```bash
cargo update -p calimero-client -p calimero-primitives -p calimero-server-primitives -p calimero-context-config
```

- [ ] **Step 3: Final build + test**

```bash
maturin develop && pytest -v 2>&1 | tail -10
```

Expected: all pass against core master.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: revert branch pin to master after core PR merge"
```

- [ ] **Step 5: Push + open PR**

```bash
git push -u origin feat/reparent-group-bindings

gh pr create --repo calimero-network/calimero-client-py --base master \
  --title "feat: reparent_group bindings + parent_id on create_group" \
  --body "$(cat <<'EOF'
## Summary

Mirrors the strict group-tree refactor in core (calimero-network/core PR #<number>):
- Drops \`nest_group()\` and \`unnest_group()\` pymethods
- Adds \`reparent_group(group_id, new_parent_id)\` (atomic edge swap)
- \`create_group()\` now requires a \`parent_id\` parameter

Spec: see core repo at \`docs/superpowers/specs/2026-04-22-strict-group-tree-and-cascade-delete.md\`.

## Coordination

This is PR 2 of 3:
1. core PR (merged)
2. **THIS PR**
3. merobox PR (depends on this)

## Test plan

- [x] Introspection tests pass (presence of reparent_group, absence of nest/unnest)
- [x] create_group signature includes parent_id
- [x] Existing pytest suite passes
- [x] pre-commit hooks pass
EOF
)"
```

---

## Self-review checklist

- Spec § 8.1 (Rust client API) and § 8.3 (calimero-client-py) covered by Tasks 2, 3.
- All `nest_group` / `unnest_group` references removed from code, tests, docs (Tasks 2-4).
- New methods have unit tests (Task 2 introspection, Task 3 signature check).
- Branch-pin coordination explicitly handled (Tasks 1 + 6).
- No "TBD" / "TODO" placeholders.

---

**Done.** Coordinates with core plan via Cargo.toml branch pin (Task 1) and PR-merge gating (Task 6).
