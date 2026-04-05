use std::time::Duration;

use calimero_primitives::context::UpgradePolicy;
use clap::ValueEnum;

#[derive(Clone, Debug, ValueEnum)]
pub enum UpgradePolicyArg {
    Automatic,
    LazyOnAccess,
    Coordinated,
}

pub fn to_upgrade_policy(arg: UpgradePolicyArg, deadline_secs: Option<u64>) -> UpgradePolicy {
    match arg {
        UpgradePolicyArg::Automatic => UpgradePolicy::Automatic,
        UpgradePolicyArg::LazyOnAccess => UpgradePolicy::LazyOnAccess,
        UpgradePolicyArg::Coordinated => UpgradePolicy::Coordinated {
            deadline: deadline_secs.map(Duration::from_secs),
        },
    }
}
