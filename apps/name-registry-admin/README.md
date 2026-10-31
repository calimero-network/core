# Name Registry (Admin)

Usernames no member can take from another, decided by the registry's admins, on
the component SDK (`calimero-app-sdk`). The v1 version was a
`Registry<String, String, Admin>`; here it is three tables (`claims`,
`verdicts`, `admins`) and explicit checks against the caller.

A member calls `claim(name, display)`. That writes the member's own claim, which
decides nothing, and emits `NameClaimed`. An admin calls `resolve(name)`: a
release by the owner is answered with a vacancy at the next epoch, then the open
epoch goes to the first live claim in the order the app's log applied them.

## Who the admins are

The app owner is the first and only admin. The set changes only through
`setAdmins(accounts)`, which only an admin may call and which cannot empty the
set. An admin who hands the role on and does not name itself can no longer
resolve, and cannot take the role back.

## Methods

- `claim(name, display)`: ask for a username (any member)
- `letGo(name)`: free a name you own, or withdraw a pending claim; a lost claim is kept
- `rebid(name)`: once a name is released, renew the claim you lost it with
- `resolve(name)`: decide a name (admins only)
- `setAdmins(accounts)`, `admins()`
- `status(name)`: `free`, `pending`, `owned` or `lost` as the caller sees it
- `claimTally(name)`, `ownerOf(name)`, `displayOf(name)`, `me()`

Gate every use of a name on `ownerOf`, never on having claimed it.

```bash
cargo test --manifest-path apps/name-registry-admin/Cargo.toml
```

`tests/scenarios.rs` runs `scenarios/name-registry-admin.json` (the former
merobox workflow: two claims and one owner, release and the next epoch, a member
refused, the role handed on) in the harness; the cluster runs the same file.
