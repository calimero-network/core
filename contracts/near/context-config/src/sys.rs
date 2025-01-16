use core::time;

use calimero_context_config::{SystemRequest, Timestamp};
use near_sdk::{env, near, Gas, NearToken, Promise};

use crate::{parse_input, ContextConfigs, ContextConfigsExt};

mod migrations;

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

        for (_, mut context) in self.contexts.drain() {
            let _ignored = context.application.into_inner();
            context.members.into_inner().clear();
            context.member_nonces.clear();
            let proxy = context.proxy.into_inner();

            let _is_sent_on_drop = Promise::new(proxy).function_call(
                "nuke".to_owned(),
                vec![],
                NearToken::default(),
                Gas::from_tgas(1),
            );
        }

        self.next_proxy_id = 0;
        self.proxy_code.set(None);

        env::log_str(&format!(
            "Post-erase storage usage: {}",
            env::storage_usage()
        ));
    }

    #[private]
    pub fn set_proxy_code(&mut self) {
        self.proxy_code
            .set(Some(env::input().expect("Expected proxy code")));
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
