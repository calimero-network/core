use near_sdk::store::{IterableMap, IterableSet};
use near_sdk::{env, near, BorshStorageKey, PanicOnDefault, Timestamp};

use super::types::{Application, ContextId, ContextIdentity};
use crate::repr::{Repr, ReprBytes};
use crate::types::SignerId;

mod guard;
mod mutate;
mod query;

use guard::Guard;

const DEFAULT_VALIDITY_THRESHOLD_MS: Timestamp = 10_000;

#[derive(Debug, PanicOnDefault)]
#[near(contract_state)]
pub struct ContextConfig {
    contexts: IterableMap<ContextId, Context>,
    config: Guard<Config>,
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
    Application(ContextId),
    MemberList(ContextId),
    Config,
}

#[near]
impl ContextConfig {
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
            Ok(signer_id) => signer_id,
            Err(err) => env::panic_str(&format!(
                "pweety please, sign the the contract initialization transaction with an ed25519 key: {}",
                err
            )),
        };

        env::log_str(&format!(
            "Contract initialized by `{}`",
            Repr::new(signer_id)
        ));

        Self {
            contexts: IterableMap::new(Prefix::Contexts),
            config: Guard::new(
                Prefix::Privileges(PrivilegeScope::Config),
                signer_id,
                Config {
                    validity_threshold_ms: DEFAULT_VALIDITY_THRESHOLD_MS,
                },
            ),
        }
    }
}
