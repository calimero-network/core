//! A deployment's rollout of the release it offers: the waves its instances on an
//! automatic update policy follow (`SystemCmd::SetRollout`, `SystemCmd::PauseRollout`,
//! [`SystemState::rollout_admits`]).
//!
//! An offer of a new release opens wave 0 at the offer's logged time
//! (`Offer::added_at`); each wave admits the first [`WAVES`] percent of the instances
//! running the offered package (ordered by `SHA-256(version, app id)`, so each release
//! reaches a different first few), and the next opens `wave_hours` later. A paused
//! rollout admits nobody new; resuming opens the next wave a full `wave_hours` after the
//! resume. Everything is judged on logged times (the offer's, the upgrade's `at`), never
//! a node's clock, and only an instance that would apply on its own waits for its wave:
//! an owner's or admin's approval applies whenever they give it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{AccountId, AppRecord, Deployment, Offer, SystemState};

/// Each wave's share of the instances, in percent: the last is everyone.
pub const WAVES: [u64; 4] = [1, 10, 50, 100];
/// How long a wave runs before the next opens, unless the deployment sets its own.
pub const DEFAULT_WAVE_HOURS: u32 = 24;
/// The shortest and longest pause between waves a deployment may set.
pub const MIN_WAVE_HOURS: u32 = 1;
pub const MAX_WAVE_HOURS: u32 = 168;
/// A rollout pauses on its own once its failed upgrades and undos are more than this
/// percent of the instances that tried the release (and at least one).
pub const ROLLOUT_FAILURE_PERCENT: u64 = 10;
/// Who [`SystemCmd::PauseRollout`](super::SystemCmd::PauseRollout) names as having
/// paused: the failures, not a person.
pub const PAUSED_BY_FAILURES: &str = "failures";

/// A deployment's rollout of one offered app, as its managers left it
/// (`SystemState::rollouts`). Absent: the defaults (wave 0 at the offer, a day apart,
/// not paused).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RolloutState {
    /// The offered version the wave and pause below are of; a newer offer starts at
    /// wave 0 again, keeping `wave_hours`.
    pub version: String,
    pub wave_hours: u32,
    /// The wave open at `base_at`; later ones open `wave_hours` apart from there.
    pub base_wave: u8,
    pub base_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<RolloutPause>,
}

/// Why and when a rollout stopped admitting instances.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RolloutPause {
    pub at: u64,
    /// A manager's account, or [`PAUSED_BY_FAILURES`].
    pub by: AccountId,
    /// The failures counted when it paused on its own.
    #[serde(default)]
    pub failures: u64,
}

/// How the instances of an offered release fare: those that run it, those whose
/// upgrade to it failed and those that undid it, out of `tried`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RolloutHealth {
    pub instances: u64,
    pub updated: u64,
    pub failed: u64,
    pub undone: u64,
    pub tried: u64,
}

impl RolloutHealth {
    /// Whether the failures pass [`ROLLOUT_FAILURE_PERCENT`] of the instances that tried.
    #[must_use]
    pub const fn failing(&self) -> bool {
        let failures = self.failed + self.undone;
        failures > 0 && failures * 100 > self.tried * ROLLOUT_FAILURE_PERCENT
    }
}

/// How many of `n` instances waves `0..=wave` admit together: at least one.
#[must_use]
pub fn admitted(n: usize, wave: u8) -> usize {
    let pct = WAVES[usize::from(wave).min(WAVES.len() - 1)];
    let n = n as u64;
    usize::try_from(n.saturating_mul(pct).div_ceil(100).max(1).min(n)).unwrap_or(usize::MAX)
}

/// The last wave's index.
#[must_use]
pub const fn last_wave() -> u8 {
    (WAVES.len() - 1) as u8
}

impl RolloutState {
    /// The defaults for `offer`'s release, keeping `wave_hours` from an earlier one.
    #[must_use]
    pub fn fresh(offer: &Offer, wave_hours: u32) -> Self {
        Self {
            version: offer.version.clone(),
            wave_hours,
            base_wave: 0,
            base_at: offer.added_at,
            paused: None,
        }
    }

    /// The wave open at `at` (the last once all are), judged from `base_at`; while
    /// paused, the one open when it paused.
    #[must_use]
    pub fn wave_at(&self, at: u64) -> u8 {
        if self.paused.is_some() {
            return self.base_wave;
        }
        let secs = u64::from(self.wave_hours.max(1)) * 3600;
        let passed = at.saturating_sub(self.base_at) / secs;
        let wave = u64::from(self.base_wave).saturating_add(passed);
        u8::try_from(wave.min(u64::from(last_wave()))).unwrap_or(last_wave())
    }

