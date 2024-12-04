use candid::{CandidType, Deserialize, Principal};
use ic_cdk;

use crate::CONTEXT_CONFIGS;

#[derive(CandidType, Deserialize)]
struct StableStorage {
    configs: crate::ContextConfigs,
}

#[ic_cdk::pre_upgrade]
fn pre_upgrade() {
    // Verify caller is the owner
    CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        if ic_cdk::api::caller() != configs.owner {
            ic_cdk::trap("unauthorized: only owner can upgrade context contract");
        }
    });

    // Store the contract state
    let state = CONTEXT_CONFIGS.with(|configs| StableStorage {
        configs: configs.borrow().clone(),
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
        Ok((state,)) => {
            CONTEXT_CONFIGS.with(|configs| {
                *configs.borrow_mut() = state.configs;
            });
        }
        Err(err) => ic_cdk::trap(&format!("Failed to restore stable storage: {}", err)),
    }
}

#[ic_cdk::update]
pub fn set_proxy_code(proxy_code: Vec<u8>, ledger_id: Principal) -> Result<(), String> {
    CONTEXT_CONFIGS.with(|configs| {
        let mut configs = configs.borrow_mut();

        // Check if caller is the owner
        if ic_cdk::api::caller() != configs.owner {
            return Err("Unauthorized: only owner can set proxy code".to_string());
        }

        configs.ledger_id = ledger_id;
        configs.proxy_code = Some(proxy_code);
        Ok(())
    })
}
