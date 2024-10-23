use std::collections::HashSet;

use near_sdk::json_types::{Base64VecU8, U128, U64};
use near_sdk::store::IterableMap;
use near_sdk::{env, log, near, AccountId, Gas, PanicOnDefault, Promise, PromiseError};
use calimero_context_config::repr::Repr;
use calimero_context_config::types::{ContextId, ContextIdentity, Signed, SignerId};

pub mod ext_config;
pub use crate::ext_config::config_contract;

pub type RequestId = u32;

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct ProxyContract {
    pub context_id: Repr<ContextId>,
    pub context_config_account_id: AccountId,
    pub num_confirmations: u32,
    pub request_nonce: RequestId,
    pub requests: IterableMap<RequestId, MultiSigRequestWithSigner>,
    pub confirmations: IterableMap<RequestId, HashSet<Repr<SignerId>>>,
    pub num_requests_pk: IterableMap<Repr<SignerId>, u32>,
    pub active_requests_limit: u32,
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh])]
pub struct FunctionCallPermission {
    allowance: Option<U128>,
    receiver_id: AccountId,
    method_names: Vec<String>,
}

// An internal request wrapped with the signer_pk and added timestamp to determine num_requests_pk and prevent against malicious key holder gas attacks
#[derive(Clone, PartialEq)]
#[near(serializers = [json, borsh])]
pub struct MultiSigRequestWithSigner {
    pub request: MultiSigRequest,
    pub signer_id: Repr<SignerId>,
}

// An internal request wrapped with the signer_pk and added timestamp to determine num_requests_pk and prevent against malicious key holder gas attacks
#[derive(Clone, PartialEq)]
#[near(serializers = [json, borsh])]
pub struct ConfirmationRequestWithSigner {
    request_id: RequestId,
    signer_id: Repr<SignerId>,
    added_timestamp: u64,
}


/// Lowest level action that can be performed by the multisig contract.
#[derive(Clone, PartialEq)]
#[near(serializers = [json, borsh])]
pub enum MultiSigRequestAction {
    /// Call function on behalf of this contract.
    FunctionCall {
        method_name: String,
        args: Base64VecU8,
        deposit: U128,
        gas: U64,
    },
    SetNumConfirmations { num_confirmations: u32 },
    SetActiveRequestsLimit { active_requests_limit: u32 },
}

// The request the user makes specifying the receiving account and actions they want to execute (1 tx)
#[derive(Clone, PartialEq)]
#[near(serializers = [json, borsh])]
pub struct MultiSigRequest {
    pub receiver_id: AccountId,
    pub actions: Vec<MultiSigRequestAction>,
}

#[near]
impl ProxyContract {
    #[init]
    pub fn init(context_id: Repr<ContextId>, context_config_account_id: AccountId) -> Self {
        assert!(!env::state_exists(), "Already initialized");
        Self {
            context_id, 
            context_config_account_id,
            request_nonce: 0,
            requests: IterableMap::new(b"r".to_vec()),
            confirmations: IterableMap::new(b"c".to_vec()),
            num_requests_pk: IterableMap::new(b"k".to_vec()),
            num_confirmations: 2,
            active_requests_limit: 10,
        }
    }

    /// Add request for multisig.
    pub fn add_request(&mut self, request: Signed<MultiSigRequestWithSigner> ) -> RequestId {
        // Verify the signature corresponds to the signer_id
        let request = request
            .parse(|i| *i.signer_id)
            .expect("failed to parse input");

        let singer_id = &request.signer_id;

        // track how many requests this key has made
        let num_requests = self
            .num_requests_pk
            .get(singer_id)
            .unwrap_or(&0)
            + 1;
        assert!(
            num_requests <= self.active_requests_limit,
            "Account has too many active requests. Confirm or delete some."
        );
        self.num_requests_pk
            .insert(singer_id.clone(), num_requests);
        // add the request
        self.requests.insert(self.request_nonce, request.clone());
        self.confirmations
            .insert(self.request_nonce,  HashSet::new());
        self.request_nonce += 1;
        self.request_nonce - 1
    }

    /// Add request for multisig and confirm with the pk that added.
    pub fn add_request_and_confirm(&mut self, request: Signed<MultiSigRequestWithSigner>) -> RequestId {
        let request_id = self.add_request(request.clone());
        let request = request
            .parse(|i| *i.signer_id)
            .expect("failed to parse input");
        self.internal_confirm(request_id, request.signer_id);
        request_id
    }

    pub fn confirm(&mut self, request: Signed<ConfirmationRequestWithSigner>) -> () {
        // self.assert_valid_request(request_id);
                // Verify the signature corresponds to the signer_id
        let request = request
                .parse(|i| *i.signer_id)
                .expect("failed to parse input");
    
    
        self.internal_confirm(request.request_id, request.signer_id);
    }

    fn internal_confirm(&mut self, request_id: RequestId, signer_id: Repr<SignerId>) -> () {
        let confirmations = self.confirmations.get_mut(&request_id).unwrap();
        assert!(
            !confirmations.contains(&signer_id),
            "Already confirmed this request with this key"
        );
        confirmations.insert(signer_id);
    }

    pub fn fetch_members(
        &self,
    ) -> Promise {
        log!("Starting fetch_members...");
        config_contract::ext(self.context_config_account_id.clone())
            .with_static_gas(Gas::from_tgas(5))
            .members(self.context_id, 0, 10)
            .then(
               Self::ext(env::current_account_id()).internal_process_members()
            )
    }

    #[private]
    pub fn internal_process_members(
        &mut self,
        #[callback_result] call_result: Result<Vec<Repr<ContextIdentity>>, PromiseError>,  // Match the return type
    ) -> Vec<Repr<ContextIdentity>> {
        if call_result.is_err() {
            log!("fetch_members failed...");
            return [].to_vec();
        } else {
            log!("fetch_members was successful!");
            return call_result.unwrap();
        }
    }
}
