# Name Registry (Admin)

Usernames no member can take from another, decided by the context's admins:
a `Registry<String, String, Admin>` (see
`crates/storage/src/collections/registry.rs`). It is `apps/name-registry` with
the `Admin` authority in place of the TEE, so it needs no enclave and no
mock attestation.

A member calls `claim(name, display)`. That writes the member's own claim,
which decides nothing, and emits `NameClaimed` so an admin's client knows there
is something to decide. An admin calls `resolve(name)`, which writes the verdict
into a `SharedStorage` cell whose writers are the admins. Claims the admin sees
together are split by an order derived from the name, the epoch and the
claimant, never from a timestamp. Every peer checks a verdict against the admins
as of that verdict and drops one from anybody else.

## Who the admins are

The account that ran `init`, the context's creator, is the first and only
admin. The set changes only through `set_admins(accounts)`, which only an admin
may call and which every node verifies as a writer-set rotation. An admin who
hands the role on and does not name itself can no longer resolve, and cannot
take the role back.

## Methods

- `claim(name, display)` — ask for a username (any member)
- `let_go(name)` — free a name you own, or withdraw a pending claim; a claim
  that lost is kept
- `rebid(name)` — once a name is released, renew the claim you lost it with
- `resolve(name)` — decide a name (admins only); answers a release with a
  vacancy at the next epoch, then grants the open epoch
- `set_admins(accounts)` — replace the admins (admins only)
- `admins()` — the accounts that may resolve
- `status(name)` — `free`, `pending`, `owned` or `lost` as the caller sees it,
  with the owner and the epoch it holds the name at
- `owner_of(name)` — the owner, once an admin has granted the name
- `display_of(name)`, `me()`

Gate every use of a name (a mention, a URL, a payment) on `owner_of`, never on
having claimed it.

## Building

```bash
cargo mero build
```

Produces `res/name_registry_admin.wasm`.

## End to end

`workflows/name-registry-admin.yml` runs on three nodes: the owner node, whose
account is the admin, and two members. Both members claim `rose` at once; the
admin resolves once it holds both claims, and every node names the same owner,
one of the two, at epoch 0. The owner lets it go, the admin answers with a
vacancy, the other member rebids, and every node names that member at epoch 1.
A member's `resolve` and `set_admins` are refused and change nothing on any
node. The admin then hands the role to one member, whose `resolve` every node
accepts, while the old admin's `resolve` and `set_admins` are refused.

```bash
cargo build -p merod --release   # into a merod:local image, as CI does
merobox bootstrap run workflows/name-registry-admin.yml --image merod:local --e2e-mode
```
