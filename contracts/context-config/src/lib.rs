#![allow(unused_results, unused_crate_dependencies)]

use calimero_context_config::repr::{Repr, ReprBytes};
use calimero_context_config::types::{Application, ContextId, ContextIdentity, SignerId};
use near_sdk::store::{IterableMap, IterableSet};
use near_sdk::{env, near, BorshStorageKey, PanicOnDefault, Timestamp};

mod guard;
mod mutate;
mod query;

use guard::Guard;

const DEFAULT_VALIDITY_THRESHOLD_MS: Timestamp = 10_000;
const MIN_VALIDITY_THRESHOLD_MS: Timestamp = 5_000;

#[derive(Debug, PanicOnDefault)]
#[near(contract_state)]
pub struct ContextConfigs {
    contexts: IterableMap<ContextId, Context>,
    config: Config,
}

#[derive(Debug)]
#[near(serializers = [borsh])]
struct Config {
    validity_threshold_ms: Timestamp,
}

#[derive(Debug)]
#[near(serializers = [borsh])]
struct Context {
    pub application: Guard<Application<'static>>,
    pub members: Guard<IterableSet<ContextIdentity>>,
}

#[derive(Copy, Clone, Debug, BorshStorageKey)]
#[near(serializers = [borsh])]
enum Prefix {
    Contexts,
    Members(ContextId),
    Privileges(PrivilegeScope),
}

#[derive(Copy, Clone, Debug)]
#[near(serializers = [borsh])]
enum PrivilegeScope {
    Context(ContextId, ContextPrivilegeScope),
}

#[derive(Copy, Clone, Debug)]
#[near(serializers = [borsh])]
enum ContextPrivilegeScope {
    Application,
    MemberList,
}

#[near]
impl ContextConfigs {
    #[init]
    pub fn init() -> Self {
        let signer_id = SignerId::from_bytes(|buf| {
            let signer_pk = env::signer_account_pk();
            let signer_pk = &signer_pk.as_bytes()[1..];
            let len = buf.len();
            buf.copy_from_slice(&signer_pk[..len]);
            Ok(signer_pk.len())
        });

        let signer_id = match signer_id {
            Ok(signer_id) => Repr::new(signer_id),
            Err(err) => env::panic_str(&format!(
                "pweety please, sign the the contract initialization transaction with an ed25519 key: {}",
                err
            )),
        };

        env::log_str(&format!("Contract initialized by `{}`", signer_id));

        Self {
            contexts: IterableMap::new(Prefix::Contexts),
            config: Config {
                validity_threshold_ms: DEFAULT_VALIDITY_THRESHOLD_MS,
            },
        }
    }
}
