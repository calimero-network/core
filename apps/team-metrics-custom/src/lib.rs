//! Team Metrics App - Using Custom Mergeable Implementation
//!
//! Demonstrates nested CRDTs with CUSTOM merge logic.
//! This shows the flexibility when you need special merge behavior.

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
/// This struct demonstrates CUSTOM Mergeable implementation.
/// You have full control and can add custom logic!
#[derive(Debug, Default, BorshSerialize, BorshDeserialize)]
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

/// Deterministic re-keying for a HAND-WRITTEN CRDT-value struct (#2577).
///
/// `#[derive(Mergeable)]` generates this for you. When you implement `Mergeable`
/// by hand (as above), you MUST also implement `RekeyTarget`, or this struct —
/// stored as an `UnorderedMap` value — is last-writer-wins'd as an opaque blob
/// and its counters silently lose concurrent increments. Re-key each nested
/// collection field under a field-namespaced child of the entry id so every
/// replica derives identical ids and the counters converge as child entities.
impl calimero_storage::collections::rekey::RekeyTarget for TeamStats {
    fn rekey_relative_to(&mut self, parent_id: calimero_storage::address::Id) {
        use calimero_storage::collections::rekey::field_child_id;
        calimero_storage::rekey_field_if_supported!(
            &mut self.wins,
            field_child_id(parent_id, "wins")
        );
        calimero_storage::rekey_field_if_supported!(
            &mut self.losses,
            field_child_id(parent_id, "losses")
        );
        calimero_storage::rekey_field_if_supported!(
            &mut self.draws,
            field_child_id(parent_id, "draws")
        );
    }
}

/// Application state
#[app::state(emits = MetricsEvent)]
pub struct TeamMetricsApp {
    /// Maps team_id → team statistics
    /// TeamStats has a CUSTOM Mergeable impl with full control
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
