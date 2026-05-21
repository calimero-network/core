//! Demonstrates `#[app::private]` — node-local state that is NOT
//! synchronised across the mesh.
//!
//! This app pairs two pieces of state:
//!
//! - [`SecretGame`] — the public `#[app::state]` struct synced
//!   across all nodes (only `games`, the hash registry).
//! - [`Secrets`] — the `#[app::private]` struct, node-local. Each
//!   node has its own set of secrets that other nodes can never
//!   observe (the on-the-wire game hash is the only thing that
//!   leaves the node).
//!
//! ## What can live inside `#[app::private]`
//!
//! The `Secrets` struct below exercises the full supported set of
//! field shapes:
//!
//! - **Primitives** (`u64`, `bool`, `String`, etc.) — borsh-serialised
//!   into the outer private blob. Whole blob is rewritten on every
//!   `save`; fine for small state.
//! - **`std::collections` types** (`BTreeMap`, `BTreeSet`, `Vec`) —
//!   same as primitives: borsh-serialised into the blob. Use for
//!   small structured local state where rewrite-on-change is fine.
//! - **Tree-backed structural collections** (`UnorderedMap`,
//!   `UnorderedSet`, `Vector`) — the `#[app::private]` macro
//!   automatically substitutes the storage adaptor to
//!   `PrivateStorage` on these field types, so each entry is stored
//!   as a separate node-local entity and only the entries you
//!   actually change are rewritten. Use these for any
//!   non-trivially-sized private state where blob-level
//!   rewrite-on-change would be too expensive.
//!
//! ## What's deliberately not supported
//!
//! - **CRDT data-types** (`LwwRegister`, `Counter`, `GCounter`,
//!   `PNCounter`, `ReplicatedGrowableArray`) — CRDTs exist for
//!   multi-writer conflict resolution; private storage has exactly
//!   one writer (this node), so the per-writer bookkeeping is
//!   overhead without a corresponding semantic gain. Use a plain
//!   `u64` / `String` / `Vec` instead.
//! - **Access-control collections** (`SharedStorage`, `UserStorage`,
//!   `FrozenStorage`) and **authored collections** (`AuthoredMap`,
//!   `AuthoredVector`) — their semantics (cross-writer mutability,
//!   per-user separation, immutability, per-entry authorship) all
//!   assume the synced tree.
//!
//! Using any of the above inside `#[app::private]` will produce a
//! regular Rust type error at compile time, since their `::new()`
//! constructors stay pinned to `MainStorage`.

#![allow(clippy::len_without_is_empty)]

use std::collections::{BTreeMap, BTreeSet};

use bs58;
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedMap, UnorderedSet, Vector};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Public app state — synced across all nodes via the merkle tree.
///
/// Only the hash of each secret lives here; the secret itself is
/// node-local (see [`Secrets`]).
#[app::state(emits = for<'a> Event<'a>)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct SecretGame {
    /// Mapping of game_id -> sha256(secret) hex. `LwwRegister` is a
    /// CRDT type — appropriate here because this is synced state
    /// where concurrent writes from different nodes need
    /// last-writer-wins resolution.
    games: UnorderedMap<String, LwwRegister<String>>,
}

/// Node-local private state — NOT synchronised.
///
/// Demonstrates every field shape supported inside
/// `#[app::private]`. See the module-level doc-comment for the
/// design rationale.
#[derive(BorshSerialize, BorshDeserialize, Debug)]
#[borsh(crate = "calimero_sdk::borsh")]
#[app::private]
pub struct Secrets {
    // -----------------------------------------------------------------
    // Tree-backed structural collections — auto-substituted to
    // `PrivateStorage` by the `#[app::private]` macro. Each entry is
    // stored as a separate node-local entity; only changed entries
    // are rewritten.
    //
    // Use these when the collection can grow to non-trivial size or
    // when individual entries change independently and you don't
    // want to rewrite the whole blob on every mutation.
    // -----------------------------------------------------------------
    /// The actual secrets the user typed in — key-value of game_id
    /// to plaintext secret.
    secrets: UnorderedMap<String, String>,

    /// Per-node guess history — game IDs the user has attempted a
    /// guess on (each attempt's outcome is local to the node).
    /// Using a set demonstrates `UnorderedSet` substitution; would
    /// also work as `UnorderedMap<String, ()>` but the set is the
    /// natural shape.
    attempted_games: UnorderedSet<String>,

