//! Team metrics with badges: wins, losses, draws and an achievement bitmask per team.
//!
//! The sibling of `team-metrics-macro`; the difference is the `badges` field, a plain `u64`
//! with one bit per badge, which `award_badge` ORs a bit into.
//!
//! What no longer exists: in v1 this app declared its own merge rule (`#[app::mergeable]`,
//! `badges |= other.badges`) so that two replicas awarding different badges concurrently
//! converged to the union instead of last-write-wins. Here one Raft log orders every call and
//! each `award_badge` reads the current mask before setting its bit, so concurrent awards
//! already end up holding both badges; there are no replicas to merge and no merge rule to
//! declare.

use calimero_app_sdk::prelude::*;

/// One team's statistics.
#[derive(Record)]
#[table(key = "team_id")]
pub struct TeamStats {
    pub team_id: String,
    pub wins: u64,
    pub losses: u64,
    pub draws: u64,
    /// Achievement bitmask, one bit per badge (0-63).
    pub badges: u64,
}

#[app::state(version = 1)]
pub struct TeamMetricsApp {
    teams: Table<TeamStats>,
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
        let mut s = self.stats_or_new(team_id)?;
        s.badges |= 1_u64 << badge;
        let badges = s.badges;
        self.teams.upsert(s)?;
        Ok(badges)
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

    fn stats_or_new(&self, team_id: String) -> app::Result<TeamStats> {
        Ok(self.teams.get(&team_id)?.unwrap_or(TeamStats {
            team_id,
            wins: 0,
            losses: 0,
            draws: 0,
            badges: 0,
        }))
    }

    fn record(&mut self, team_id: String, outcome: Outcome) -> app::Result<u64> {
        let mut s = self.stats_or_new(team_id.clone())?;
        let (total, event) = match outcome {
            Outcome::Win => {
                s.wins += 1;
                let total = s.wins;
                (total, MetricsEvent::WinRecorded { team_id, total })
            }
            Outcome::Loss => {
                s.losses += 1;
                let total = s.losses;
                (total, MetricsEvent::LossRecorded { team_id, total })
            }
            Outcome::Draw => {
                s.draws += 1;
                let total = s.draws;
                (total, MetricsEvent::DrawRecorded { team_id, total })
            }
        };
        self.teams.upsert(s)?;
        env::emit(event, Audience::All)?;
        Ok(total)
    }
}
