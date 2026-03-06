# Calimero Poker — Decentralized AI Poker on Calimero

## What It Is

A Texas Hold'em poker engine running as a Calimero WASM application, designed for AI bot competitions. Bots compete on decentralized infrastructure with verifiable randomness and encrypted card dealing.

## How It Works

### Architecture

```
┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│  Dealer Node  │  │  Bot Node A  │  │  Bot Node B  │  │  Bot Node C  │
│  (organizer)  │  │  🦈 SHARK    │  │  📞 STATION  │  │  🎲 GAMBLER  │
│               │  │  TAG strat   │  │  Caller      │  │  Random      │
│  Shuffles     │  │              │  │              │  │              │
│  Encrypts     │  │  Decides     │  │  Decides     │  │  Decides     │
│  Reveals      │  │  Acts        │  │  Acts        │  │  Acts        │
└──────┬───────┘  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘
       │                 │                 │                 │
       └─────────────────┴─────────────────┴─────────────────┘
                    Calimero P2P Network (gossipsub)
                    Shared State (CRDTs) + Private Storage
```

### Secure Dealing Protocol

Each hand follows a commit-reveal protocol:

1. **Commit** — Each bot submits `hash(random_seed)` to shared state. Once committed, the seed can't be changed.
2. **Reveal** — Each bot reveals their seed. Verified against the committed hash.
3. **Shuffle** — Dealer combines all seeds: `deck = shuffle(hash(seed_A || seed_B || seed_C))`. Deterministic — same seeds always produce the same deck. Any one honest bot guarantees fairness.
4. **Encrypt & Deal** — Dealer encrypts each bot's hole cards with their X25519 public key (ECDH + AES-256-GCM). Ciphertext stored in shared state. Only the intended recipient can decrypt.
5. **Play** — Standard Texas Hold'em betting rounds. Dealer reveals community cards per street from private storage.
6. **Showdown** — Non-folded players' cards revealed. Hand evaluator picks the winner. Folded cards are never revealed.

### What Each Party Sees

|  | Own cards | Others' cards | Community | Undealt cards | Full deck |
|---|---|---|---|---|---|
| **Bot** | ✅ decrypts | ❌ encrypted | ✅ when revealed | ❌ | ❌ |
| **Dealer** | N/A (not playing) | ✅ in private storage | ✅ | ✅ | ✅ |

### Card Privacy After the Hand

- Showdown cards: revealed (standard poker)
- Folded cards: **never revealed** — bots can't learn opponents' folded hands
- Undealt cards: never exposed

## What's Built

| Component | Description |
|---|---|
| `apps/poker/` | WASM app — 1,963 lines of Rust, 44 unit tests |
| `apps/poker-bot/` | External bot client — connects via JSON-RPC |
| `apps/poker/demo.py` | Live demo — secure dealing, 3 bots + dealer |
| `apps/poker/workflows/` | 5 merobox E2E test workflows |

### Game Features

- 2-6 players, configurable blinds/buy-in/timeout
- Full hand lifecycle: PreFlop → Flop → Turn → River → Showdown
- 14 public API methods (join, fold, check, call, raise, bot_play, etc.)
- 3 bot strategies: Random, Caller (always calls), TAG (tight-aggressive)
- Hand history, per-player stats, blind escalation
- Timeout enforcement (force-fold idle players)

### Crypto

- X25519 ECDH key exchange (per-player)
- AES-256-GCM card encryption
- SHA-256 commit-reveal for seed fairness
- Deterministic shuffle from combined seeds
- All crypto runs in WASM — no runtime modifications needed

## Running the Demo

```bash
cd apps/poker

# Build
cargo build --target wasm32-unknown-unknown --profile app-release
cp ../../target/wasm32-unknown-unknown/app-release/poker.wasm res/
cd ../poker-bot && cargo build --release && cd ../poker

# Run (3 bots, secure dealing, live output)
python3 demo.py --pace 1 --max-hands 10
```

## Security Model

### What's Protected

| Threat | Protection | Status |
|---|---|---|
| Rigged shuffle | Commit-reveal — any 1 honest bot guarantees fairness | ✅ Implemented |
| Seeing others' cards | X25519 + AES-256-GCM encryption | ✅ Implemented |
| Acting out of turn | `executor_id()` checked against `action_pos` | ✅ Implemented |
| Betting fake chips | WASM enforces deductions, CRDT state synced | ✅ Implemented |
| Modifying local storage | CRDT merge from honest nodes overwrites fake state | ✅ By design |
| Stalling / not acting | `claim_timeout()` force-folds after 30s | ✅ Implemented |

### Remaining Gaps

| Threat | Risk | Mitigation |
|---|---|---|
| Dealer sees all cards | Medium — dealer is non-playing organizer | **TEE** (next step) |
| Seed replay → learn folded cards | Medium — seeds in shared state post-hand | Clear seeds after dealing, or encrypt |
| Collusion (bots sharing info) | Low — undetectable at protocol level | Statistical analysis of play patterns |
| Stalling seed reveal | Low — bot commits but never reveals | Add reveal timeout (not yet implemented) |

## Next Steps

### 1. TEE Dealer (High Priority)

Run the dealer node inside an SGX/TDX enclave. Same WASM, same protocol — the dealer just can't be inspected by the operator. Calimero already has TEE attestation infrastructure (`crates/server/src/admin/handlers/tee/`).

**What changes:** Deployment only. The dealer WASM and protocol stay the same. The node runs inside an enclave and provides an attestation proof that clients can verify.

**What it solves:** The dealer can no longer peek at cards or leak information. The competition organizer is trustless.

### 2. Chips in UserStorage (Medium Priority)

Move chip balances from shared `UnorderedMap` to per-player `UserStorage`. Only the owner can write to their own chips. Winners claim pots via a `claim_pot()` method that verifies the showdown result.

**What it solves:** Defense-in-depth. Even a modified merod binary can't inflate chip counts — the storage layer enforces write permissions per-user.

### 3. Seed Privacy (Medium Priority)

After dealing, clear or encrypt the revealed seeds so that post-hand analysis can't reconstruct the full deck and learn folded cards.

### 4. More Bot Strategies (Low Priority)

- Monte Carlo tree search
- Neural network interface (load model weights, run inference in WASM)
- GTO (game theory optimal) approximation

### 5. Tournament Mode (Low Priority)

Multi-table brackets using `xcall` between contexts. Parent context manages seating, blind schedules, and ELO rankings.
