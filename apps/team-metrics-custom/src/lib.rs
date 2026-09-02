//! Team Metrics — app-defined merge.
//!
//! The sibling of `team-metrics-macro`, which uses `#[derive(Mergeable)]` and
//! lets the storage layer converge everything structurally. This one declares a
//! rule of its own, and the difference is visible in exactly one field.
//!
//! ## What `#[app::mergeable]` buys, and what it does not
//!
//! `wins` / `losses` / `draws` are `Counter`s. They converge whether or not this
//! app declares anything: a counter is stored as its own child entity and the
//! storage layer merges it by summing per-writer contributions. Removing
//! `#[app::mergeable]` would not change them.
//!
//! `badges` is a plain `u64` bitmask. It lives in the value blob, and without a
//! declared rule the blob resolves last-write-wins — one node's badges survive
//! and the other node's are discarded. `#[app::mergeable]` makes bitwise-OR the
//! rule instead, so a team awarded badge 1 on one node and badge 2 on another
//! ends up holding BOTH on both nodes.
//!
//! Union is the distinguishing case on purpose. `max` would not be: it returns
//! one of its two inputs, and so does last-write-wins, so the two agree whenever
//! LWW happens to pick the larger side — a test built on `max` passes with the
//! merge rule deleted. `1 | 2 == 3` is a value neither input holds, so only a
//! dispatched merge can produce it.
//!
//! ## The contract
//!
//! Dispatch hands merge authority to this code, so `merge` must be
//! deterministic, commutative, associative, idempotent and **total**. The last
//! is the trap: returning `Err` is not validation, it is a refusal to converge —
//! the entity stays divergent and repair retries it forever. Reject bad input in
//! `set_streak`, not here.

#![allow(
    unused_crate_dependencies,
    reason = "Dependencies used in build process"
)]

use calimero_sdk::abi::AbiType;
use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::{Counter, Mergeable, UnorderedMap};

/// Per-team statistics, merged by this app rather than by the storage layer.
///
/// `#[app::mergeable]` gives the type a `CustomTypeId` that is stamped on every
/// entry holding it, so the merge point dispatches here instead of resolving the
/// entry by last-write-wins. It also generates the `RekeyTarget` impl that a
/// hand-written `Mergeable` used to have to supply by itself — forgetting which
/// silently lost the counters' concurrent increments.
#[app::mergeable]
#[derive(Debug, Default, BorshSerialize, BorshDeserialize, AbiType)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct TeamStats {
    pub wins: Counter,
    pub losses: Counter,
    pub draws: Counter,
    /// Achievement bitmask — one bit per badge.
    ///
    /// A plain `u64`, deliberately: it is the one field whose convergence
    /// depends on the rule below rather than on the storage layer.
    pub badges: u64,
}

impl Mergeable for TeamStats {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // The counters would converge without this — they are child entities
        // with their own merge. Delegating keeps the root-conflict path
        // consistent with the per-entity one.
        self.wins.merge(&other.wins)?;
        self.losses.merge(&other.losses)?;
        self.draws.merge(&other.draws)?;

        // The part that only happens because this rule is dispatched. OR is
        // commutative, associative and idempotent — a grow-only set of badges —
        // and, unlike `max`, its result is a value NEITHER side held, so it
        // cannot be mistaken for last-write-wins picking a winner.
        self.badges |= other.badges;

        Ok(())
    }
}

/// Application state
#[app::state(emits = MetricsEvent)]
pub struct TeamMetricsApp {
    /// Maps team_id → team statistics
    /// Values are merged by `TeamStats`'s own rule; see the module docs.
    pub teams: UnorderedMap<String, TeamStats>,
}

#[app::event]
pub enum MetricsEvent {
    WinRecorded { team_id: String, total: u64 },
    LossRecorded { team_id: String, total: u64 },
    DrawRecorded { team_id: String, total: u64 },
}

#[app::logic]
impl TeamMetricsApp {
    #[app::init]
    pub fn init() -> TeamMetricsApp {
        TeamMetricsApp {
            teams: UnorderedMap::new(),
        }
    }

    pub fn record_win(&mut self, team_id: String) -> app::Result<u64> {
        let mut stats = self.teams.entry(team_id.clone())?.or_default()?;

        stats.wins.increment()?;
        let total = stats.wins.value()?;

        app::emit!(MetricsEvent::WinRecorded { team_id, total });

        Ok(total)
    }