    /// When the wave after the one open at `at` opens; `None` once the last is open or
    /// while paused.
    #[must_use]
    pub fn next_wave_at(&self, at: u64) -> Option<u64> {
        let wave = self.wave_at(at);
        if self.paused.is_some() || wave >= last_wave() {
            return None;
        }
        let secs = u64::from(self.wave_hours.max(1)) * 3600;
        let steps = u64::from(wave - self.base_wave) + 1;
        Some(self.base_at.saturating_add(steps.saturating_mul(secs)))
    }
}

impl SystemState {
    /// The rollout of `deployment`'s `app` as it stands for the release offered now.
    #[must_use]
    pub fn rollout_of(&self, deployment: &str, app: &str) -> Option<(Offer, RolloutState)> {
        let offer = self.deployments.get(deployment)?.apps.get(app)?.clone();
        let kept = self.rollouts.get(deployment).and_then(|r| r.get(app));
        let state = match kept {
            Some(r) if r.version == offer.version => r.clone(),
            Some(r) => RolloutState::fresh(&offer, r.wave_hours),
            None => RolloutState::fresh(&offer, DEFAULT_WAVE_HOURS),
        };
        Some((offer, state))
    }

    /// The instances of `deployment` that run `offer`'s package, in the order its
    /// release's waves reach them.
    #[must_use]
    pub fn rollout_order(&self, deployment: &str, offer: &Offer) -> Vec<&AppRecord> {
        let mut apps: Vec<(String, &AppRecord)> = self
            .apps
            .values()
            .filter(|a| a.deployment.as_deref() == Some(deployment))
            .filter(|a| self.runs_package(a, &offer.package))
            .map(|a| {
                let mut h = ring::digest::Context::new(&ring::digest::SHA256);
                h.update(offer.version.as_bytes());
                h.update(&[0]);
                h.update(a.app_id.as_bytes());
                (hex::encode(h.finish()), a)
            })
            .collect();
        apps.sort_by(|x, y| x.0.cmp(&y.0));
        apps.into_iter().map(|(_, a)| a).collect()
    }

    /// Whether `app` runs a release of `package` (or did, by its bundle, before releases
    /// were recorded).
    fn runs_package(&self, app: &AppRecord, package: &str) -> bool {
        app.release
            .as_ref()
            .or_else(|| self.governing_release(app))
            .is_some_and(|r| r.package == package)
    }

    /// The offered app name of `deployment` whose package `app` runs.
    #[must_use]
    pub fn offered_app_of(&self, d: &Deployment, app: &AppRecord) -> Option<String> {
        d.apps
            .iter()
            .find(|(_, o)| self.runs_package(app, &o.package))
            .map(|(name, _)| name.clone())
    }

    /// Whether `app`, an instance of a deployment, may move on its own to the release
    /// its deployment offers at `at`: the rollout is not paused and its wave reaches
    /// the instance.
    #[must_use]
    pub fn rollout_admits(&self, app: &AppRecord, at: u64) -> bool {
        let Some(d) = app
            .deployment
            .as_ref()
            .and_then(|d| self.deployments.get(d))
        else {
            return false;
        };
        let Some(name) = self.offered_app_of(d, app) else {
            return false;
        };
        let Some((offer, state)) = self.rollout_of(&d.id, &name) else {
            return false;
        };
        if state.paused.is_some() {
            return false;
        }
        let order = self.rollout_order(&d.id, &offer);
        let reach = admitted(order.len(), state.wave_at(at));
        order.iter().take(reach).any(|a| a.app_id == app.app_id)
    }

    /// How the instances of `deployment`'s `offer` fare with its release.
    #[must_use]
    pub fn rollout_health(&self, deployment: &str, offer: &Offer) -> RolloutHealth {
        let mut h = RolloutHealth::default();
        for a in self.rollout_order(deployment, offer) {
            h.instances += 1;
            let runs = a
                .release
                .as_ref()
                .is_some_and(|r| r.package == offer.package && r.version == offer.version);
            let is_offer = |r: &Option<super::PackageRef>| {
                r.as_ref()
                    .is_some_and(|r| r.package == offer.package && r.version == offer.version)
            };
            let last = a.last_upgrade.as_ref();
            let failed = last.is_some_and(|u| !u.ok && is_offer(&u.to_release));
            let undone = last.is_some_and(|u| u.undo && is_offer(&u.from_release));
            if runs {
                h.updated += 1;
            }
            if failed {
                h.failed += 1;
            }
            if undone {
                h.undone += 1;
            }
            if runs || failed || undone {
                h.tried += 1;
            }
        }
        h
    }

