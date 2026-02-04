# Run Tests and Fix

Run all tests and fix any failures. Also runs format and clippy.

**Instructions:**

1. Run: `cargo fmt --check`
   - If fails: run `cargo fmt` and fix formatting
2. Run: `cargo clippy --workspace -- -A warnings`
   - Fix any clippy warnings (avoid `#[allow(...)]` unless justified)
3. Run: `cargo test`

   - For each failing test: analyze the failure, fix the code, re-run
   - Use `cargo test -p <crate>` to run tests for a specific crate
   - Use `cargo test -- --nocapture` to see stdout

4. Report when all checks pass. If fixes were made, briefly summarize changes.
