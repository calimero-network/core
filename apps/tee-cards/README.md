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

## Building

```bash
cargo mero build
```

Produces `res/tee_cards.wasm`.

## End to end

`workflows/tee-cards-failover.yml` runs two mock-TEE authorities: the timer
shuffles the deck, one enclave is stopped, and every later draw is still dealt
by the other. It needs a merod built with `--features mock-attestation`:

```bash
cargo build -p merod --release --features mock-attestation
merobox bootstrap run workflows/tee-cards-failover.yml \
  --image merod:local-mock-tee --e2e-mode
```