    /// A deployment's rollouts as its managers read them (`GET
    /// /deployments/{id}/rollout`), judged at `now`.
    #[must_use]
    pub fn rollout_json(&self, deployment: &str, now: u64) -> Value {
        let Some(d) = self.deployments.get(deployment) else {
            return Value::Null;
        };
        let apps: Vec<Value> = d
            .apps
            .keys()
            .filter_map(|name| {
                let (offer, state) = self.rollout_of(deployment, name)?;
                let order = self.rollout_order(deployment, &offer);
                let health = self.rollout_health(deployment, &offer);
                let wave = state.wave_at(now);
                let secs = u64::from(state.wave_hours.max(1)) * 3600;
                let waves: Vec<Value> = (0..=last_wave())
                    .map(|w| {
                        let opens = (w > state.base_wave).then(|| {
                            state
                                .base_at
                                .saturating_add(u64::from(w - state.base_wave) * secs)
                        });
                        json!({
                            "percent": WAVES[usize::from(w)],
                            "instances": admitted(order.len(), w),
                            "open": w <= wave,
                            "opensAt": if w <= wave || state.paused.is_some() { None } else { opens },
                        })
                    })
                    .collect();
                Some(json!({
                    "app": name,
                    "package": offer.package,
                    "version": offer.version,
                    "offeredAt": offer.added_at,
                    "waveHours": state.wave_hours,
                    "wave": wave,
                    "waves": waves,
                    "nextWaveAt": state.next_wave_at(now),
                    "paused": state.paused,
                    "instances": health.instances,
                    "updated": health.updated,
                    "failed": health.failed,
                    "undone": health.undone,
                    "pausesAbovePercent": ROLLOUT_FAILURE_PERCENT,
                }))
            })
            .collect();
        json!({ "deployment": deployment, "apps": apps })
    }

    /// Keep `state` as `deployment`'s rollout of `app`, dropping kept rollouts of apps
    /// it no longer offers.
    pub(super) fn keep_rollout(&mut self, deployment: &str, app: &str, state: RolloutState) {
        let offered: Vec<String> = self
            .deployments
            .get(deployment)
            .map(|d| d.apps.keys().cloned().collect())
            .unwrap_or_default();
        let kept = self.rollouts.entry(deployment.to_owned()).or_default();
        let _prev = kept.insert(app.to_owned(), state);
        kept.retain(|name, _| offered.contains(name));
        self.rollouts
            .retain(|id, _| self.deployments.contains_key(id));
    }
}

/// The rollouts a cluster keeps, by deployment, then offered app.
pub type Rollouts = BTreeMap<String, BTreeMap<String, RolloutState>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waves_admit_a_growing_share_and_never_none() {
        assert_eq!(
            (0..=3).map(|w| admitted(1000, w)).collect::<Vec<_>>(),
            [10, 100, 500, 1000]
        );
        assert_eq!(
            (0..=3).map(|w| admitted(3, w)).collect::<Vec<_>>(),
            [1, 1, 2, 3]
        );
        assert_eq!(admitted(0, 3), 0);
    }

    #[test]
    fn a_wave_opens_every_wave_hours_from_its_base_and_stops_while_paused() {
        let mut r = RolloutState {
            version: "1".into(),
            wave_hours: 2,
            base_wave: 0,
            base_at: 100,
            paused: None,
        };
        assert_eq!(r.wave_at(100), 0);
        assert_eq!(r.wave_at(100 + 7200 - 1), 0);
        assert_eq!(r.wave_at(100 + 7200), 1);
        assert_eq!(r.next_wave_at(100 + 7200), Some(100 + 2 * 7200));
        assert_eq!(r.wave_at(u64::MAX), 3);
        assert_eq!(r.next_wave_at(u64::MAX), None);
        r.paused = Some(RolloutPause {
            at: 7300,
            by: "m".into(),
            failures: 0,
        });
        r.base_wave = 1;
        assert_eq!(r.wave_at(u64::MAX), 1);
        assert_eq!(r.next_wave_at(10_000), None);
    }
}
