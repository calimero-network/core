---
name: dead-code-cleanup
description: Detects and removes dead code introduced by AI agents. Analyzes codebase for unused functions, variables, imports, and types; verifies no references (including indirect, dynamic, or runtime); reports with file paths and line numbers; optionally generates safe patches. Use when cleaning up after AI-generated code, removing unused code, or when the user asks for dead code detection, unused code removal, or code quality cleanup.
---

# Dead Code Cleanup

Remove unused code introduced during development while avoiding functional regressions. Always **verify** that code is truly unused before suggesting removal.

## Workflow overview

1. **Identify** candidates (unused functions, variables, imports, types).
2. **Verify** no references anywhere (direct, indirect, dynamic, runtime).
3. **Exclude** items that match the exclusion rules below.
4. **Report** confirmed dead code with file paths and line numbers.
5. **Remove** only when user confirms; optionally produce a patch or PR.

---

## 1. Identifying dead code candidates

### Rust (this workspace)

- Run: `cargo clippy --workspace -- -W dead_code -W unused_imports -W unused_variables`
- Grep for `#[allow(dead_code)]` and assess whether the allowance is still justified.
- Search for unused modules: symbols exported but never referenced (use LSP "find references" or `rg "symbol_name" crates/`).

### General

- Use language linters and "unused" diagnostics (e.g. `dead_code`, `unused_imports`).
- Use LSP "find references" for each candidate symbol across the whole repo.
- Grep for the symbol name (exact and with word boundaries) in source, configs, and tests.

---

## 2. Verification (required before removal)

Do **not** treat something as dead until all of the following are checked:

| Check                   | How                                                                                                                   |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------- |
| Direct references       | LSP "find references" or grep for symbol in all source and test files.                                                |
| Re-exports              | Symbol re-exported from a `pub use` or public module? If yes, treat as used unless the re-export is also dead.        |
| Dynamic / runtime       | Used via reflection, config-driven loading, plugin names, or string-based dispatch? If yes, **exclude** from removal. |
| Conditional compilation | Behind `#[cfg(...)]` / `cfg!` / build flags? Only remove if the user confirms that config is obsolete.                |
| Public API              | Exposed in a public crate API (e.g. `pub fn` in a library)? **Exclude** unless user explicitly confirms removal.      |
| Tests / examples        | Referenced only in tests or examples? Still "used"; do not remove.                                                    |
| FFI / C ABI             | Used from C or other languages? **Exclude** unless confirmed.                                                         |

If any reference or usage is found, **do not** suggest removal. Only suggest removal when there are zero references and the item is not in the exclusion list.

---

## 3. Exclusions (do not remove unless user confirms)

- **Public API surface**: `pub` items in library crates that external code might depend on.
- **Reflection / config-based loading**: Types or names loaded by name (e.g. from config or plugin registry).
- **Conditional builds**: Code under `#[cfg(...)]` or build-system–controlled features; treat as used unless user says the config is unused.
- **Test / bench / example only**: Used only in `tests/`, `examples/`, or `#[cfg(test)]`; keep unless user asks to remove tests/examples too.
- **FFI / ABI**: Exported for C or other runtimes.
- **Documentation or macros**: Items referenced only in doc comments or macro expansions; verify macro expansion before removing.

When in doubt, **include the item in the report as "excluded"** with a short reason, and do not remove it.

---

## 4. Report format

Produce a clear, machine-friendly report. Use this structure:

```markdown
# Dead code report

## Confirmed dead (safe to remove)

| File             | Line(s) | Kind     | Symbol                  |
| ---------------- | ------- | -------- | ----------------------- |
| path/to/file.rs  | 42–45   | function | `helper_foo`            |
| path/to/other.rs | 1       | import   | `use crate::UnusedType` |

## Excluded (do not remove without confirmation)

| File                  | Line(s) | Symbol          | Reason     |
| --------------------- | ------- | --------------- | ---------- |
| crates/sdk/src/lib.rs | 100     | `public_api_fn` | Public API |

## Verification summary

- [ ] All confirmed-dead items: references checked (0 found).
- [ ] Excluded items listed with reasons.
```

Always include file paths and line numbers (or line ranges) for every listed item.

---

## 5. Safe removal and patches

- **Do not delete** any code until the user explicitly agrees to remove the reported items (or a subset).
- When generating a patch or PR:
  - Remove only items listed under "Confirmed dead".
  - Do not remove or change excluded items.
  - After edits: run `cargo check --workspace` and `cargo test` (for Rust); suggest equivalent checks for other languages.
- Commit message (for this repo): follow `<type>(<scope>): <summary>` (e.g. `refactor(sdk): remove dead code reported by dead-code-cleanup`).

---

## Checklist before suggesting removal

- [ ] Every candidate was checked with "find references" or equivalent.
- [ ] No references in tests, examples, docs, or re-exports.
- [ ] No dynamic / reflection / config-based use.
- [ ] Public API and conditional-build items excluded or explicitly confirmed by user.
- [ ] Report lists file paths and line numbers for all entries.
- [ ] Patch/PR only removes "Confirmed dead" items and leaves "Excluded" items unchanged.
