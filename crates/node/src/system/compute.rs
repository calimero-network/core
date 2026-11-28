//! System commands for the compute queue and file-byte charges.
//!
//! [`authorize`] judges the proof each carries and [`apply`] applies it;
//! `system.rs` sends a command here by its [`super::Area`]. Each arm of
//! [`apply`] is one call to a function of its own, in the order of the arms.
//! `AcquireRun` answers its refusals as codes in its result
//! (`acquire_refused`), not as a [`Rejection`].

#[allow(
    clippy::wildcard_imports,
    reason = "the arms moved here from `system.rs` read its items"
)]
use super::*;

/// Judge the proof a command of this area carries ([`super::authorize`]):
/// none carries one.
pub(super) fn authorize(proofs: &Proofs<'_>, cmd: &SystemCmd) -> Result<Option<String>, Rejection> {
    let _ = (proofs, cmd);
    Ok(None)
}

/// Apply a command of this area by its own rules, its proof already
/// judged ([`SystemMachine::run_authorized`]).
pub(super) fn apply(m: &mut SystemMachine, cmd: SystemCmd) -> Result<Value, Rejection> {
    let state = &mut m.state;
    match cmd {
        SystemCmd::ChargeFileBytes { app_id, delta } => charge_file_bytes(state, &app_id, delta),
        SystemCmd::AcquireRun {
            app,
            run,
            payer,
            fuel,
            memory_mib,
            pool: named_pool,
            sandbox,
        } => acquire_run(
            state, app, run, payer, fuel, memory_mib, named_pool, sandbox,
        ),
        SystemCmd::ReleaseRun { app, run } => release_run(state, &app, run),
        // SAFETY: `SystemCmd::area` sends a command only to the module of
        // its area, and this match has an arm for each command of it.
        _ => unreachable!("a command of another area reached compute::apply"),
    }
}

/// Charge (or, with a negative `delta`, refund) an app's file bytes; a charge
/// stays within its payer's file-byte quota.
fn charge_file_bytes(
    state: &mut SystemState,
    app_id: &str,
    delta: i64,
) -> Result<Value, Rejection> {
    let app = state.apps.get(app_id).ok_or(NO_SUCH_APP)?;
    let held = state.app_file_bytes.get(app_id).copied().unwrap_or(0);
    let now = if delta >= 0 {
        let payer = crate::usage::payer(app);
        if state
            .file_bytes(&payer)
            .saturating_add(delta.unsigned_abs())
            > state.payer_quotas(&payer).2
        {
            return Err(Rejection::quota("file bytes"));
        }
        held.saturating_add(delta.unsigned_abs())
    } else {
        held.saturating_sub(delta.unsigned_abs())
    };
    state.set_app_file_bytes(app_id, now);
    Ok(json!({ "fileBytes": now }))
}

/// Queue a compute run (once: a known run is answered as it stands) and run
/// the grant pass; answer whether it is granted, queued or refused.
fn acquire_run(
    state: &mut SystemState,
    app: AppId,
    run: String,
    payer: String,
    fuel: u64,
    memory_mib: u32,
    named_pool: Option<String>,
    sandbox: bool,
) -> Result<Value, Rejection> {
    let Some(record) = state.apps.get(&app) else {
        return Ok(acquire_refused(RUN_NO_SUCH_APP, false));
    };
    if state.run_slot(&app, &run).is_none() {
        let pool = match named_pool {
            Some(name) => state.named_pool_for(name, memory_mib),
            None => state.pool_for(memory_mib, sandbox),
        };
        let app_payer = crate::usage::payer(record);
        let refusal = acquire_refusal(state, &app, &run, &app_payer, &payer, fuel, pool.is_some());
        if let Some((code, retryable)) = refusal {
            return Ok(acquire_refused(code, retryable));
        }
        let seq = state.next_run_seq;
        state.next_run_seq += 1;
        let _prev = state.compute.entry(app.clone()).or_default().insert(
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
        state.grant_runs();
    }
    let slot = state.run_slot(&app, &run);
    if slot.is_some_and(|slot| slot.refused) {
        return Ok(acquire_refused(RUN_FUEL_OVER_QUOTA, false));
    }
    let granted = slot.is_some_and(|slot| slot.granted);
    Ok(json!({
        "state": if granted { "granted" } else { "queued" },
        "code": null,
        "retryable": false,
    }))
}

/// Why a new run is not queued, as `(code, retryable)`, judged in this order:
/// the payer is not the app's, its fuel passes a run's most or the payer's
/// quota, no pool fits it, its id was released before, or the app's queue is full.
fn acquire_refusal(
    state: &SystemState,
    app: &str,
    run: &str,
    app_payer: &str,
    payer: &str,
    fuel: u64,
    fits_a_pool: bool,
) -> Option<(&'static str, bool)> {
    if payer != app_payer {
        Some((RUN_NOT_PAYER, true))
    } else if fuel > calimero_app_host::MAX_COMPUTE_FUEL {
        Some((RUN_FUEL_OVER_MAX, false))
    } else if fuel > state.payer_quotas(payer).1 {
        Some((RUN_FUEL_OVER_QUOTA, false))
    } else if !fits_a_pool {
        Some((RUN_NO_POOL, false))
    } else if state
        .released_runs
        .get(app)
        .is_some_and(|released| released.iter().any(|id| id == run))
    {
        Some((RUN_RELEASED, false))
    } else if state.compute.get(app).map_or(0, BTreeMap::len) >= calimero_app_host::MAX_ACTIVE_RUNS
    {
        Some((RUN_APP_QUEUE_FULL, true))
    } else {
        None
    }
}

/// Release a run's slot (a run never queued is released too), remember its
/// id among the app's last [`RELEASED_RUNS_KEPT`], and run the grant pass.
fn release_run(state: &mut SystemState, app: &str, run: String) -> Result<Value, Rejection> {
    if !state.apps.contains_key(app) {
        return Ok(json!({ "released": false }));
    }
    let released = state
        .compute
        .get_mut(app)
        .is_some_and(|slots| slots.remove(&run).is_some());
    let ring = state.released_runs.entry(app.to_owned()).or_default();
    if !ring.contains(&run) {
        ring.push_back(run);
    }
    while ring.len() > RELEASED_RUNS_KEPT {
        let _old = ring.pop_front();
    }
    if released {
        if state.compute.get(app).is_some_and(BTreeMap::is_empty) {
            let _gone = state.compute.remove(app);
        }
        state.grant_runs();
    }
    Ok(json!({ "released": released }))
}
