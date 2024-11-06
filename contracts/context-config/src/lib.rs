#![allow(unused_crate_dependencies, reason = "False positives")]
#![allow(
    clippy::multiple_inherent_impl,
    reason = "Needed to separate NEAR functionality"
)]

use calimero_context_config::types::{Application, ContextId, ContextIdentity};
use calimero_context_config::Timestamp;
use near_sdk::store::{IterableMap, IterableSet, LazyOption};
use near_sdk::{near, BorshStorageKey};
mod guard;
mod mutate;
mod query;
mod sys;

use guard::Guard;

const DEFAULT_VALIDITY_THRESHOLD_MS: Timestamp = 10_000;
const DEFAULT_CONTRACT: &[u8] = include_bytes!("../../proxy-lib/res/proxy_lib.wasm");

#[derive(Debug)]
#[near(contract_state)]
pub struct ContextConfigs {
    contexts: IterableMap<ContextId, Context>,
    config: Config,
    proxy_code: LazyOption<Vec<u8>>,
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
    pub proxy: Option<String>,
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
            proxy_code: LazyOption::new("code".as_bytes(), Some(DEFAULT_CONTRACT.to_vec())),
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
