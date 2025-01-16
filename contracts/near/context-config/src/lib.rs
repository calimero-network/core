#![allow(unused_crate_dependencies, reason = "False positives")]
#![allow(
    clippy::multiple_inherent_impl,
    reason = "Needed to separate NEAR functionality"
)]

use calimero_context_config::types::{Application, ContextId, ContextIdentity};
use calimero_context_config::Timestamp;
use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
use near_sdk::store::{IterableMap, IterableSet, LazyOption};
use near_sdk::{near, AccountId, BorshStorageKey};

mod guard;
mod mutate;
mod query;
mod sys;

use guard::Guard;

const DEFAULT_VALIDITY_THRESHOLD_MS: Timestamp = 10_000;

#[derive(Debug)]
#[near(contract_state)]
pub struct ContextConfigs {
    contexts: IterableMap<ContextId, Context>,
    config: Config,
    proxy_code: LazyOption<Vec<u8>>,
    next_proxy_id: u64,
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
    pub member_nonces: IterableMap<ContextIdentity, u64>,
    pub proxy: Guard<AccountId>,
}

#[derive(Copy, Clone, Debug, BorshSerialize, BorshDeserialize, BorshStorageKey)]
#[borsh(crate = "::near_sdk::borsh", use_discriminant = true)]
#[repr(u8)]
enum Prefix {
    Contexts = 1,
    Members(ContextId) = 2,
    Privileges(PrivilegeScope) = 3,
    ProxyCode = 4,
    MemberNonces(ContextId) = 5,
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
    Proxy,
}

impl Default for ContextConfigs {
    fn default() -> Self {
        Self {
            contexts: IterableMap::new(Prefix::Contexts),
            config: Config {
                validity_threshold_ms: DEFAULT_VALIDITY_THRESHOLD_MS,
            },
            proxy_code: LazyOption::new(Prefix::ProxyCode, None),
            next_proxy_id: 0,
        }
    }
}

macro_rules! _parse_input {
    ($input:ident $(: $input_ty:ty)?) => {
        let $input = ::near_sdk::env::input().unwrap_or_default();

        let $input $(: $input_ty )? = ::near_sdk::serde_json::from_slice(&$input).expect("failed to parse input");
    };
}

use _parse_input as parse_input;
