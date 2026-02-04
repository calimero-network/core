# Code Review Checklist

Comprehensive checklist for Calimero core PR reviews. Align with AGENTS.md conventions.

## Functionality

- [ ] Code does what it's supposed to do
- [ ] Edge cases are handled
- [ ] Error handling uses `eyre` and `?` (no `.unwrap()` / `.expect()` without SAFETY comment)
- [ ] No obvious bugs or logic errors

## Code Quality

- [ ] Code is readable and well-structured
- [ ] Imports follow StdExternalCrate pattern (std → external → crate → local)
- [ ] No `mod.rs` – modules use named files
- [ ] Functions are small and focused
- [ ] Variable names are descriptive
- [ ] No code duplication

## Calimero Conventions (AGENTS.md)

- [ ] **No dead code** – all functions, variables, imports, types are used
- [ ] No commented-out code blocks
- [ ] Commit format: `<type>(<scope>): <summary>` (e.g. `feat(runtime): add wasm bounds check`)
- [ ] Relevant documentation updated (README, AGENTS.md, crate docs)

## Tests

- [ ] Unit tests in `#[cfg(test)]` modules or `tests/` directory
- [ ] Async tests use `#[tokio::test]`
- [ ] Tests pass: `cargo test -p <crate>`

## Definition of Done

- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -A warnings` passes
- [ ] `cargo test` passes
- [ ] PR has Test plan and Documentation update sections filled
