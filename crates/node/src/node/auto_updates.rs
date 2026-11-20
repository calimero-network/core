//! Updates that apply on their own (C9; docs: protocol upgrades "Updates that apply on
//! their own"): the system leader's pass, from `reconcile`, at most every
//! [`AUTO_UPDATE_EVERY`].
//!
//! Each pass first pauses the deployments' rollouts whose failures pass their threshold
//! (`PauseRollout`), then finds the apps an update would apply to on its own now
//! (`SystemState::auto_update_candidates`: a `noDestinations` policy, a newer release
//! whose hold passed since it was logged, its deployment's wave), classifies each move as
//! any upgrade does and runs the green ones (`api::upgrades::auto_upgrade`: a restore
//! point first, then `UpgradeApp { auto }`, which apply judges again). The pass's time is
//! the one the upgrade logs (`types::update_now`); nothing judges a node's clock.

use std::sync::atomic::Ordering;
use std::time::Duration;

use tracing::{info, warn};

use super::Node;
use crate::system::cluster_version::{self as cv, Feature};
use crate::system::SystemCmd;

/// How often the system leader looks for updates to apply on their own.
pub const AUTO_UPDATE_EVERY: Duration = Duration::from_secs(5);

/// The most apps one pass upgrades; the rest wait for the next.
pub const MAX_AUTO_UPDATES_PER_PASS: usize = 16;

impl Node {
    /// On the system leader: start a pass ([`Self::auto_update_once`]) in the
    /// background unless one runs or ran within [`AUTO_UPDATE_EVERY`].
    pub(super) fn auto_update(&self) {
        let now = tokio::time::Instant::now();
        {
            let Ok(mut due) = self.auto_update_due.lock() else {
                return;
            };
            if due.is_some_and(|d| now < d) {
                return;
            }
            *due = Some(now + AUTO_UPDATE_EVERY);
        }
        if self.auto_updating.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(me) = self.shared() else {
            self.auto_updating.store(false, Ordering::SeqCst);
            return;
        };
        drop(tokio::spawn(async move {
            let applied = me.auto_update_once().await;
            if applied > 0 {
                info!(applied, "updates applied on their own");
            }
            me.auto_updating.store(false, Ordering::SeqCst);
        }));
    }

    /// One pass: pause failing rollouts, then apply each green update whose hold and
    /// wave allow it. Answers how many applied. Nothing before the logged cluster version
    /// brings automatic updates and restore points.
    pub async fn auto_update_once(&self) -> usize {
        let on = self.sys(|s| {
            cv::check(
                s.cluster_version.as_deref(),
                &[
                    Feature::UpgradeClasses,
                    Feature::AutoUpdates,
                    Feature::RestorePoints,
                    Feature::Rollouts,
                ],
            )
            .is_ok()
        });
        if !on {
            return 0;
        }
        let now = crate::types::update_now();
        for (deployment, app, version) in self.sys(crate::system::SystemState::failing_rollouts) {
            let paused = self
                .sys_write(&SystemCmd::PauseRollout {
                    deployment: deployment.clone(),
                    app: app.clone(),
                    version: version.clone(),
                    at: now,
                })
                .await;
            match paused {
                Ok(_) => warn!(%deployment, %app, %version, "a rollout paused on its failures"),
                Err(e) => {
                    warn!(%deployment, %app, error = %e.message, "could not pause a failing rollout")
                }
            }
        }
        let candidates = self.sys(|s| s.auto_update_candidates(now));
        let mut applied = 0;
        for (app, to) in candidates.into_iter().take(MAX_AUTO_UPDATES_PER_PASS) {
            let class = crate::api::upgrades::move_class(self, &app.app_id, Some(&to)).await;
            if !class
                .as_ref()
                .is_some_and(crate::types::UpgradeClass::is_green)
            {
                continue;
            }
            match crate::api::upgrades::auto_upgrade(self, &app, &to).await {
                Ok(_) => {
                    applied += 1;
                    info!(app = %app.app_id, version = %to.version, "an update applied on its own");
                }
                Err(e) => {
                    warn!(app = %app.app_id, version = %to.version, error = %e.message, "an update did not apply on its own");
                }
            }
        }
        applied
    }
}
