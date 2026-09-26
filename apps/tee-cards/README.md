# TEE Cards

Hidden cards no player can peek at or stack: TEE authorship with secrets (see
`docs/src/content/docs/protocol/tee-authorship.mdx`).

- **Seats** (`UserStorage`): each player records the device key their cards are
  sealed to. Nobody can repoint another player's seat.
- **The deck** (`TeeSecret<Vec<u8>>`): sealed to every TEE authority, so members
  replicate it and cannot read it.
- **Hands** (`TeeOnly`): each card sealed to its player's seat key. Everyone
  stores every hand; only the player's own node opens theirs.

A player calls `draw()`, which emits `CardRequested` with the TEE handler
`tee:deal`. The elected TEE authority opens the deck, takes the top card, seals
it to the player and seals the rest back. `reshuffle` is an
`#[app::tee(every = "1m")]` timer: the node's TEE scheduler fires it once a
minute, and it shuffles a fresh deck whenever there is none or it is spent.

## Methods

- `sit()` — take a seat on the device you call from (any member)
- `draw()` — ask the TEE for a card (a seated member)
- `deal(player)` — `#[app::tee]`: run only by the TEE scheduler
- `reshuffle()` — `#[app::tee(every = "1m")]`: run only by the TEE scheduler
- `my_hand()` — your cards, opened on your own node
- `hand_size(player)` — how many cards a player holds (anyone)
- `cards_left()` — how many cards the deck holds (anyone)

A card is sealed to one device. A player's other devices see that they hold
cards, not which.

## Building

```bash
cargo mero build
```

Produces `res/tee_cards.wasm`.
