---
name: core-pr-helper
description: Suggests one appropriate branch name and a PR description in the project template (title, Description, Test plan, Documentation update) for Calimero core. Use when creating a PR for core, or when the user asks for a branch name and PR description.
---

# Core PR Helper

When the user asks for a branch name and PR description for core:

1. Infer the change from the conversation (recent edits, stated scope, or ask briefly if unclear).
2. Output **one** branch name, then the PR body in the exact format below.

## Branch name

- **One** name only. Use lowercase, hyphens; no slashes.
- Pattern: `{type}-{scope}-{short-summary}` (e.g. `feat-meroctl-install-flow`, `fix-runtime-wasm-imports`).
- Types (match repo commit types): `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `perf`, `build`, `ci`, `style`, `revert`.
- Scope: crate or area (e.g. `meroctl`, `runtime`, `sdk`, `server`, `network`, `storage`). Omit scope if change is cross-cutting.

## PR description format

Use this structure exactly. Fill each section from the inferred change; keep placeholders only where the user must fill in (e.g. issue number, links).

```markdown
# title

Short imperative title for the PR (e.g. "Add install flow to meroctl" or "Fix WASM import resolution in runtime").

## Description

Brief description of the change and which issue is fixed (if any). Include context and any dependency or prerequisite changes.

**Motivation**: Why this change is needed.

## Test plan

What was run to verify (e.g. `cargo test -p crate-name`, manual steps). Whether e2e or new tests were added. For UI changes, mention screenshots or videos if applicable.

## Documentation update

Which public or internal docs (if any) need updates. If none, state "None" or "N/A". Note: documentation must be updated no later than one day after merge.
```

## Output

1. **Branch:** `single-branch-name`
2. **PR description:** (the filled template as above)

Do not offer multiple branch names; choose the single most appropriate one.
