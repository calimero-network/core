#![allow(unused_results, unused_crate_dependencies)]

use calimero_context_config::types::{Application, ContextId, ContextIdentity};
use calimero_context_config::Timestamp;
use near_sdk::store::{IterableMap, IterableSet};
use near_sdk::{near, BorshStorageKey};

mod guard;
mod mutate;
mod query;

use guard::Guard;

const DEFAULT_VALIDITY_THRESHOLD_MS: Timestamp = 10_000;

#[derive(Debug)]
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

impl Default for ContextConfigs {
    fn default() -> Self {
        Self {
            contexts: IterableMap::new(Prefix::Contexts),
            config: Config {
                validity_threshold_ms: DEFAULT_VALIDITY_THRESHOLD_MS,
            },
        }
    }
}
