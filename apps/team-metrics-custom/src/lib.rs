//! Team Metrics App - Using Custom Mergeable Implementation
//!
//! Demonstrates nested CRDTs with CUSTOM merge logic.
//! This shows the flexibility when you need special merge behavior.

#![allow(unused_crate_dependencies)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::{Counter, Mergeable, UnorderedMap};

/// Team statistics with multiple counters
///
/// This struct demonstrates CUSTOM Mergeable implementation.
/// You have full control and can add custom logic!
#[derive(BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct TeamStats {
    pub wins: Counter,
    pub losses: Counter,
    pub draws: Counter,
}

/// Custom Mergeable implementation
///
/// This shows how you can:
/// - Add custom validation
/// - Log merge events  
/// - Apply business rules
/// - Skip certain fields
impl Mergeable for TeamStats {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // Example: You could add logging here
        // eprintln!("Merging team stats...");

        // Standard CRDT merge
        self.wins.merge(&other.wins)?;
        self.losses.merge(&other.losses)?;
        self.draws.merge(&other.draws)?;

        // Example: You could add validation
        // if self.wins.value()? > 1000 {
        //     return Err(MergeError::InvalidValue("Too many wins!".into()));
        // }

        Ok(())
    }
}

/// Application state
#[app::state(emits = MetricsEvent)]
#[derive(BorshSerialize, BorshDeserialize)]
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
            .ok_or_else(|| "Team not found".to_string())
    }

    pub fn get_losses(&self, team_id: String) -> Result<u64, String> {
        self.teams
            .get(&team_id)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .map(|s| s.losses.value().unwrap_or(0))
            .ok_or_else(|| "Team not found".to_string())
    }

    pub fn get_draws(&self, team_id: String) -> Result<u64, String> {
        self.teams
            .get(&team_id)
            .map_err(|e| format!("Get failed: {:?}", e))?
            .map(|s| s.draws.value().unwrap_or(0))
            .ok_or_else(|| "Team not found".to_string())
    }
}
