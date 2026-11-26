//! What an applied system entry may change for live views: the facts a watched view can read
//! through the host (`calimero_app_host::SystemRead`) and who may still hold a watch stream.
//!
//! Every replica applies the same entries, so each node classifies them alike. An entry this
//! module does not name is [`SysEffect::All`]: a new command re-runs every view that read the
//! system until it is classified here, never silently none.

use std::sync::Arc;

use tokio::sync::broadcast;

use super::SystemCmd;
use crate::types::{AccountId, OrgId, TeamId};

/// Effects one node's system machine buffers for slow watch streams; one that falls behind
/// treats what it missed as [`SysEffect::All`].
pub const SYS_EFFECTS_BUFFER: usize = 1_024;

/// What an applied system entry may change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SysEffect {
    /// Nothing a view reads and nobody's access: a token refreshed, a sign count, a run's
    /// lease, an invite made but not redeemed.
    Quiet,
    /// Someone's access may have ended (a session, passkey, key, link or app invite
    /// revoked); nothing a view reads.
    Access,
    /// These facts, and anyone's access.
    Touches(Vec<SysTouch>),
    /// Anything.
    All,
}

/// One fact an entry may change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SysTouch {
    /// `account`'s membership or role in `org` (`None`: an org the entry does not name).
    Member {
        org: Option<OrgId>,
        account: AccountId,
    },
    /// Team `team` (`None`: any team, e.g. one whose parent moved) of `org`, for `account`
    /// (`None`: everyone in it).
    Team {
        org: Option<OrgId>,
        team: Option<TeamId>,
        account: Option<AccountId>,
    },
    /// Grants on apps (capabilities, app scopes) in `org` for `account` (`None`: everyone).
    Grant {
        org: Option<OrgId>,
        account: Option<AccountId>,
    },
}

/// The effect of `cmd`, decided before it applies (an entry refused at apply changes
/// nothing and is not sent).
#[must_use]
pub fn effect_of(cmd: &SystemCmd) -> SysEffect {
    use SysTouch as T;
    use SystemCmd as C;
    let one = |t: SysTouch| SysEffect::Touches(vec![t]);
    match cmd {
        C::CreateRefresh { .. }
        | C::UseRefresh { .. }
        | C::SetSignCount { .. }
        | C::SetPasskeyLabel { .. }
        | C::AcquireRun { .. }
        | C::ReleaseRun { .. }
        | C::ChargeFileBytes { .. }
        | C::CreateAccount { .. }
        | C::AddPasskey { .. }
        | C::CreateApiKey { .. }
        | C::SetAlias { .. }
        | C::SetBackupKey { .. }
        | C::CreateInvite { .. }
        | C::RevokeInvite { .. }
        | C::CreateAppInvite { .. }
        | C::CreatePublicLink { .. }
        | C::SetAccountRecoveryKey { .. }
        | C::RemoveAccountRecoveryKey { .. }
        | C::StartAccountRecovery { .. }
        | C::CancelAccountRecovery { .. } => SysEffect::Quiet,
        C::RevokeSession { .. }
        | C::RevokeAllSessions { .. }
        | C::RefreshReused { .. }
        | C::RemovePasskey { .. }
        | C::RevokeApiKey { .. }
        | C::RevokePublicLink { .. }
        | C::RevokeAppInvite { .. }
        | C::FinishAccountRecovery { .. } => SysEffect::Access,
        C::SetOrgRole { org, account, .. } | C::RemoveOrgMember { org, account, .. } => {
            one(T::Member {
                org: Some(org.clone()),
                account: account.clone(),
            })
        }
        // The invite names its org and teams in the system state, not in the entry.
        C::RedeemInvite { account, .. } => SysEffect::Touches(vec![
            T::Member {
                org: None,
                account: account.clone(),
            },
            T::Team {
                org: None,
                team: None,
                account: Some(account.clone()),
            },
            T::Grant {
                org: None,
                account: Some(account.clone()),
            },
        ]),
        C::RedeemAppInvite { account, .. } => one(T::Grant {
            org: None,
            account: Some(account.clone()),
        }),
        C::CreateTeam { org, team_id, .. } => one(T::Team {
            org: Some(org.clone()),
            team: Some(team_id.clone()),
            account: None,
        }),
        C::SetTeamMember {
            org, team, account, ..
        } => one(T::Team {
            org: Some(org.clone()),
            team: Some(team.clone()),
            account: Some(account.clone()),
        }),
        C::SetTeamMembers { org, account, .. }
        | C::RemoveTeamMember { org, account, .. }
        | C::RemoveTeamMembers { org, account, .. } => one(T::Team {
            org: Some(org.clone()),
            team: None,
            account: Some(account.clone()),
        }),
        // A team's parent decides who its subteams' members are in: any team of the org.
        C::UpdateTeam { org, .. } | C::DeleteTeam { org, .. } => one(T::Team {
            org: Some(org.clone()),
            team: None,
            account: None,
        }),
        C::SetCapabilities { org, account, .. } | C::SetAllApps { org, account, .. } => {
            one(T::Grant {
                org: Some(org.clone()),
                account: Some(account.clone()),
            })
        }
        C::SetDefaultCapabilities { org, .. } | C::ScopeApps { org, .. } => one(T::Grant {
            org: Some(org.clone()),
            account: None,
        }),
        _ => SysEffect::All,
    }
}

/// Where a system machine sends the effect of each entry it applied, while anyone listens.
#[derive(Clone)]
pub struct EffectsTx(broadcast::Sender<Arc<(u64, SysEffect)>>);

impl Default for EffectsTx {
    fn default() -> Self {
        Self(broadcast::channel(SYS_EFFECTS_BUFFER).0)
    }
}

impl EffectsTx {
    /// Sends `effect` of entry `index` (nothing when nobody listens).
    pub fn send(&self, index: u64, effect: SysEffect) {
        if self.0.receiver_count() > 0 {
            let _receivers = self.0.send(Arc::new((index, effect)));
        }
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<(u64, SysEffect)>> {
        self.0.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn cmd(v: serde_json::Value) -> SystemCmd {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_revoked_session_ends_access_and_re_runs_nothing() {
        let revoke = cmd(json!({ "op": "revokeSession", "session": "s", "actor": "ada" }));
        assert_eq!(effect_of(&revoke), SysEffect::Access);
    }

    #[test]
    fn a_role_change_names_its_member_and_org() {
        let promote = cmd(json!({
            "op": "setOrgRole", "org": "o", "account": "ada", "role": "admin",
            "actor": "bob", "consent": null
        }));
        assert_eq!(
            effect_of(&promote),
            SysEffect::Touches(vec![SysTouch::Member {
                org: Some("o".into()),
                account: "ada".into(),
            }])
        );
    }

    #[test]
    fn an_unclassified_entry_touches_everything() {
        let rename = cmd(json!({ "op": "requireSessionProofs" }));
        assert_eq!(effect_of(&rename), SysEffect::All);
    }
}
