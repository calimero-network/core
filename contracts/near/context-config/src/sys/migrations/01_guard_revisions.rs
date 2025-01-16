use std::io;

use calimero_context_config::repr::Repr;
use calimero_context_config::types::{Application, ContextId, ContextIdentity, SignerId};
use near_sdk::store::{IterableMap, IterableSet};
use near_sdk::{env, near};

use crate::Config;

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct OldContextConfigs {
    contexts: IterableMap<ContextId, OldContext>,
    config: Config,
}

#[derive(Debug)]
#[near(serializers = [borsh])]
struct OldContext {
    pub application: OldGuard<Application<'static>>,
    pub members: OldGuard<IterableSet<ContextIdentity>>,
}

#[derive(Debug)]
#[near(serializers = [borsh])]
pub struct OldGuard<T> {
    inner: T,
    #[borsh(deserialize_with = "skipped")]
    revision: u64,
    priviledged: IterableSet<SignerId>,
}

#[expect(clippy::unnecessary_wraps, reason = "borsh needs this")]
pub fn skipped<R: io::Read>(_reader: &mut R) -> Result<u64, io::Error> {
    Ok(Default::default())
}

pub fn migrate() {
    // IterableMap doesn't support raw access to the underlying storage
    // Which hinders migration of the data, so we have to employ this trick

    let mut state = env::state_read::<OldContextConfigs>().expect("failed to read state");

    for (context_id, _) in state.contexts.iter_mut() {
        env::log_str(&format!("Migrating context `{}`", Repr::new(*context_id)));
    }
}
