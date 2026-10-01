# TEE Dice

A dice roll no player can rig: the smallest end-to-end use of **TEE
authorship** (see `docs/src/content/docs/protocol/tee-authorship.mdx`).

A member calls `roll(roll_id, sides)`. That writes nothing the game trusts; it
emits `RollRequested` with the TEE handler `tee:resolve_roll`. The namespace's
elected TEE authority runs `resolve_roll` inside the enclave, draws the face
from `env::tee_random_bytes`, and records it in `TeeOnly` state. Every peer
accepts that write because its signer resolves to `AccountId::TEE_AUTHORITY`,
and drops a member's attempt to write the same cell.

> **Mock attestation.** The roll is unrigged only if the enclave is genuine. The
> workflows here run with mock attestation: the namespace's TEE admission policy
> sets `acceptMock` (`accept_mock` in the merobox step) and lists the all-zero
> measurements a mock quote reports, and the authoring policy's `allowedMrtd`
> does the same. Production needs a real attestation policy: `acceptMock` false,
> `allowedMrtd` and `allowedRtmr1` to `allowedRtmr3` naming the approved image
> (or `signedRelease`), the real MRTD in the authoring policy, and a `merod`
> that does not run with `--mock-tee`. See
> `docs/src/content/docs/protocol/tee-attestation.mdx`.

## Methods

- `roll(roll_id, sides)` — ask the TEE for a roll (any member)
- `resolve_roll(roll_id, sides)` — `#[app::tee]`: run only by the TEE scheduler
- `get_roll(roll_id)` — the face, once resolved (anyone)
- `is_resolved(roll_id)` — whether the TEE has resolved it (anyone)

## What the TEE cannot tell

The enclave decides the face, and nobody else can write it. It does not know
who asked: a `tee:` handler runs on the arguments of an event, and any member's
node can emit `RollRequested` for any `roll_id` with any `sides`. So a roll id
is shared by the whole context, and the first request the TEE handles for it
fixes its `sides` and its face; later requests for the id are ignored.

A game built on this should derive roll ids nobody else can predict before the
roll is needed (a turn's id plus a nonce the roller commits to, say), and read
`sides` from its own rules rather than from whichever request won. Binding a
roll to its requester takes a request record in owned state (an `Authored` or
`WriteOnce` entry) that the TEE handler reads, instead of trusting the event.

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
