# Dead Code Cleanup

Detect and remove dead code. Use the **dead-code-cleanup** skill for the full workflow.

**Instructions:**

1. Read and follow the **dead-code-cleanup** skill: `.cursor/skills/dead-code-cleanup/SKILL.md`
2. Identify dead code candidates (unused functions, variables, imports, types)
3. Verify no references anywhere before suggesting removal
4. Produce the structured report (Confirmed dead / Excluded with reasons)
5. Do not remove anything until the user explicitly confirms

**Quick verification for Rust:**

```bash
cargo clippy --workspace -- -W dead_code -W unused_imports -W unused_variables
```

**Calimero rule (from AGENTS.md):** All code in PRs must be used. No dead code, no commented-out blocks. Use `#[allow(dead_code)]` only with a comment explaining why.
