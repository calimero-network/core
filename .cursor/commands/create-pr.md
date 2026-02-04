# Create PR

Generate a branch name and PR description for Calimero core.

**Instructions:**

1. Read and follow the **core-pr-helper** skill: `.cursor/skills/core-pr-helper/SKILL.md`
2. Infer the change from the conversation (recent edits, stated scope, or ask briefly if unclear)
3. Output **one** branch name, then the PR body in the exact template format

If the user provided additional context after the command (e.g. `/create-pr fix the runtime bug`), use that to infer the change.

**Output format:**

1. **Branch:** `single-branch-name`
2. **PR description:** (filled template with title, Description, Test plan, Documentation update)
