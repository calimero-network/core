# merobox: reparent_group step type + cascade-aware delete

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `nest_group` and `unnest_group` workflow step types with a single `reparent_group` step. Update `create_group_in_namespace` step to support the new `parent_id` semantics (the namespace_id IS the parent — internal-only change). Add unit tests covering validation. Once landed, core's `group-reparent.yml` and `group-reparent-and-cascade-delete.yml` E2E workflows pass.

**Spec:** `../core/docs/superpowers/specs/2026-04-22-strict-group-tree-and-cascade-delete.md` (in the core repo)

**Architecture:** merobox dispatches workflow YAML steps to Python classes. Each step class is a `BaseStep` subclass with `_get_required_fields`, `_validate_field_types`, and an async `execute` that calls into `calimero-client-py`. We're: removing `NestGroupStep` and `UnnestGroupStep`, adding `ReparentGroupStep`, updating the dispatch table in `executor.py`, the validation table in `validator.py`, and the config-schema table in `config.py`.

**Tech Stack:** Python 3.11+, pytest, pytest-asyncio, pytest-mock, pydantic for config schemas, pre-commit (ruff, black).

**Landing order:** This PR is **#3** in the sequence:
1. core PR (lands first)
2. calimero-client-py PR (lands within minutes of core)
3. **THIS PR** (lands after calimero-client-py — depends on its `reparent_group()` Python method)

Repo path: `/Users/ronitchawla/Developer/Calimero/merobox`

---

## File structure

### Files modified

- `merobox/commands/bootstrap/steps/subgroup.py` — drop `NestGroupStep` and `UnnestGroupStep` classes; add `ReparentGroupStep`.
- `merobox/commands/bootstrap/run/executor.py` — drop dispatch arms for `nest_group` / `unnest_group`; add arm for `reparent_group`.
- `merobox/commands/bootstrap/validate/validator.py` — drop validation entries for `nest_group` / `unnest_group`; add for `reparent_group`.
- `merobox/commands/bootstrap/config.py` — drop `NestGroupStepConfig` / `UnnestGroupStepConfig` pydantic models; add `ReparentGroupStepConfig`.
- `merobox/tests/unit/test_group_steps.py` — drop tests for the removed steps; add `TestReparentGroupStep` class.
- `LLM.md` — if it lists supported step types, update.
- `README.md` — if it documents `nest_group` / `unnest_group`, replace with `reparent_group`.
- `CHANGELOG.md` — add entry for the breaking change.

### Files created

- None (all changes are in-place — `ReparentGroupStep` lives alongside `ListSubgroupsStep` in `subgroup.py`).

### Files deleted

- None.

---

## Tasks

### Task 1: Bump calimero-client-py pin

**Files:**
- Modify: `pyproject.toml`

> Rationale: merobox depends on calimero-client-py at runtime. While developing, we need it to point at calimero-client-py's feature branch (or pre-release). After both core and calimero-client-py PRs merge, this can revert to the published master / next pinned release.

- [ ] **Step 1: Locate the pin**

```bash
grep -A2 "calimero-client-py" pyproject.toml
```

- [ ] **Step 2: Repoint to the calimero-client-py feature branch**

If the pin is `calimero-client-py @ git+...master`, change to point at the feature branch:

```toml
calimero-client-py @ git+https://github.com/calimero-network/calimero-client-py.git@feat/reparent-group-bindings
```

If the pin is a version (e.g. `calimero-client-py>=0.5.0`), temporarily switch to the git URL form for the development period.

- [ ] **Step 3: Reinstall**

```bash
pip install -e ".[dev]" 2>&1 | tail -10
```

Expected: clean install. If calimero-client-py's local checkout is already installed in dev mode, you can install from there instead: `pip install -e ../calimero-client-py`.

- [ ] **Step 4: Confirm `reparent_group` is callable**

