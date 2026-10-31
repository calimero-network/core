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
> scenarios here run with mock attestation: the cluster's nodes report mock
> quotes and the cluster is every app's TEE, so no admission or authoring policy
> is set (v1's workflows set `acceptMock` and the all-zero measurements a mock
> quote reports). Production needs a real attestation policy: `acceptMock` false,
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

This is a demo of TEE authorship, not a game to deploy as is: it does not bind
a roll to its requester or its die, and if two TEE authorities resolve the same
roll at once, the last write wins and a face already read can change.

## Building

```bash
cargo mero build
```

Produces `res/tee_dice.wasm`.

## End to end

`scenarios/cluster/tee-dice-roll.json` has a member ask for a roll and checks
that the cluster's TEE resolves it into `TeeOnly` state the member's node
reads. v1's first check (a roll staying unresolved until an authoring policy
named the enclave) has no counterpart, since a cluster is the TEE from its
start; the file's `comment` says so.

```bash
cargo test -p calimero-node --test scenarios -- apps/tee-dice/
```