    /// Append-only log of guesses (game_id, guess_text) the user
    /// made. Demonstrates `Vector` substitution. Persists across
    /// app reboots on this node only.
    guess_log: Vector<GuessEntry>,

    // -----------------------------------------------------------------
    // Primitives + std types — borsh-serialised into the outer
    // private blob. Rewritten in full on every save. Fine for small
    // state that changes infrequently.
    // -----------------------------------------------------------------
    /// How many times this node has used `add_secret`. A plain
    /// counter — no CRDT needed because only this node writes.
    secrets_added: u64,

    /// Whether the user has opted into "remember last guess" UX. A
    /// single bool is a textbook case for the borsh-blob path —
    /// rewriting one byte's worth of state on toggle is trivial.
    remember_last_guess: bool,

    /// The most recent guess the user typed, if any. Stored
    /// alongside the bool above purely so the demo covers
    /// `Option<String>` round-tripping through the private blob.
    last_guess: Option<String>,

    /// User-defined notes per game — small text. `BTreeMap` not
    /// `UnorderedMap` because:
    /// 1. notes are short-and-few (a handful at most), so the blob
    ///    rewrite is cheap;
    /// 2. ordered iteration (alphabetical by game_id) is convenient
    ///    when displaying the notes.
    notes: BTreeMap<String, String>,

    /// Tags the user has applied to themselves locally. `BTreeSet`
    /// covers the std-set case alongside `UnorderedSet` above; the
    /// difference is identical to map vs unordered-map — pick
    /// `BTreeSet` for small ordered local state, `UnorderedSet` for
    /// scale.
    tags: BTreeSet<String>,
}

/// One entry in the [`Secrets::guess_log`] vector.
#[derive(BorshSerialize, BorshDeserialize, Debug, Clone)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct GuessEntry {
    pub game_id: String,
    pub guess: String,
    pub success: bool,
}

impl Default for Secrets {
    fn default() -> Self {
        Self {
            // Tree-backed: the macro substituted `PrivateStorage` as
            // the trailing generic on the field types above, so
            // these constructors infer `S = PrivateStorage` from the
            // assignment site.
            secrets: UnorderedMap::new(),
            attempted_games: UnorderedSet::new(),
            guess_log: Vector::new(),
            // Primitives + std types: just sensible defaults.
            secrets_added: 0,
            remember_last_guess: false,
            last_guess: None,
            notes: BTreeMap::new(),
            tags: BTreeSet::new(),
        }
    }
}

#[app::event]
pub enum Event<'a> {
    SecretSet {
        game_id: &'a str,
    },
    Guessed {
        game_id: &'a str,
        success: bool,
        by: &'a str,
    },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("no public hash set yet")]
    NoHash,
    #[error("utf8 error: {0}")]
    Utf8(&'a str),
}

#[app::logic]
impl SecretGame {
    #[app::init]
    pub fn init() -> SecretGame {
        SecretGame {
            games: UnorderedMap::new(),
        }
    }

    /// Create/update a game by id: store secret privately and record
    /// its hash publicly.
    pub fn add_secret(&mut self, game_id: String, secret: String) -> app::Result<()> {
        // Save the secret + bookkeeping in the node-local
        // `Secrets` private struct.
        let mut secrets = Secrets::private_load_or_default()?;
        let mut secrets_mut = secrets.as_mut();
        secrets_mut
            .secrets
            .insert(game_id.clone(), secret.clone())?;
        secrets_mut.secrets_added = secrets_mut.secrets_added.saturating_add(1);

        // Save public hash for guess verification in the synced
        // `games` map.
        let hash = Sha256::digest(secret.as_bytes());
        let hash_hex = hex::encode(hash);
        self.games.insert(game_id.clone(), hash_hex.into())?;
        app::emit!(Event::SecretSet { game_id: &game_id });
        Ok(())
    }

