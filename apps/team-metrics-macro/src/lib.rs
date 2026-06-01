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
pub struct TeamMetricsApp {
    /// Maps team_id → team statistics
    /// The TeamStats struct uses #[derive(Mergeable)] - no manual impl needed!
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
        // `or_default()` hands back a write-back guard over the (possibly newly
        // created) `TeamStats`; mutating it through the guard re-persists the
        // whole struct on drop, no manual re-insert.
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