```bash
python3 -c "from calimero_client_py import CalimeroClient; print(hasattr(CalimeroClient, 'reparent_group'), not hasattr(CalimeroClient, 'nest_group'))"
```

Expected: `True True`. If `False`, calimero-client-py wasn't built with the new bindings — re-run its `maturin develop` first.

- [ ] **Step 5: Commit**

```bash
git add pyproject.toml
git commit -m "chore: pin calimero-client-py feature branch during dev

Temporary pin during coordinated landing. Revert to published
release in the final commit (after calimero-client-py PR merges)."
```

---

### Task 2: Add `ReparentGroupStep` to subgroup.py

**Files:**
- Modify: `merobox/commands/bootstrap/steps/subgroup.py`
- Test: `merobox/tests/unit/test_group_steps.py`

- [ ] **Step 1: Write the failing tests**

Append to `merobox/tests/unit/test_group_steps.py` (use the existing `TestCreateNamespaceStep` style as a template):

```python
# At top of file, add the import:
# from merobox.commands.bootstrap.steps.subgroup import ReparentGroupStep


class TestReparentGroupStep:
    """Validation tests for ReparentGroupStep."""

    def setup_method(self):
        self.base_config = {
            "type": "reparent_group",
            "name": "Test Reparent",
            "node": "calimero-node-1",
            "child_group_id": "abcd1234",
            "new_parent_id": "ef567890",
        }

    def _make_step(self, config: dict) -> "ReparentGroupStep":
        from merobox.commands.bootstrap.steps.subgroup import ReparentGroupStep
        return ReparentGroupStep(config)

    def test_valid_config_passes_validation(self):
        self._make_step(self.base_config)

    def test_missing_node_raises(self):
        config = {**self.base_config}
        del config["node"]
        with pytest.raises(ValueError, match="node"):
            self._make_step(config)

    def test_missing_child_group_id_raises(self):
        config = {**self.base_config}
        del config["child_group_id"]
        with pytest.raises(ValueError, match="child_group_id"):
            self._make_step(config)

    def test_missing_new_parent_id_raises(self):
        config = {**self.base_config}
        del config["new_parent_id"]
        with pytest.raises(ValueError, match="new_parent_id"):
            self._make_step(config)

    def test_node_not_string_raises(self):
        config = {**self.base_config, "node": 123}
        with pytest.raises(ValueError, match="'node' must be a string"):
            self._make_step(config)

    def test_child_group_id_not_string_raises(self):
        config = {**self.base_config, "child_group_id": 123}
        with pytest.raises(ValueError, match="'child_group_id' must be a string"):
            self._make_step(config)

    def test_new_parent_id_not_string_raises(self):
        config = {**self.base_config, "new_parent_id": 456}
        with pytest.raises(ValueError, match="'new_parent_id' must be a string"):
            self._make_step(config)


class TestNestUnnestRemoved:
    """The old NestGroupStep / UnnestGroupStep classes must not exist."""

    def test_nest_group_step_removed(self):
        from merobox.commands.bootstrap.steps import subgroup
        assert not hasattr(subgroup, "NestGroupStep"), \
            "NestGroupStep should be removed in the strict-tree refactor"

    def test_unnest_group_step_removed(self):
        from merobox.commands.bootstrap.steps import subgroup
        assert not hasattr(subgroup, "UnnestGroupStep"), \
            "UnnestGroupStep should be removed in the strict-tree refactor"
```

- [ ] **Step 2: Run failing tests**

```bash
pytest merobox/tests/unit/test_group_steps.py::TestReparentGroupStep -v 2>&1 | tail -15
```

