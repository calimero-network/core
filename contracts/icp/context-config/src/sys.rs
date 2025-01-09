use candid::{CandidType, Deserialize};

use crate::{with_state_mut, ContextConfigs, CONTEXT_CONFIGS};

#[derive(CandidType, Deserialize)]
struct StableStorage {
    saved_state: ContextConfigs,
}

#[ic_cdk::pre_upgrade]
fn pre_upgrade() {
    let state = CONTEXT_CONFIGS.with(|configs| {
        let configs = configs
            .borrow_mut()
            .take()
            .expect("cannister is being upgraded");

        if ic_cdk::api::caller() != configs.owner {
            ic_cdk::trap("unauthorized: only owner can upgrade context cannister");
        }

        StableStorage {
            saved_state: configs,
        }
    });

    // Write state to stable storage
    match ic_cdk::storage::stable_save((state,)) {
        Ok(_) => (),
        Err(err) => ic_cdk::trap(&format!("Failed to save stable storage: {}", err)),
    }
}

#[ic_cdk::post_upgrade]
fn post_upgrade() {
    // Restore the cannister state
    match ic_cdk::storage::stable_restore::<(StableStorage,)>() {
        Ok((StableStorage { saved_state },)) => {
            CONTEXT_CONFIGS.with(|configs| {
                let mut configs = configs.borrow_mut();

                if configs.is_some() {
                    ic_cdk::trap("cannister state already exists??");
                }

                *configs = Some(saved_state);
            });
        }
        Err(err) => ic_cdk::trap(&format!("Failed to restore stable storage: {}", err)),
    }
}

#[ic_cdk::update]
pub fn set_proxy_code(proxy_code: Vec<u8>) -> Result<(), String> {
    with_state_mut(|configs| {
        if ic_cdk::api::caller() != configs.owner {
            return Err("Unauthorized: only owner can set proxy code".to_string());
        }

        configs.proxy_code = Some(proxy_code);

        Ok(())
    })
}
