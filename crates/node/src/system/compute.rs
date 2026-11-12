//! System commands for the compute queue and file-byte charges.
//!
//! [`authorize`] judges the proof each carries and [`apply`] applies it;
//! `system.rs` sends a command here by its [`super::Area`].

#[allow(
    clippy::wildcard_imports,
    reason = "the arms moved here from `system.rs` read its items"
)]
use super::*;

/// Judge the proof a command of this area carries ([`super::authorize`]).
pub(super) fn authorize(
    proofs: &Proofs<'_>,
    cmd: &SystemCmd,
) -> Result<Option<String>, &'static str> {
    let _ = (proofs, cmd);
    Ok(None)
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
#[allow(clippy::too_many_lines, reason = "one arm per command")]
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, &'static str> {
    let s = &mut m.state;
    match cmd {
        SystemCmd::ChargeFileBytes { app_id, delta } => {
            let app = s.apps.get(&app_id).ok_or("no such app")?;
            let held = s.app_file_bytes.get(&app_id).copied().unwrap_or(0);
            let now = if delta >= 0 {
                let payer = crate::usage::payer(app);
                if s.file_bytes(&payer).saturating_add(delta.unsigned_abs())
                    > s.payer_quotas(&payer).2
                {
                    return Err("quota exceeded: file bytes");
                }
                held.saturating_add(delta.unsigned_abs())
            } else {
                held.saturating_sub(delta.unsigned_abs())
            };
            s.set_app_file_bytes(&app_id, now);
            Ok(json!({ "fileBytes": now }))
        }
        SystemCmd::AcquireRun {
            app,
            run,
            payer,
            fuel,
            memory_mib,
        } => {
            let Some(record) = s.apps.get(&app) else {
                return Ok(acquire_refused(RUN_NO_SUCH_APP, false));
            };
            if s.run_slot(&app, &run).is_none() {
                let pool = s.pool_for(memory_mib);
                let refusal = if payer != crate::usage::payer(record) {
                    Some((RUN_NOT_PAYER, true))
                } else if fuel > calimero_app_host::MAX_COMPUTE_FUEL {
                    Some((RUN_FUEL_OVER_MAX, false))
                } else if fuel > s.payer_quotas(&payer).1 {
                    Some((RUN_FUEL_OVER_QUOTA, false))
                } else if pool.is_none() {
                    Some((RUN_NO_POOL, false))
                } else if s.released_runs.get(&app).is_some_and(|r| r.contains(&run)) {
                    Some((RUN_RELEASED, false))
                } else if s.compute.get(&app).map_or(0, BTreeMap::len)
                    >= calimero_app_host::MAX_ACTIVE_RUNS
                {
                    Some((RUN_APP_QUEUE_FULL, true))
                } else {
                    None
                };
                if let Some((code, retryable)) = refusal {
                    return Ok(acquire_refused(code, retryable));
                }
                let seq = s.next_run_seq;
                s.next_run_seq += 1;
                let _prev = s.compute.entry(app.clone()).or_default().insert(
                    run.clone(),
                    RunSlot {
                        payer,
                        fuel,
                        seq,
                        granted: false,
                        refused: false,
                        pool: pool.unwrap_or_default(),
                    },
                );
                s.grant_runs();
            }
            let slot = s.run_slot(&app, &run);
            if slot.is_some_and(|r| r.refused) {
                return Ok(acquire_refused(RUN_FUEL_OVER_QUOTA, false));
            }
            let granted = slot.is_some_and(|r| r.granted);
            Ok(json!({
                "state": if granted { "granted" } else { "queued" },
                "code": null,
                "retryable": false,
            }))
        }
        SystemCmd::ReleaseRun { app, run } => {
            if !s.apps.contains_key(&app) {
                return Ok(json!({ "released": false }));
            }
            let released = s
                .compute
                .get_mut(&app)
                .is_some_and(|r| r.remove(&run).is_some());
            let ring = s.released_runs.entry(app.clone()).or_default();
            if !ring.contains(&run) {
                ring.push_back(run.clone());
            }
            while ring.len() > RELEASED_RUNS_KEPT {
                let _old = ring.pop_front();
            }
            if released {
                if s.compute.get(&app).is_some_and(BTreeMap::is_empty) {
                    let _gone = s.compute.remove(&app);
                }
                s.grant_runs();
            }
            Ok(json!({ "released": released }))
        }
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached compute::apply"),
    }
}
