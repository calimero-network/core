//! Team metrics: wins, losses and draws per team.
//!
//! In v1 this app showed `#[derive(Mergeable)]`: a struct of `Counter`s that the storage
//! layer merged field by field when replicas wrote concurrently. Here one Raft log orders
//! every call, so there is nothing to merge: `TeamStats` is a plain record of numbers, and
//! two members recording a win at the same time are two entries that each add one.

use calimero_app_sdk::prelude::*;

/// One team's statistics.
#[derive(Record)]
#[table(key = "team_id")]
pub struct TeamStats {
    pub team_id: String,
    pub wins: u64,
    pub losses: u64,
    pub draws: u64,
}

#[app::state(version = 1)]
pub struct TeamMetricsApp {
    teams: Table<TeamStats>,
}

#[app::error]
pub enum Error {
    TeamNotFound { team_id: String },
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
        let mut s = self.teams.get(&team_id)?.unwrap_or(TeamStats {
            team_id: team_id.clone(),
            wins: 0,
            losses: 0,
            draws: 0,
        });
        let (total, event) = match outcome {
            Outcome::Win => {
                s.wins += 1;
                (
                    s.wins,
                    MetricsEvent::WinRecorded {
                        team_id,
                        total: s.wins,
                    },
                )
            }
            Outcome::Loss => {
                s.losses += 1;
                (
                    s.losses,
                    MetricsEvent::LossRecorded {
                        team_id,
                        total: s.losses,
                    },
                )
            }
            Outcome::Draw => {
                s.draws += 1;
                (
                    s.draws,
                    MetricsEvent::DrawRecorded {
                        team_id,
                        total: s.draws,
                    },
                )
            }
        };
        self.teams.upsert(s)?;
        env::emit(event, Audience::All)?;
        Ok(total)
    }
}