Expected: ImportError or AttributeError on `ReparentGroupStep` (doesn't exist yet).

- [ ] **Step 3: Delete `NestGroupStep` and `UnnestGroupStep`**

In `merobox/commands/bootstrap/steps/subgroup.py`, locate and delete the entire `class NestGroupStep(BaseStep):` block and the entire `class UnnestGroupStep(BaseStep):` block. Keep `ListSubgroupsStep` and any other classes intact.

- [ ] **Step 4: Add `ReparentGroupStep`**

After the imports in `subgroup.py`, add:

```python
class ReparentGroupStep(BaseStep):
    """Atomically move `child_group_id` to a new parent within the same namespace.

    Replaces the old NestGroupStep + UnnestGroupStep two-step pattern. The
    underlying RPC emits a single GroupReparented governance op; orphan
    state is structurally impossible.
    """

    def _get_required_fields(self) -> list[str]:
        return ["node", "child_group_id", "new_parent_id"]

    def _validate_field_types(self) -> None:
        step_name = self.config.get(
            "name", f'Unnamed {self.config.get("type", "Unknown")} step'
        )
        for field in ("node", "child_group_id", "new_parent_id"):
            if not isinstance(self.config.get(field), str):
                raise ValueError(f"Step '{step_name}': '{field}' must be a string")

    async def execute(
        self, workflow_results: dict[str, Any], dynamic_values: dict[str, Any]
    ) -> bool:
        node_name = self.config["node"]
        child_group_id = self._resolve_dynamic_value(
            self.config["child_group_id"], workflow_results, dynamic_values
        )
        new_parent_id = self._resolve_dynamic_value(
            self.config["new_parent_id"], workflow_results, dynamic_values
        )
        try:
            rpc_url, client_node_name = self._resolve_node_for_client(node_name)
            client = get_client_for_rpc_url(rpc_url, node_name=client_node_name)
            api_result = client.reparent_group(
                group_id=child_group_id,
                new_parent_id=new_parent_id,
            )
            result = ok(api_result)
        except Exception as e:
            result = fail("reparent_group failed", error=e)
        if result["success"]:
            if self._check_jsonrpc_error(result["data"]):
                return False
            workflow_results[f"reparent_group_{node_name}"] = result["data"]
            console.print(
                f"[green]✓ Reparented group {child_group_id} to {new_parent_id} on {node_name}[/green]"
            )
            return True
        console.print(
            f"[red]reparent_group failed on {node_name}: {result.get('error', 'Unknown error')}[/red]"
        )
        return False
```

- [ ] **Step 5: Run the tests**

```bash
pytest merobox/tests/unit/test_group_steps.py::TestReparentGroupStep merobox/tests/unit/test_group_steps.py::TestNestUnnestRemoved -v 2>&1 | tail -15
```

Expected: all 9 tests pass.

- [ ] **Step 6: Commit**

```bash
git add merobox/commands/bootstrap/steps/subgroup.py merobox/tests/unit/test_group_steps.py
git commit -m "feat: replace NestGroupStep/UnnestGroupStep with ReparentGroupStep

Drops the two old step classes. Adds a single ReparentGroupStep that
calls client.reparent_group(child, new_parent) — atomic edge swap.
Includes 7 validation tests + 2 absence-assertions on the old classes."
```

---

### Task 3: Update executor dispatch

**Files:**
- Modify: `merobox/commands/bootstrap/run/executor.py`
- Test: `merobox/tests/unit/test_group_steps.py` (extend with dispatch tests)

- [ ] **Step 1: Write the failing tests**

Append to `merobox/tests/unit/test_group_steps.py`:

```python
class TestExecutorDispatch:
    """Verify the executor dispatch table reflects the strict-tree refactor."""

    def test_reparent_group_step_dispatched(self):
        from merobox.commands.bootstrap.run.executor import WorkflowExecutor
        # Construct a minimal config so we can call _create_step_executor.
        executor = WorkflowExecutor.__new__(WorkflowExecutor)
        step = executor._create_step_executor(
            "reparent_group",
            {
                "type": "reparent_group",
                "name": "test",
                "node": "n1",
                "child_group_id": "abc",
                "new_parent_id": "def",
            },
        )
        from merobox.commands.bootstrap.steps.subgroup import ReparentGroupStep
        assert isinstance(step, ReparentGroupStep)

    def test_nest_group_step_type_unknown(self, capsys):
        from merobox.commands.bootstrap.run.executor import WorkflowExecutor
        executor = WorkflowExecutor.__new__(WorkflowExecutor)
        # Should not produce a step instance — should print "Unknown step type"
        # and return None (matches existing pattern at executor.py:1027).
        result = executor._create_step_executor("nest_group", {})
        assert result is None

    def test_unnest_group_step_type_unknown(self, capsys):
        from merobox.commands.bootstrap.run.executor import WorkflowExecutor
        executor = WorkflowExecutor.__new__(WorkflowExecutor)
        result = executor._create_step_executor("unnest_group", {})
        assert result is None
```

If `_create_step_executor` requires more setup than `__new__`, follow whatever existing tests do — check `test_dry_run.py` for executor-construction patterns.

- [ ] **Step 2: Run failing tests**

```bash
pytest merobox/tests/unit/test_group_steps.py::TestExecutorDispatch -v 2>&1 | tail -10
```

Expected: failure on `test_reparent_group_step_dispatched`.

- [ ] **Step 3: Update the dispatch**

Open `merobox/commands/bootstrap/run/executor.py`. Find lines ~1430-1432:

```python
elif step_type == "nest_group":
    return NestGroupStep(step_config, **common_kwargs)
elif step_type == "unnest_group":
    return UnnestGroupStep(step_config, **common_kwargs)
```

Replace with:

```python
elif step_type == "reparent_group":
    return ReparentGroupStep(step_config, **common_kwargs)
```

Update the import at the top of the file:

```python
# was:
from merobox.commands.bootstrap.steps.subgroup import (
    NestGroupStep,
    UnnestGroupStep,
    ListSubgroupsStep,
)
# becomes:
from merobox.commands.bootstrap.steps.subgroup import (
    ReparentGroupStep,
    ListSubgroupsStep,
)
```

(Adjust based on actual import shape.)

- [ ] **Step 4: Run the tests**

```bash
pytest merobox/tests/unit/test_group_steps.py::TestExecutorDispatch -v 2>&1 | tail -10
```

Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add merobox/commands/bootstrap/run/executor.py merobox/tests/unit/test_group_steps.py
git commit -m "feat: dispatch reparent_group, drop nest/unnest dispatch arms

Updates the executor dispatch table to recognize 'reparent_group'.
Old 'nest_group' and 'unnest_group' types now fall through to
'Unknown step type' (returns None) — workflow validation should
catch them before they reach the executor."
```

---

### Task 4: Update validator and pydantic config schema

**Files:**
- Modify: `merobox/commands/bootstrap/validate/validator.py`
- Modify: `merobox/commands/bootstrap/config.py`
- Test: `merobox/tests/unit/test_group_steps.py` (validator tests)

- [ ] **Step 1: Find the validator entries**

```bash
grep -B1 -A2 "nest_group\|unnest_group" merobox/commands/bootstrap/validate/validator.py
```

- [ ] **Step 2: Update the validator dispatch**

In `merobox/commands/bootstrap/validate/validator.py`, find the lines around 190 that handle `nest_group` / `unnest_group`:

```python
elif step_type == "nest_group":
    NestGroupStep(step)
elif step_type == "unnest_group":
    UnnestGroupStep(step)
```

Replace with:

```python
elif step_type == "reparent_group":
    ReparentGroupStep(step)
```

Update imports at the top of the validator file accordingly.

- [ ] **Step 3: Update pydantic schemas in config.py**

In `merobox/commands/bootstrap/config.py`, find:

```python
"nest_group",
"unnest_group",
```

(in the SUPPORTED_STEP_TYPES list around line 40-41). Replace with `"reparent_group"`.

Find the `NestGroupStepConfig` and `UnnestGroupStepConfig` pydantic models around line 450-460. Delete both. Add:

```python
class ReparentGroupStepConfig(BaseStepConfig):
    """Configuration for reparent_group step."""

    type: Literal["reparent_group"] = "reparent_group"
    node: str
    child_group_id: str
    new_parent_id: str
```

Find the dispatch table around line 540 (`STEP_CONFIG_MAP` or similar):

```python
"nest_group": NestGroupStepConfig,
"unnest_group": UnnestGroupStepConfig,
```

Replace with:

```python
"reparent_group": ReparentGroupStepConfig,
```

- [ ] **Step 4: Add a config-schema test**

Append to `merobox/tests/unit/test_group_steps.py`:

```python
class TestReparentGroupStepConfigSchema:
    def test_pydantic_schema_accepts_valid(self):
        from merobox.commands.bootstrap.config import ReparentGroupStepConfig
        cfg = ReparentGroupStepConfig(
            name="test",
            node="n1",
            child_group_id="abc",
            new_parent_id="def",
        )
        assert cfg.type == "reparent_group"

    def test_pydantic_schema_rejects_missing_new_parent_id(self):
        from merobox.commands.bootstrap.config import ReparentGroupStepConfig
        from pydantic import ValidationError
        with pytest.raises(ValidationError):
            ReparentGroupStepConfig(
                name="test",
                node="n1",
                child_group_id="abc",
            )

    def test_old_nest_group_step_config_removed(self):
        from merobox.commands.bootstrap import config
        assert not hasattr(config, "NestGroupStepConfig")

    def test_old_unnest_group_step_config_removed(self):
        from merobox.commands.bootstrap import config
        assert not hasattr(config, "UnnestGroupStepConfig")
```

If the existing `BaseStepConfig` doesn't expect a `name` field, drop it from the test inputs.

- [ ] **Step 5: Run all the new tests**

```bash
pytest merobox/tests/unit/test_group_steps.py -v 2>&1 | tail -20
```

Expected: all pass.

- [ ] **Step 6: Run the full test suite to catch regressions**

```bash
pytest merobox/tests/unit/ -v 2>&1 | tail -20
```

Expected: all pass. If any pre-existing test references `nest_group` / `unnest_group` (e.g. in `test_dry_run.py`), update or delete it.

- [ ] **Step 7: Commit**

```bash
git add merobox/commands/bootstrap/validate/validator.py merobox/commands/bootstrap/config.py merobox/tests/unit/test_group_steps.py
git commit -m "feat: validator + pydantic schema for reparent_group

Drops NestGroupStepConfig / UnnestGroupStepConfig pydantic models and
their entries in SUPPORTED_STEP_TYPES + STEP_CONFIG_MAP. Adds
ReparentGroupStepConfig with the same field shape as the runtime
class. Validator dispatch updated to match."
```

---

### Task 5: Update docs (LLM.md, README, CHANGELOG)

**Files:**
- Modify: `LLM.md`
- Modify: `README.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Update LLM.md**

```bash
grep -n "nest_group\|unnest_group" LLM.md
```

For each occurrence, either replace with `reparent_group` documentation or remove if it was specifically about the old two-step pattern.

- [ ] **Step 2: Update README.md**

Same as LLM.md. The reparent_group example should look like:

```yaml
- name: Move folder under new parent
  type: reparent_group
  node: calimero-node-1
  child_group_id: '{{folder_id}}'
  new_parent_id: '{{new_parent_id}}'
```

- [ ] **Step 3: Add CHANGELOG entry**

Prepend to `CHANGELOG.md`:

```markdown
## [Unreleased]

### Breaking changes

- Removed `nest_group` and `unnest_group` workflow step types. Use
  `reparent_group` (atomic edge swap) instead. See migration guide
  below.

### Added

- `reparent_group` step type for atomically moving a group under a
  new parent. Required fields: `node`, `child_group_id`,
  `new_parent_id`.

### Migration

Replace:

```yaml
- type: unnest_group
  parent_group_id: '{{old_parent}}'
  child_group_id: '{{child}}'
- type: nest_group
  parent_group_id: '{{new_parent}}'
  child_group_id: '{{child}}'
```

With:

```yaml
- type: reparent_group
  child_group_id: '{{child}}'
  new_parent_id: '{{new_parent}}'
```

Note that `delete_group` now cascades — it will delete the target
group, its descendants, AND all contexts in the subtree. To preserve
a context before deletion, use `detach_context_from_group` first.
```

- [ ] **Step 4: Commit**

```bash
git add LLM.md README.md CHANGELOG.md
git commit -m "docs: document reparent_group, migration guide, CHANGELOG entry

Removes mentions of nest_group / unnest_group from LLM.md and README.
Adds CHANGELOG entry covering the breaking change and a migration
example for users with workflows that used the old pattern."
```

---

### Task 6: Pre-commit + restore production pin

- [ ] **Step 1: Run pre-commit hooks**

```bash
pre-commit run --all-files 2>&1 | tail -15
```

Expected: pass. Fix any reported issues (likely formatting only).

- [ ] **Step 2: Run the full test suite one more time**

```bash
pytest 2>&1 | tail -10
```

Expected: all pass.

- [ ] **Step 3: Wait for calimero-client-py PR to merge** (coordination point)

```bash
gh pr view <client-py-pr-number> --repo calimero-network/calimero-client-py | head -5
```

Expected: state `MERGED`.

- [ ] **Step 4: Revert pyproject.toml pin**

If Task 1 changed the calimero-client-py pin to a feature branch, revert it back to the production form (master git pin or version range).

- [ ] **Step 5: Commit and push**

```bash
git add pyproject.toml
git commit -m "chore: revert calimero-client-py pin to master after PR merge"
git push -u origin feat/reparent-group-step-type
```

- [ ] **Step 6: Open the PR**

```bash
gh pr create --repo calimero-network/merobox --base master \
  --title "feat: reparent_group step type, drop nest_group/unnest_group" \
  --body "$(cat <<'EOF'
## Summary

Mirrors the strict group-tree refactor in core (calimero-network/core PR #<num>)
and calimero-client-py (calimero-network/calimero-client-py PR #<num>):

- Drops \`nest_group\` and \`unnest_group\` workflow step types
- Adds \`reparent_group\` step type (atomic edge swap)
- Pydantic schemas, validator, and executor dispatch updated to match
- 12 new unit tests covering validation, dispatch, and config schema
- CHANGELOG migration guide for users with workflows that used the old pattern

Spec: see core repo at \`docs/superpowers/specs/2026-04-22-strict-group-tree-and-cascade-delete.md\`.

## Coordination

This is PR 3 of 3:
1. core PR (merged)
2. calimero-client-py PR (merged)
3. **THIS PR**

Once this lands, core's \`group-reparent.yml\` and \`group-reparent-and-cascade-delete.yml\`
E2E workflows will pass.

## Test plan

- [x] Unit tests: validation, dispatch, pydantic schema, presence/absence (12 tests)
- [x] Full pytest suite passes
- [x] pre-commit hooks pass
- [ ] Manual smoke: run a simple workflow with reparent_group locally against a merod node
EOF
)"
```

---

## Self-review checklist

- Spec § 8.4 (merobox follow-up) covered by all tasks.
- All `nest_group` / `unnest_group` references removed from steps, executor, validator, config, tests, docs.
- New `reparent_group` step has unit tests for: validation (Task 2), dispatch (Task 3), pydantic schema (Task 4).
- Pin coordination explicit (Tasks 1 + 6).
- CHANGELOG includes migration guide so existing users can update their YAMLs.
- No "TBD" / "TODO" placeholders.

---

**Done.** This is the final plan in the 3-PR sequence. After this lands, the architectural change is complete: orphan groups are structurally impossible across all three layers (core ops, Python SDK, test harness), and core's E2E CI workflows pass.
