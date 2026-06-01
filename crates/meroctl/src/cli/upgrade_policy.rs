use calimero_primitives::context::UpgradePolicy;
use clap::ValueEnum;

/// CLI-selectable upgrade policies.
///
/// `Coordinated` is intentionally absent: it has been removed (it did nothing
/// `Automatic` doesn't and its `deadline` was never enforced), so it can no
/// longer be selected. Use `LazyOnAccess` for migrating upgrades and
/// `Automatic` for code-only upgrades.
#[derive(Clone, Debug, ValueEnum)]
pub enum UpgradePolicyArg {
    Automatic,
    LazyOnAccess,
}

/// Map a CLI policy choice to the core [`UpgradePolicy`].
pub fn to_upgrade_policy(arg: UpgradePolicyArg) -> UpgradePolicy {
    match arg {
        UpgradePolicyArg::Automatic => UpgradePolicy::Automatic,
        UpgradePolicyArg::LazyOnAccess => UpgradePolicy::LazyOnAccess,
    }
}
