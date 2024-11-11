use core::time;
use std::io;

use calimero_context_config::repr::Repr;
use calimero_context_config::types::{Application, ContextId, ContextIdentity, SignerId};
use calimero_context_config::{SystemRequest, Timestamp};
use near_sdk::store::{IterableMap, IterableSet};
use near_sdk::{env, near};

use crate::{parse_input, Config, ContextConfigs, ContextConfigsExt};

const MIN_VALIDITY_THRESHOLD_MS: Timestamp = 5_000;

#[near]
impl ContextConfigs {
    #[private]
    pub fn set(&mut self) {
        parse_input!(request);

        match request {
            SystemRequest::SetValidityThreshold { threshold_ms } => {
                self.set_validity_threshold_ms(threshold_ms);
            }
        }
    }

    #[private]
    pub fn erase(&mut self) {
        env::log_str(&format!(
            "Pre-erase storage usage: {}",
            env::storage_usage()
        ));

        env::log_str("Erasing contract");

        for (_, context) in self.contexts.drain() {
            drop(context.application.into_inner());
            context.members.into_inner().clear();
        }

        env::log_str(&format!(
            "Post-erase storage usage: {}",
            env::storage_usage()
        ));
    }

    #[private]
    pub fn migrate() {
        // IterableMap doesn't support raw access to the underlying storage
        // Which hinders migration of the data, so we have to employ this trick

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

        let mut state = env::state_read::<OldContextConfigs>().expect("failed to read state");

        for (context_id, _) in state.contexts.iter_mut() {
            env::log_str(&format!("Migrating context `{}`", Repr::new(*context_id)));
        }
    }

    #[private]
    pub fn update_proxy_callback(&mut self) {
        env::log_str("Successfully updated proxy contract");
    }

    #[private]
    pub fn set_proxy_code(&mut self) {
        self.proxy_code
            .set(Some(env::input().expect("Expected proxy code").to_vec()));
    }
}

impl ContextConfigs {
    fn set_validity_threshold_ms(&mut self, validity_threshold_ms: Timestamp) {
        if validity_threshold_ms < MIN_VALIDITY_THRESHOLD_MS {
            env::panic_str("invalid validity threshold");
        }

        self.config.validity_threshold_ms = validity_threshold_ms;

        env::log_str(&format!(
            "Set validity threshold to `{:?}`",
            time::Duration::from_millis(validity_threshold_ms)
        ));
    }
}
