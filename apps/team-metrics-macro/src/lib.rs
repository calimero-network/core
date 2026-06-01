//! Team Metrics App - Using #[derive(Mergeable)]
//!
//! Demonstrates nested CRDTs with automatic merge via derive macro.
//! This is the SIMPLEST way to use nested structures.

#![allow(
    unused_crate_dependencies,
    reason = "Dependencies used in build process"
)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::{Counter, UnorderedMap};
use calimero_storage_macros::Mergeable;

/// Team statistics with multiple counters
///
/// This struct demonstrates #[derive(Mergeable)] - zero boilerplate!
/// All fields are CRDTs, so the macro just calls merge on each.
#[derive(Debug, Default, Mergeable, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct TeamStats {
    pub wins: Counter,
    pub losses: Counter,
    pub draws: Counter,
}

/// Application state
#[app::state(emits = MetricsEvent)]
#[derive(Debug)]
pub struct TeamMetricsApp {
    /// Maps team_id → team statistics
    /// The TeamStats struct uses #[derive(Mergeable)] - no manual impl needed!
    pub teams: UnorderedMap<String, TeamStats>,
}

#[app::event]
#[derive(Debug)]
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
        let mut stats = self.teams.get(&team_id)?.unwrap_or_default();

        stats.wins.increment()?;
        let total = stats.wins.value()?;

        self.teams.insert(team_id.clone(), stats)?;

        app::emit!(MetricsEvent::WinRecorded { team_id, total });

        Ok(total)
    }

    pub fn record_loss(&mut self, team_id: String) -> app::Result<u64> {
        let mut stats = self.teams.get(&team_id)?.unwrap_or_default();

        stats.losses.increment()?;
        let total = stats.losses.value()?;

        self.teams.insert(team_id.clone(), stats)?;

        app::emit!(MetricsEvent::LossRecorded { team_id, total });

        Ok(total)
    }

    pub fn record_draw(&mut self, team_id: String) -> app::Result<u64> {
        let mut stats = self.teams.get(&team_id)?.unwrap_or_default();

        stats.draws.increment()?;
        let total = stats.draws.value()?;

        self.teams.insert(team_id.clone(), stats)?;

        app::emit!(MetricsEvent::DrawRecorded { team_id, total });

        Ok(total)
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
