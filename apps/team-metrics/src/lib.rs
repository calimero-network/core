//! Team metrics: wins, losses, draws and an achievement bitmask per team.
//!
//! `TeamStats` is a plain value in a map keyed by team, as in v1: three counts and `badges`,
//! a `u64` with one bit per badge, which `award_badge` ORs a bit into.
//!
//! What no longer exists: v1 shipped this as two apps, one deriving `Mergeable` for a struct
//! of `Counter`s that storage merged field by field, and one declaring its own merge rule
//! (`#[app::mergeable]`, `badges |= other.badges`) so that two replicas awarding different
//! badges concurrently converged to the union instead of last-write-wins. Here one Raft log
//! orders every call: two members recording a win at once are two entries that each add one,
//! and each `award_badge` reads the current mask before setting its bit, so concurrent awards
//! already end up holding both badges. There are no replicas to merge and no merge rule to
//! declare.

use calimero_app_sdk::prelude::*;

/// One team's statistics.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct TeamStats {
    pub wins: u64,
    pub losses: u64,
    pub draws: u64,
    /// Achievement bitmask, one bit per badge (0-63).
    pub badges: u64,
}

#[app::state(version = 1)]
pub struct TeamMetricsApp {
    teams: UnorderedMap<String, TeamStats>,
}

#[app::error]
pub enum Error {
    TeamNotFound { team_id: String },
    BadgeOutOfRange { badge: u32 },
}

#[app::event]
pub enum MetricsEvent {
    WinRecorded { team_id: String, total: u64 },
    LossRecorded { team_id: String, total: u64 },
    DrawRecorded { team_id: String, total: u64 },
}

/// Which count a call changes.
#[derive(Clone, Copy)]
enum Outcome {
    Win,
    Loss,
    Draw,
}

#[app::logic]
impl TeamMetricsApp {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Adds a win; returns the team's wins.
    pub fn record_win(&mut self, team_id: String) -> app::Result<u64> {
        self.record(team_id, Outcome::Win)
    }

    /// Adds a loss; returns the team's losses.
    pub fn record_loss(&mut self, team_id: String) -> app::Result<u64> {
        self.record(team_id, Outcome::Loss)
    }

    /// Adds a draw; returns the team's draws.
    pub fn record_draw(&mut self, team_id: String) -> app::Result<u64> {
        self.record(team_id, Outcome::Draw)
    }

    /// Awards badge `badge` (0-63); returns the team's mask. `BadgeOutOfRange` past 63.
    pub fn award_badge(&mut self, team_id: String, badge: u32) -> app::Result<u64> {
        if badge > 63 {
            app::bail!(Error::BadgeOutOfRange { badge });
        }
        let mut s = self.teams.get(&team_id)?.unwrap_or_default();
        s.badges |= 1_u64 << badge;
        let _old = self.teams.insert(&team_id, &s)?;
        Ok(s.badges)
    }

    /// How many badges the team holds.
    #[app::view]
    pub fn get_badge_count(&self, team_id: String) -> app::Result<u32> {
        Ok(self.stats(team_id)?.badges.count_ones())
    }

    /// The team's badge mask.
    #[app::view]
    pub fn get_badges(&self, team_id: String) -> app::Result<u64> {
        Ok(self.stats(team_id)?.badges)
    }

    /// The team's wins; `TeamNotFound` before its first record.
    #[app::view]
    pub fn get_wins(&self, team_id: String) -> app::Result<u64> {
        Ok(self.stats(team_id)?.wins)
    }

    /// The team's losses; `TeamNotFound` before its first record.
    #[app::view]
    pub fn get_losses(&self, team_id: String) -> app::Result<u64> {
        Ok(self.stats(team_id)?.losses)
    }

    /// The team's draws; `TeamNotFound` before its first record.
    #[app::view]
    pub fn get_draws(&self, team_id: String) -> app::Result<u64> {
        Ok(self.stats(team_id)?.draws)
    }

    fn stats(&self, team_id: String) -> app::Result<TeamStats> {
        match self.teams.get(&team_id)? {
            Some(s) => Ok(s),
            None => Err(Error::TeamNotFound { team_id }.into()),
        }
    }

    fn record(&mut self, team_id: String, outcome: Outcome) -> app::Result<u64> {
        let mut s = self.teams.get(&team_id)?.unwrap_or_default();
        let count = match outcome {
            Outcome::Win => &mut s.wins,
            Outcome::Loss => &mut s.losses,
            Outcome::Draw => &mut s.draws,
        };
        *count += 1;
        let total = *count;
        let _old = self.teams.insert(&team_id, &s)?;
        let event = match outcome {
            Outcome::Win => MetricsEvent::WinRecorded { team_id, total },
            Outcome::Loss => MetricsEvent::LossRecorded { team_id, total },
            Outcome::Draw => MetricsEvent::DrawRecorded { team_id, total },
        };
        env::emit(event, Audience::All)?;
        Ok(total)
    }
}