    pub fn record_loss(&mut self, team_id: String) -> app::Result<u64> {
        let mut stats = self.teams.entry(team_id.clone())?.or_default()?;

        stats.losses.increment()?;
        let total = stats.losses.value()?;

        app::emit!(MetricsEvent::LossRecorded { team_id, total });

        Ok(total)
    }

    pub fn record_draw(&mut self, team_id: String) -> app::Result<u64> {
        let mut stats = self.teams.entry(team_id.clone())?.or_default()?;

        stats.draws.increment()?;
        let total = stats.draws.value()?;

        app::emit!(MetricsEvent::DrawRecorded { team_id, total });

        Ok(total)
    }

    /// Award a badge (0-63). Validation belongs here, on the write path — NOT
    /// in `merge`, where an `Err` would stop the entity converging.
    pub fn award_badge(&mut self, team_id: String, badge: u64) -> app::Result<u64> {
        if badge > 63 {
            app::bail!("badge out of range");
        }

        let mut stats = self.teams.entry(team_id)?.or_default()?;
        stats.badges |= 1_u64 << badge;

        Ok(stats.badges)
    }

    /// Award the badge that belongs to the CALLING device, derived from its
    /// device id.
    ///
    /// This exists to be testable, and the reason is worth stating. A
    /// convergence harness applies the same op list to every replica, so an op
    /// taking an explicit badge number leaves every replica computing the same
    /// value — no conflict, nothing for a merge to do, and a test that passes
    /// with the merge rule deleted. A counter avoids that by being per-writer;
    /// this makes the badge per-writer for the same reason, so identical ops
    /// still produce divergent blobs that only a union can reconcile.
    pub fn award_own_badge(&mut self, team_id: String) -> app::Result<u64> {
        let badge = u64::from(calimero_sdk::env::device_id()[0] % 64);

        self.award_badge(team_id, badge)
    }

    /// How many badges the team holds.
    ///
    /// Exposed so an e2e assertion can be a plain integer comparison: the
    /// individual bits depend on each node's device id and are not predictable
    /// from a workflow, and relying on the assertion evaluator to provide
    /// `bin(..).count('1')` would be betting on its expression support.
    pub fn get_badge_count(&self, team_id: String) -> app::Result<u32> {
        let Some(stats) = self.teams.get(&team_id)? else {
            app::bail!("Team not found");
        };

        Ok(stats.badges.count_ones())
    }

    pub fn get_badges(&self, team_id: String) -> app::Result<u64> {
        let Some(stats) = self.teams.get(&team_id)? else {
            app::bail!("Team not found");
        };

        Ok(stats.badges)
    }

    pub fn get_wins(&self, team_id: String) -> app::Result<u64> {
        let Some(stats) = self.teams.get(&team_id)? else {
            app::bail!("Team not found");
        };

        Ok(stats.wins.value()?)
    }

    pub fn get_losses(&self, team_id: String) -> app::Result<u64> {
        let Some(stats) = self.teams.get(&team_id)? else {
            app::bail!("Team not found");
        };

        Ok(stats.losses.value()?)
    }

    pub fn get_draws(&self, team_id: String) -> app::Result<u64> {
        let Some(stats) = self.teams.get(&team_id)? else {
            app::bail!("Team not found");
        };

        Ok(stats.draws.value()?)
    }
}

#[cfg(test)]
mod tests {
    use calimero_sdk::testing::TestHost;

    use super::*;

    #[test]
    fn records_accumulate_per_team() {
        let mut app = TestHost::new(TeamMetricsApp::init);

        assert_eq!(app.call(|s| s.record_win("red".into())).unwrap(), 1);
        assert_eq!(app.call(|s| s.record_win("red".into())).unwrap(), 2);
        app.call(|s| s.record_loss("red".into())).unwrap();
        app.call(|s| s.record_draw("red".into())).unwrap();

        assert_eq!(app.view(|s| s.get_wins("red".into())).unwrap(), 2);
        assert_eq!(app.view(|s| s.get_losses("red".into())).unwrap(), 1);
        assert_eq!(app.view(|s| s.get_draws("red".into())).unwrap(), 1);
    }

    #[test]
    fn teams_are_independent() {
        let mut app = TestHost::new(TeamMetricsApp::init);

        app.call(|s| s.record_win("red".into())).unwrap();
        app.call(|s| s.record_win("blue".into())).unwrap();
        app.call(|s| s.record_win("blue".into())).unwrap();

        assert_eq!(app.view(|s| s.get_wins("red".into())).unwrap(), 1);
        assert_eq!(app.view(|s| s.get_wins("blue".into())).unwrap(), 2);
    }
}
