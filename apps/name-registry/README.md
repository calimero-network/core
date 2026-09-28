# Name Registry

Usernames no member can take from another: the smallest end-to-end use of a
`Registry` (see `crates/storage/src/collections/registry.rs`), decided by the
namespace's attested TEE authority.

A member calls `claim(name, display)`. That writes the member's own claim,
which decides nothing, and emits `NameClaimed` with the TEE handler
`tee:resolve_name`. The elected TEE runs `resolve_name` inside the enclave and
writes the verdict into `TeeOnly` state. The first claim to reach the TEE wins;
claims it sees together are split by an order derived from the name, the epoch
and the claimant, never from a timestamp, so nobody can backdate a claim to
take a name. Every peer accepts the verdict because its signer resolves to
`AccountId::TEE_AUTHORITY`, and drops any member's attempt to write one.

## Methods

- `claim(name, display)` — ask for a username (any member)
- `let_go(name)` — free a name you own, or withdraw a claim you made
- `resolve_name(name)` — `#[app::tee]`: run only by the TEE scheduler
- `sweep()` — `#[app::tee(every = "1m")]`: resolves what a missed trigger left
- `status(name)` — `free`, `pending`, `owned` or `lost`, as the caller sees it
- `owner_of(name)` — the owner, once the TEE has granted the name
- `display_of(name)`, `my_pending()`, `me()`

Gate every use of a name (a mention, a URL, a payment) on `owner_of`, never on
having claimed it.

## Building

```bash
cargo mero build
```

Produces `res/name_registry.wasm`.

## End to end

`workflows/name-registry-contested.yml` admits a mock-TEE replica, shows a claim
stays pending while the namespace has no TEE authoring policy, then has two
members on two nodes claim `rose`. Both nodes must name the same owner and
disagree only on who lost; after the owner lets it go, the other member claims
it at the next epoch. It needs a `merod` built with `--features
mock-attestation`; the header of the workflow has the local command.
