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
#[derive(Mergeable, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct TeamStats {
    pub wins: Counter,
    pub losses: Counter,
    pub draws: Counter,
}

/// Application state
#[app::state(emits = MetricsEvent)]
#[derive(BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct TeamMetricsApp {
    /// Maps team_id → team statistics
    /// The TeamStats struct uses #[derive(Mergeable)] - no manual impl needed!
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

        self.teams
            .insert(team_id.clone(), stats)
            .map_err(|e| format!("Insert failed: {:?}", e))?;

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

        self.teams
            .insert(team_id.clone(), stats)
            .map_err(|e| format!("Insert failed: {:?}", e))?;

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

        self.teams
            .insert(team_id.clone(), stats)
            .map_err(|e| format!("Insert failed: {:?}", e))?;

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
