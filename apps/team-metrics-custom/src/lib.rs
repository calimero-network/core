//! Team Metrics App - Custom Type Merge via WASM Callback
//!
//! This app demonstrates **custom CRDT types** that are merged via WASM callback
//! during sync (PR #1940, Issue #1780).
//!
//! ## WASM Exports Generated
//!
//! | Export | Source | Purpose |
//! |--------|--------|---------|
//! | `__calimero_merge_TeamStats` | `#[app::mergeable]` | Merge `TeamStats` entities |
//! | `__calimero_merge_root_state` | `#[app::state]` | Merge root state conflicts |
//! | `__calimero_alloc` | `#[app::state]` | Memory allocation for merge |
//!
//! ## How Custom Type Merge Works
//!
//! When `TeamStats` entities conflict during sync:
//!
//! ```text
//! Entity with CrdtType::Custom("TeamStats") conflicts
//!   → storage calls merge_by_crdt_type_with_callback()
//!   → dispatches to CrdtType::Custom("TeamStats")
//!   → calls RuntimeMergeCallback::merge_custom_mut("TeamStats", ...)
//!   → runtime calls __calimero_merge_TeamStats WASM export
//!   → TeamStats::merge() is invoked
//!   → merged bytes returned to storage
//! ```
//!
//! ## Test Scenario
//!
//! 1. Node A: `record_win("Team1")` → Team1 {wins:1, losses:0}
//! 2. Node B: `record_loss("Team1")` → Team1 {wins:0, losses:1} (concurrent)
//! 3. Sync detects conflict on same entity
//! 4. Storage sees `CrdtType::Custom("TeamStats")` in metadata
//! 5. Calls WASM `__calimero_merge_TeamStats`
//! 6. Result: Team1 {wins:1, losses:1} (CRDT merge via custom impl)

#![allow(
    unused_crate_dependencies,
    reason = "Dependencies used in build process"
)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::{Counter, Mergeable, UnorderedMap};

/// Team statistics with multiple counters
///
/// This struct has a **custom `Mergeable` implementation** (see below).
/// The merge is invoked when:
/// 1. Two nodes concurrently modify the same team's stats
/// 2. Sync triggers root state merge via `__calimero_merge_root_state`
/// 3. `UnorderedMap::merge()` calls `TeamStats::merge()` for each entry
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct TeamStats {
    pub wins: Counter,
    pub losses: Counter,
    pub draws: Counter,
}

/// Custom Mergeable implementation for TeamStats
///
/// This demonstrates custom merge logic that:
/// - Merges each counter using standard CRDT merge
/// - Could add custom validation, logging, or business rules
///
/// ## The #[app::mergeable] Macro
///
/// This macro generates `__calimero_merge_TeamStats` WASM export.
/// When entities with `CrdtType::Custom("TeamStats")` conflict during sync,
/// the runtime calls this export to merge them.
///
/// ## When is this called?
///
/// For entity-level conflicts (e.g., two nodes update same map entry):
/// ```text
/// Entity with CrdtType::Custom("TeamStats") conflicts
///   → storage layer calls merge_custom("TeamStats", local, remote)
///   → runtime calls __calimero_merge_TeamStats WASM export
///   → this Mergeable::merge() impl is invoked
/// ```
#[app::mergeable]
impl Mergeable for TeamStats {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // Standard CRDT merge for each counter
        // PnCounter merge: positive = max(p1, p2), negative = max(n1, n2)
        self.wins.merge(&other.wins)?;
        self.losses.merge(&other.losses)?;
        self.draws.merge(&other.draws)?;

        // Example: Could add custom validation after merge
        // if self.total_games() > MAX_GAMES {
        //     return Err(MergeError::Custom("Too many games".into()));
        // }

        Ok(())
    }
}

/// Application state
#[app::state(emits = MetricsEvent)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct TeamMetricsApp {
    /// Maps team_id → team statistics
    /// TeamStats has a CUSTOM Mergeable impl with full control
    pub teams: UnorderedMap<String, TeamStats>,
}

#[app::event]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
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

    pub fn record_win(&mut self, team_id: String) -> Result<u64, String> {
        let mut stats = self
            .teams
            .get(&team_id)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .unwrap_or_else(|| TeamStats {
                wins: Counter::new(),
                losses: Counter::new(),
                draws: Counter::new(),
            });

        stats
            .wins
            .increment()
            .map_err(|e| format!("Increment failed: {:?}", e))?;
        let total = stats
            .wins
            .value()
            .map_err(|e| format!("Value failed: {:?}", e))?;

        drop(
            self.teams
                .insert(team_id.clone(), stats)
                .map_err(|e| format!("Insert failed: {:?}", e))?,
        );

        app::emit!(MetricsEvent::WinRecorded { team_id, total });

        Ok(total)
    }

    pub fn record_loss(&mut self, team_id: String) -> Result<u64, String> {
        let mut stats = self
            .teams
            .get(&team_id)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .unwrap_or_else(|| TeamStats {
                wins: Counter::new(),
                losses: Counter::new(),
                draws: Counter::new(),
            });

        stats
            .losses
            .increment()
            .map_err(|e| format!("Increment failed: {:?}", e))?;
        let total = stats
            .losses
            .value()
            .map_err(|e| format!("Value failed: {:?}", e))?;

        drop(
            self.teams
                .insert(team_id.clone(), stats)
                .map_err(|e| format!("Insert failed: {:?}", e))?,
        );

        app::emit!(MetricsEvent::LossRecorded { team_id, total });

        Ok(total)
    }

    pub fn record_draw(&mut self, team_id: String) -> Result<u64, String> {
        let mut stats = self
            .teams
            .get(&team_id)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .unwrap_or_else(|| TeamStats {
                wins: Counter::new(),
                losses: Counter::new(),
                draws: Counter::new(),
            });

        stats
            .draws
            .increment()
            .map_err(|e| format!("Increment failed: {:?}", e))?;
        let total = stats
            .draws
            .value()
            .map_err(|e| format!("Value failed: {:?}", e))?;

        drop(
            self.teams
                .insert(team_id.clone(), stats)
                .map_err(|e| format!("Insert failed: {:?}", e))?,
        );

        app::emit!(MetricsEvent::DrawRecorded { team_id, total });

        Ok(total)
    }

    pub fn get_wins(&self, team_id: String) -> Result<u64, String> {
        self.teams
            .get(&team_id)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .map(|s| s.wins.value().unwrap_or(0))
            .ok_or_else(|| "Team not found".to_owned())
    }

    pub fn get_losses(&self, team_id: String) -> Result<u64, String> {
        self.teams
            .get(&team_id)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .map(|s| s.losses.value().unwrap_or(0))
            .ok_or_else(|| "Team not found".to_owned())
    }

    pub fn get_draws(&self, team_id: String) -> Result<u64, String> {
        self.teams
            .get(&team_id)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .map(|s| s.draws.value().unwrap_or(0))
            .ok_or_else(|| "Team not found".to_owned())
    }
}
