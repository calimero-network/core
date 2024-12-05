use candid::{CandidType, Deserialize};

use crate::{ICProxyContract, PROXY_CONTRACT};

#[derive(CandidType, Deserialize)]
struct StableStorage {
    saved_state: ICProxyContract,
}

#[ic_cdk::pre_upgrade]
fn pre_upgrade() {
    let state = PROXY_CONTRACT.with(|state| {
        let state = state
            .borrow_mut()
            .take()
            .expect("cannister is being upgraded");

        if ic_cdk::caller() != state.context_config_id {
            ic_cdk::trap("unauthorized: only context contract can upgrade proxy");
        }

        StableStorage { saved_state: state }
    });

    // Write state to stable storage
    match ic_cdk::storage::stable_save((state,)) {
        Ok(_) => (),
        Err(err) => ic_cdk::trap(&format!("Failed to save stable storage: {}", err)),
    }
}

#[ic_cdk::post_upgrade]
fn post_upgrade() {
    // Restore the contract state
    match ic_cdk::storage::stable_restore::<(StableStorage,)>() {
        Ok((StableStorage { saved_state },)) => {
            PROXY_CONTRACT.with(|state| {
                let mut state = state.borrow_mut();

                if state.is_some() {
                    ic_cdk::trap("cannister state already exists??");
                }

                *state = Some(saved_state);
            });
        }
        Err(err) => ic_cdk::trap(&format!("Failed to restore stable storage: {}", err)),
    }
}
