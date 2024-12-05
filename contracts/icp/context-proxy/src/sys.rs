use candid::{CandidType, Deserialize, Principal};
use ic_cdk;

use crate::{ICProxyContract, PROXY_CONTRACT};

#[derive(CandidType, Deserialize)]
struct StableStorage {
    proxy_contract: ICProxyContract,
}

#[ic_cdk::pre_upgrade]
fn pre_upgrade() {
    // Verify caller is the context contract that created this proxy
    let caller = ic_cdk::caller();
    let context_canister = PROXY_CONTRACT.with(|contract| {
        Principal::from_text(&contract.borrow().context_config_id)
            .expect("Invalid context canister ID")
    });

    if caller != context_canister {
        ic_cdk::trap("unauthorized: only context contract can upgrade proxy");
    }

    // Store the contract state
    let state = PROXY_CONTRACT.with(|contract| StableStorage {
        proxy_contract: contract.borrow().clone(),
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
            PROXY_CONTRACT.with(|contract| {
                *contract.borrow_mut() = state.proxy_contract;
            });
        }
        Err(err) => ic_cdk::trap(&format!("Failed to restore stable storage: {}", err)),
    }
}
