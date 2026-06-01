# calimero-abi

CLI for working with the Calimero WASM ABI (`abi.json` / `state-schema.json`).

## Subcommands

| Command | Purpose |
|---|---|
| `extract <wasm>` | Extract the full ABI from a built app. |
| `types <wasm>` | Extract just the types schema. |
| `state <wasm>` | Extract the state schema (state root + its type dependencies). |
| `inspect <wasm>` | List the wasm's custom sections. |
| `diff <current> <baseline>` | **Compare two `state-schema.json` versions and flag changes that require (or forbid) a migration.** |

## `calimero-abi diff` — migration safety lint

Compares the **current** build's state schema against a **baseline** (the
previous version) and classifies every top-level state-field change. It is the
CI (L2) layer of the migration safety rail: it tells a developer *before* shipping
whether a schema change needs a migration, and refuses the one change that
silently destroys user data — stripping ownership/authorship from an
identity-gated CRDT.

```bash
calimero-abi diff res/state-schema.json ../v1/state-schema.json
```

### Finding classes

| Class | Meaning | Exit |
|---|---|---|
| `ADDITIVE` | A new field an old state can default-fill. No migration needed. | does not fail |
| `BREAKING` | A field's type changed, or a field was removed. A migration is required. | exit 1 |
| `UNSAFE_IDENTITY_DOWNGRADE` | An identity-gated CRDT (`AuthoredMap`, `AuthoredVector`, `SharedStorage`) was replaced by a non-identity-gated type, or dropped. This **silently strips per-entry authorship / the writer-ACL network-wide.** | exit 1 |

Example output for an unsafe downgrade:

```
⛔ [UNSAFE_IDENTITY_DOWNGRADE] field 'wiki' AuthoredMap → UnorderedMap — strips authorship / writer-ACL network-wide
    override requires #[migrate(unsafe_strip_identity = "…")] + governance allowance (see #2534)
```

### Exit codes

- `0` — no changes, or only `ADDITIVE` changes.
- `1` — at least one `BREAKING` or `UNSAFE_IDENTITY_DOWNGRADE` change (or a bad
  input: missing/non-record state root, a dangling/cyclic `$ref`, a duplicate
  field — the tool **fails closed** on a corrupt schema rather than silently
  passing).
- `--exit-zero` reports findings but always exits `0` (for report-only use).

### Design notes

- **Canonical comparison.** Each field's type is fully `$ref`/alias-expanded
  before comparison, so an aliased type compares equal to the same type written
  inline, and a change hidden behind a stable `$ref` name (the referenced type
  mutating) is still detected.
- **Fail-closed.** A corrupt or unresolvable schema is an error, never a silent
  "no findings" — a security lint must not pass a downgrade it failed to parse.
- **Top-level scope.** Identity-gating is checked on the top-level type of each
  state field. An identity-gated CRDT nested *inside* a `Record`/`Variant` field
  is not currently inspected (would need a recursive walk).
- The identity classification reuses the authoritative `collection_category`
  classifier from `calimero-wasm-abi`.

### Intended CI use (follow-up)

Wiring `diff` to run automatically against a real app's two builds — failing CI
on a `BREAKING`-without-migration or an `UNSAFE_IDENTITY_DOWNGRADE`-without-override
— is a follow-up. It needs a per-app *baseline* (the previous version's schema)
and the `unsafe_strip_identity` override to exist (tracked with the
`#[derive(Migrate)]` work). Today the tool is a standalone command; the unit and
end-to-end tests in `src/diff.rs` and `tests/diff_cli.rs` validate its behaviour.
