use std::io;

use calimero_context_config::repr::Repr;
use calimero_context_config::types::{Application, ContextId, ContextIdentity};
use near_sdk::store::{IterableMap, IterableSet, LazyOption};
use near_sdk::{env, near, AccountId};

use crate::guard::Guard;
use crate::{Config, Prefix};

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct OldContextConfigs {
    contexts: IterableMap<ContextId, OldContext>,
    config: Config,
    proxy_code: LazyOption<Vec<u8>>,
    next_proxy_id: u64,
}

#[derive(Debug)]
#[near(serializers = [borsh])]
struct OldContext {
    pub application: Guard<Application<'static>>,
    pub members: Guard<IterableSet<ContextIdentity>>,
    #[borsh(deserialize_with = "skipped")]
    pub member_nonces: IterableMap<ContextIdentity, u64>,
    pub proxy: Guard<AccountId>,
}

#[expect(clippy::unnecessary_wraps, reason = "borsh needs this")]
pub fn skipped<R: io::Read>(
    _reader: &mut R,
) -> Result<IterableMap<ContextIdentity, u64>, io::Error> {
    Ok(IterableMap::new(&[13; 37][..]))
}

pub fn migrate() {
    let mut state = env::state_read::<OldContextConfigs>().expect("failed to read state");

    for (context_id, context) in state.contexts.iter_mut() {
        env::log_str(&format!("Migrating context `{}`", Repr::new(*context_id)));

        context.member_nonces = IterableMap::new(Prefix::MemberNonces(*context_id));

        for member in context.members.iter() {
            let _ignored = context.member_nonces.insert(*member, 0);
        }
    }
}
