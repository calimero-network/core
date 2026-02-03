# Pre-PR Check

Run the Definition of Done checks before creating a PR. See `AGENTS.md` for full checklist.

**Instructions:**

1. Run these commands in order and report results:
   ```bash
   cargo fmt --check
   cargo clippy --workspace -- -A warnings
   cargo test
   cargo deny check licenses sources   # only if dependencies were modified
   ```
2. For any failure, fix the issues and re-run until all pass
3. Remind that documentation must be updated (README, AGENTS.md, crate docs) if the change warrants it

**Definition of Done (from AGENTS.md):**

1. `cargo fmt --check` passes
2. `cargo clippy -- -A warnings` passes
3. `cargo test` passes
4. `cargo deny check licenses sources` passes (if modifying dependencies)
5. Update relevant documentation at the end of changes
