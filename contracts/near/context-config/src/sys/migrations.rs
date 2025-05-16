use cfg_if::cfg_if;

#[cfg(feature = "migrations")]
const _: () = {
    use near_sdk::near;

    use crate::{ContextConfigs, ContextConfigsExt};

    #[near]
    impl ContextConfigs {
        #[private]
        pub fn migrate() {
            directive::migrate();
        }
    }
};

migrations! {
    "01_guard_revisions" => "migrations/01_guard_revisions.rs",
    "02_nonces"          => "migrations/02_nonces.rs",
}

// ---

macro_rules! _migrations {
    ($($migration:literal => $path:literal),+ $(,)?) => {
        $(
            #[cfg(feature = $migration)]
            #[path = $path]
            mod directive; /* migrations are exclusive */

            #[cfg(all(feature = $migration, not(feature = "migrations")))]
            compile_error!("migration selected without migrations enabled");
        )+

        cfg_if! {
            if #[cfg(not(feature = "migrations"))] {}
            $(
                else if #[cfg(feature = $migration)] {}
            )+
            else {
                mod directive {
                    pub fn migrate() {
                        /* no op */
                    }
                }

                compile_error!("migrations enabled, but no migration selected");
            }
        }
    };
}

use _migrations as migrations;
