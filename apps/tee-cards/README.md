# TEE Cards

Hidden cards no player can peek at or stack: TEE authorship with secrets (see
`docs/src/content/docs/protocol/tee-authorship.mdx`).

- **Seats** (`UserStorage`): who is playing. Each player seats themselves, and
  nobody can seat another.
- **The deck** (`TeeSecret<Vec<u8>>`): sealed to every TEE authority, so members
  replicate it and cannot read it.
- **Hands** (`TeeOnly`): each card sealed to every device of its player's
  account (`Sealed::to_account`). Everyone stores every hand; only the player's
  own devices open theirs.

A player calls `draw()`, which emits `CardRequested` with the TEE handler
`tee:deal`. The elected TEE authority opens the deck, takes the top card, seals
it to the player and seals the rest back. `reshuffle` is an
`#[app::tee(every = "1m")]` timer: the node's TEE scheduler fires it once a
minute, and it shuffles a fresh deck whenever there is none or it is spent.

> **Mock attestation.** Nobody but the TEE can deal or see the deck only if the
> enclave is genuine. The scenarios here run with mock attestation: the
> cluster's nodes report mock quotes and the cluster is every app's TEE, so no
> admission or authoring policy is set (v1's workflows set `acceptMock` and the
> all-zero measurements a mock quote reports). Production needs a real
> attestation policy: `acceptMock` false, `allowedMrtd` and `allowedRtmr1` to
> `allowedRtmr3` naming the approved image (or `signedRelease`), the real MRTD
> in the authoring policy, and a `merod` that does not run with `--mock-tee`.
> See `docs/src/content/docs/protocol/tee-attestation.mdx`.

## Methods

- `sit()` — take a seat (any member)
- `draw()` — ask the TEE for a card (a seated member)
- `deal(player)` — `#[app::tee]`: run only by the TEE scheduler
- `reshuffle()` — `#[app::tee(every = "1m")]`: run only by the TEE scheduler
- `my_hand()` — your cards, opened on any of your devices
- `hand_size(player)` — how many cards a player holds (anyone)
- `cards_left()` — how many cards the deck holds (anyone)

A card is sealed to every device bound to the player's account when it is
dealt. A device bound later sees that the player holds the card, not which.

## What the TEE cannot tell

Nobody but the TEE can deal, see the deck, or stack it. It cannot tell who
asked for a card, though: a `tee:` handler runs on the arguments of an event,
and a patched node can emit `CardRequested` naming any seated player. The TEE
still deals only to seated players and seals each card to its player, so this
reveals nothing, but a member can push cards into someone else's hand and run
the deck down. A game that must meter draws per player needs a request record
in owned state (an `Authored` or `WriteOnce` entry per draw) that `deal` reads
and checks, instead of trusting the event.

This is a demo of TEE authorship, not a game to deploy as is: if two TEE
authorities deal at once, each pops the same top card and one card can be dealt
twice.

## Building

```bash
cargo mero build
```

Produces `res/tee_cards.wasm`.

## End to end

`scenarios/cluster/tee-cards.json` plays the game on an in-process cluster: a
draw asks the TEE (a `tee:` handler the app's leader runs once), which deals
from a deck only it reads, sealed to the player, and each player reads only
their own hand. v1's failover, late-TEE and key-rotation workflows had no
counterpart on a cluster and were deleted
([Testing strategy](../../docs/src/content/docs/contribute/testing.mdx#workflows-deleted-in-the-migration)).

```bash
cargo test -p calimero-node --test scenarios -- apps/tee-cards/
```