    /// Allow a user to guess the secret; returns true if the guess
    /// matches the stored hash. `who` is derived from the executor
    /// identity rather than passed as an argument.
    ///
    /// Takes `&self` because the *public* `SecretGame` state isn't
    /// mutated here — only this node's private `Secrets`. The two
    /// persistence paths are:
    ///
    /// - Tree-backed `attempted_games` / `guess_log` write each new
    ///   entry through `PrivateStorage` immediately on
    ///   `insert` / `push` (no save call needed).
    /// - The borsh-blob field `last_guess` is persisted by the
    ///   `EntryMut` `Drop` impl when `secrets_mut` goes out of scope.
    ///   Early `?`-returns still drop the guard, so the blob saves
    ///   whether we exit normally or via error propagation.
    pub fn add_guess(&self, game_id: &str, guess: String) -> app::Result<bool> {
        let Some(public_hash_hex) = self.games.get(game_id)?.map(|v| v.get().clone()) else {
            app::bail!(Error::NoHash);
        };
        let guess_hash = Sha256::digest(guess.as_bytes());
        let guess_hash_hex = hex::encode(guess_hash);
        let who_b = calimero_sdk::env::executor_id();
        let who = bs58::encode(who_b).into_string();
        let success = guess_hash_hex == public_hash_hex;

        // Record the guess in local-only history.
        let mut secrets = Secrets::private_load_or_default()?;
        let mut secrets_mut = secrets.as_mut();
        secrets_mut.attempted_games.insert(game_id.to_owned())?;
        secrets_mut.guess_log.push(GuessEntry {
            game_id: game_id.to_owned(),
            guess: guess.clone(),
            success,
        })?;
        if secrets_mut.remember_last_guess {
            secrets_mut.last_guess = Some(guess);
        }

        app::emit!(Event::Guessed {
            game_id,
            success,
            by: &who
        });
        Ok(success)
    }

    /// Toggle the "remember last guess" UX flag (private state).
    pub fn set_remember_last_guess(&self, value: bool) -> app::Result<()> {
        let mut secrets = Secrets::private_load_or_default()?;
        secrets.as_mut().remember_last_guess = value;
        Ok(())
    }

    /// Attach a private note to a game (private state).
    pub fn set_note(&self, game_id: String, note: String) -> app::Result<()> {
        let mut secrets = Secrets::private_load_or_default()?;
        let _ = secrets.as_mut().notes.insert(game_id, note);
        Ok(())
    }

    /// Add a private user-defined tag.
    pub fn add_tag(&self, tag: String) -> app::Result<()> {
        let mut secrets = Secrets::private_load_or_default()?;
        let _ = secrets.as_mut().tags.insert(tag);
        Ok(())
    }

    /// Get all local secrets from private storage for the current caller.
    pub fn my_secrets(&self) -> app::Result<BTreeMap<String, String>> {
        let secrets = Secrets::private_load_or_default()?;
        let map: BTreeMap<_, _> = secrets.secrets.entries()?.collect();
        Ok(map)
    }

    /// Get all public games and their secret hashes.
    pub fn games(&self) -> app::Result<BTreeMap<String, String>> {
        Ok(self
            .games
            .entries()?
            .map(|(k, v)| (k, v.get().clone()))
            .collect())
    }

    /// Number of `add_secret` calls this node has performed.
    pub fn secrets_added(&self) -> app::Result<u64> {
        Ok(Secrets::private_load_or_default()?.secrets_added)
    }

    /// Game IDs this node has ever attempted a guess on.
    pub fn attempted_games(&self) -> app::Result<BTreeSet<String>> {
        let secrets = Secrets::private_load_or_default()?;
        let out: BTreeSet<String> = secrets.attempted_games.iter()?.collect();
        Ok(out)
    }

    /// All private notes on this node, ordered by game_id.
    pub fn notes(&self) -> app::Result<BTreeMap<String, String>> {
        Ok(Secrets::private_load_or_default()?.notes.clone())
    }

    /// All private tags this user has set on this node.
    pub fn tags(&self) -> app::Result<BTreeSet<String>> {
        Ok(Secrets::private_load_or_default()?.tags.clone())
    }

    /// Total number of guesses this node has logged across all
    /// games.
    pub fn guess_count(&self) -> app::Result<u64> {
        let secrets = Secrets::private_load_or_default()?;
        Ok(secrets.guess_log.len()? as u64)
    }

    /// Whether the "remember last guess" flag is on.
    pub fn remember_last_guess(&self) -> app::Result<bool> {
        Ok(Secrets::private_load_or_default()?.remember_last_guess)
    }

    /// The most recently typed guess on this node, if any (only
    /// populated when `remember_last_guess` was on at the time).
    pub fn last_guess(&self) -> app::Result<Option<String>> {
        Ok(Secrets::private_load_or_default()?.last_guess.clone())
    }
}
