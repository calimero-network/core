# TEE Dice

A dice roll no player can rig: the smallest end-to-end use of **TEE
authorship** (see `docs/src/content/docs/protocol/tee-authorship.mdx`).

A member calls `roll(roll_id, sides)`. That writes nothing the game trusts; it
emits `RollRequested` with the TEE handler `tee:resolve_roll`. The namespace's
elected TEE authority runs `resolve_roll` inside the enclave, draws the face
from `env::tee_random_bytes`, and records it in `TeeOnly` state. Every peer
accepts that write because its signer resolves to `AccountId::TEE_AUTHORITY`,
and drops a member's attempt to write the same cell.

## Methods

- `roll(roll_id, sides)` — ask the TEE for a roll (any member)
- `resolve_roll(roll_id, sides)` — `#[app::tee]`: run only by the TEE scheduler
- `get_roll(roll_id)` — the face, once resolved (anyone)
- `is_resolved(roll_id)` — whether the TEE has resolved it (anyone)

## Building

```bash
cargo mero build
```

Produces `res/tee_dice.wasm`.

## End to end

`workflows/tee-dice-roll.yml` admits a mock-TEE replica, shows a roll stays
unresolved while the namespace has no TEE authoring policy, then names the mock
MRTD in the policy and shows the next roll resolved on the player's node. It
needs a `merod` built with `--features mock-attestation`; the header of the
workflow has the local command.
