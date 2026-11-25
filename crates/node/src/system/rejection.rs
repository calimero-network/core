//! A system command's refusal, typed by what it refuses ([`RejectionKind`]).
//!
//! The status and code a client sees come from the kind, never from the
//! wording: a message may be reworded freely. `apply` answers a refusal as
//! `{"err": <message>, "code": <code>}` ([`Rejection::to_output`]), and the
//! API reads the kind back from that code ([`Rejection::from_output`]).

use std::borrow::Cow;
use std::fmt;

use serde_json::{json, Value};

/// What a system command was refused for: each kind has its own status and
/// code ([`Self::status`], [`Self::code`]). A refusal of no kind of its own
/// is [`Self::Rejected`], `400 REJECTED`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectionKind {
    /// `400 REJECTED`: refused by the command's own rules.
    Rejected,
    /// `429 QUOTA_EXCEEDED`: a cluster or organization quota ([`Rejection::quota`]).
    QuotaExceeded,
    /// `409 RELEASE_DEADLINE`: new work past a release's deadline ([`super::Stall`]).
    ReleaseDeadline,
    /// `403 SIGN_UP_REFUSED`: the cluster's sign-up policy ([`Rejection::sign_up`]).
    SignUpRefused,
    /// `403 DEPLOY_NOT_GRANTED`: [`super::NOT_A_DEPLOYER`].
    DeployNotGranted,
    /// `409 PERSONAL_ORG_EXISTS`: [`super::PERSONAL_ORG_EXISTS`].
    PersonalOrgExists,
    /// `409 GUESTS_NOT_ALLOWED`: [`super::GUESTS_NOT_ALLOWED`].
    GuestsNotAllowed,
    /// `409 PERSONAL_ORGS_ENDED`: [`super::NO_PERSONAL_ORGS`].
    PersonalOrgsEnded,
    /// `403 CANNOT_GRANT_UNHELD`: [`super::CANNOT_GRANT_UNHELD`].
    CannotGrantUnheld,
    /// `400 UNKNOWN_CAPABILITIES`: [`super::UNKNOWN_CAPABILITIES`].
    UnknownCapabilities,
    /// `404 UNDECLARED_SECRET`: [`super::UNDECLARED_SECRET`].
    UndeclaredSecret,
    /// `409 OUTSIDE_DEPLOYMENT`: [`super::OUTSIDE_DEPLOYMENT`].
    OutsideDeployment,
    /// `404 NOT_FOUND`: [`super::NO_DEPLOYMENT_SECRET`].
    NotFound,
    /// `409 MODULE_IN_USE`: [`super::MODULE_IN_USE`].
    ModuleInUse,
    /// `409 UNIT_MEMBER`: [`super::UNIT_MEMBER`].
    UnitMember,
    /// `409 NO_RECOVERY_KEY`: [`super::NO_RECOVERY_KEY`].
    NoRecoveryKey,
    /// `409 RECOVERY_PENDING`: [`super::RECOVERY_PENDING`], [`super::ACCOUNT_RECOVERY_PENDING`].
    RecoveryPending,
    /// `409 RECOVERY_NOT_READY`: [`super::RECOVERY_NOT_READY`].
    RecoveryNotReady,
    /// `400 BAD_UPGRADE_POLICY`: [`super::BAD_UPGRADE_POLICY`].
    BadUpgradePolicy,
    /// `409 SECOND_APPROVAL_REQUIRED`: [`super::SECOND_APPROVAL_REQUIRED`].
    SecondApprovalRequired,
    /// `409 UPGRADE_CLASS_MISMATCH`: [`super::UPGRADE_CLASS_MISMATCH`].
    UpgradeClassMismatch,
    /// `409 ONE_APPROVAL`: [`super::ONE_APPROVAL`].
    OneApproval,
    /// `409 APPROVAL_PENDING`: [`super::APPROVAL_PENDING`].
    ApprovalPending,
    /// `409 AUTO_UPDATE_REFUSED`: [`super::AUTO_UPDATE_REFUSED`].
    AutoUpdateRefused,
    /// `409 HOLD_NOT_OVER`: [`super::HOLD_NOT_OVER`].
    HoldNotOver,
    /// `409 RESTORE_POINT_REQUIRED`: [`super::RESTORE_POINT_REQUIRED`].
    RestorePointRequired,
    /// `409 NO_RESTORE_POINT`: [`super::NO_RESTORE_POINT`].
    NoRestorePoint,
    /// `409 UNDO_WINDOW_PASSED`: [`super::UNDO_WINDOW_PASSED`].
    UndoWindowPassed,
    /// `409 UNDO_STALE`: [`super::UNDO_STALE`].
    UndoStale,
    /// `409 UNDO_NOT_AS_NAMED`: [`super::UNDO_NOT_AS_NAMED`].
    UndoNotAsNamed,
    /// `409 UNDO_RELEASE_GONE`: [`super::UNDO_RELEASE_GONE`].
    UndoReleaseGone,
}

impl RejectionKind {
    /// Every kind, for reading one back from its code.
    pub const ALL: [Self; 31] = [
        Self::Rejected,
        Self::QuotaExceeded,
        Self::ReleaseDeadline,
        Self::SignUpRefused,
        Self::DeployNotGranted,
        Self::PersonalOrgExists,
        Self::GuestsNotAllowed,
        Self::PersonalOrgsEnded,
        Self::CannotGrantUnheld,
        Self::UnknownCapabilities,
        Self::UndeclaredSecret,
        Self::OutsideDeployment,
        Self::NotFound,
        Self::ModuleInUse,
        Self::UnitMember,
        Self::NoRecoveryKey,
        Self::RecoveryPending,
        Self::RecoveryNotReady,
        Self::BadUpgradePolicy,
        Self::SecondApprovalRequired,
        Self::UpgradeClassMismatch,
        Self::OneApproval,
        Self::ApprovalPending,
        Self::AutoUpdateRefused,
        Self::HoldNotOver,
        Self::RestorePointRequired,
        Self::NoRestorePoint,
        Self::UndoWindowPassed,
        Self::UndoStale,
        Self::UndoNotAsNamed,
        Self::UndoReleaseGone,
    ];

    /// The code the API answers it with, and `apply`'s answer carries.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Rejected => crate::node::REJECTED,
            Self::QuotaExceeded => crate::node::QUOTA_EXCEEDED,
            Self::ReleaseDeadline => crate::node::RELEASE_DEADLINE,
            Self::SignUpRefused => "SIGN_UP_REFUSED",
            Self::DeployNotGranted => crate::node::DEPLOY_NOT_GRANTED,
            Self::PersonalOrgExists => crate::node::PERSONAL_ORG_EXISTS,
            Self::GuestsNotAllowed => "GUESTS_NOT_ALLOWED",
            Self::PersonalOrgsEnded => "PERSONAL_ORGS_ENDED",
            Self::CannotGrantUnheld => crate::node::CANNOT_GRANT_UNHELD,
            Self::UnknownCapabilities => crate::node::UNKNOWN_CAPABILITIES,
            Self::UndeclaredSecret => "UNDECLARED_SECRET",
            Self::OutsideDeployment => "OUTSIDE_DEPLOYMENT",
            Self::NotFound => "NOT_FOUND",
            Self::ModuleInUse => "MODULE_IN_USE",
            Self::UnitMember => "UNIT_MEMBER",
            Self::NoRecoveryKey => "NO_RECOVERY_KEY",
            Self::RecoveryPending => "RECOVERY_PENDING",
            Self::RecoveryNotReady => "RECOVERY_NOT_READY",
            Self::BadUpgradePolicy => "BAD_UPGRADE_POLICY",
            Self::SecondApprovalRequired => "SECOND_APPROVAL_REQUIRED",
            Self::UpgradeClassMismatch => "UPGRADE_CLASS_MISMATCH",
            Self::OneApproval => "ONE_APPROVAL",
            Self::ApprovalPending => "APPROVAL_PENDING",
            Self::AutoUpdateRefused => "AUTO_UPDATE_REFUSED",
            Self::HoldNotOver => "HOLD_NOT_OVER",
            Self::RestorePointRequired => "RESTORE_POINT_REQUIRED",
            Self::NoRestorePoint => "NO_RESTORE_POINT",
            Self::UndoWindowPassed => "UNDO_WINDOW_PASSED",
            Self::UndoStale => "UNDO_STALE",
            Self::UndoNotAsNamed => "UNDO_NOT_AS_NAMED",
            Self::UndoReleaseGone => "UNDO_RELEASE_GONE",
        }
    }

    /// The HTTP status the API answers it with.
    #[must_use]
    pub const fn status(self) -> u16 {
        match self {
            Self::Rejected | Self::UnknownCapabilities | Self::BadUpgradePolicy => 400,
            Self::SignUpRefused | Self::DeployNotGranted | Self::CannotGrantUnheld => 403,
            Self::UndeclaredSecret | Self::NotFound => 404,
            Self::QuotaExceeded => 429,
            Self::ReleaseDeadline
            | Self::PersonalOrgExists
            | Self::GuestsNotAllowed
            | Self::PersonalOrgsEnded
            | Self::OutsideDeployment
            | Self::ModuleInUse
            | Self::UnitMember
            | Self::NoRecoveryKey
            | Self::RecoveryPending
            | Self::RecoveryNotReady
            | Self::SecondApprovalRequired
            | Self::UpgradeClassMismatch
            | Self::OneApproval
            | Self::ApprovalPending
            | Self::AutoUpdateRefused
            | Self::HoldNotOver
            | Self::RestorePointRequired
            | Self::NoRestorePoint
            | Self::UndoWindowPassed
            | Self::UndoStale
            | Self::UndoNotAsNamed
            | Self::UndoReleaseGone => 409,
        }
    }

    /// The kind `code` names; `None` for a code no kind has.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.code() == code)
    }
}

/// A system command's refusal: its kind, which decides the status and code
/// the API answers, and its message, which only explains it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejection {
    kind: RejectionKind,
    message: Cow<'static, str>,
}

impl Rejection {
    /// A refusal of `kind` with a fixed message.
    #[must_use]
    pub const fn new(kind: RejectionKind, message: &'static str) -> Self {
        Self {
            kind,
            message: Cow::Borrowed(message),
        }
    }

    /// A refusal of `kind` with a message made at the time.
    #[must_use]
    pub fn with(kind: RejectionKind, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// A quota refusal: `quota exceeded: <what>`.
    #[must_use]
    pub fn quota(what: &str) -> Self {
        Self::with(
            RejectionKind::QuotaExceeded,
            format!("{}: {what}", super::QUOTA_PREFIX),
        )
    }

    /// A refusal under the cluster's sign-up policy: `sign-up refused: <why>`.
    #[must_use]
    pub fn sign_up(why: &str) -> Self {
        Self::with(
            RejectionKind::SignUpRefused,
            format!("{}: {why}", super::SIGN_UP_PREFIX),
        )
    }

    #[must_use]
    pub const fn kind(&self) -> RejectionKind {
        self.kind
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The code the API answers it with ([`RejectionKind::code`]).
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.kind.code()
    }

    /// The HTTP status the API answers it with ([`RejectionKind::status`]).
    #[must_use]
    pub const fn status(&self) -> u16 {
        self.kind.status()
    }

    /// A quota refusal, worded as one ([`Self::quota`]).
    #[cfg(test)]
    pub fn is_quota(&self) -> bool {
        self.kind == RejectionKind::QuotaExceeded && self.message.starts_with(super::QUOTA_PREFIX)
    }

    /// A refusal of new work past a release deadline, worded as one ([`super::Stall`]).
    #[cfg(test)]
    pub fn is_deadline(&self) -> bool {
        self.kind == RejectionKind::ReleaseDeadline
            && self.message.starts_with(super::DEADLINE_PREFIX)
    }

    /// What `apply` answers for it: `{"err": <message>, "code": <code>}`.
    /// Every replica answers the same bytes for the same entry.
    #[must_use]
    pub fn to_output(&self) -> Vec<u8> {
        // `Value`'s `Display` is its compact JSON and cannot fail.
        json!({ "err": self.message(), "code": self.code() })
            .to_string()
            .into_bytes()
    }

    /// The refusal an `apply` answer's `err` holds, of the kind its `code`
    /// names; [`RejectionKind::Rejected`] without one.
    #[must_use]
    pub fn from_output(answer: &Value, err: &Value) -> Self {
        let kind = answer
            .get("code")
            .and_then(Value::as_str)
            .and_then(RejectionKind::from_code)
            .unwrap_or(RejectionKind::Rejected);
        Self::with(kind, err.as_str().unwrap_or("error").to_owned())
    }
}

impl From<&'static str> for Rejection {
    /// A refusal by the command's own rules ([`RejectionKind::Rejected`]).
    fn from(message: &'static str) -> Self {
        Self::new(RejectionKind::Rejected, message)
    }
}

impl From<&Rejection> for Rejection {
    fn from(refused: &Rejection) -> Self {
        refused.clone()
    }
}

impl From<String> for Rejection {
    /// A refusal by the command's own rules ([`RejectionKind::Rejected`]).
    fn from(message: String) -> Self {
        Self::with(RejectionKind::Rejected, message)
    }
}

impl fmt::Display for Rejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

#[cfg(test)]
impl PartialEq<str> for Rejection {
    /// Its message is `other`, whatever its kind.
    fn eq(&self, other: &str) -> bool {
        self.message == other
    }
}

#[cfg(test)]
impl PartialEq<&str> for Rejection {
    /// Its message is `other`, whatever its kind.
    fn eq(&self, other: &&str) -> bool {
        self.message == *other
    }
}

#[cfg(test)]
impl PartialEq<String> for Rejection {
    /// Its message is `other`, whatever its kind.
    fn eq(&self, other: &String) -> bool {
        self.message == other.as_str()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{Rejection, RejectionKind};
    use crate::system;

    /// Each kind reads back from its own code, and no two kinds share one.
    #[test]
    fn every_kind_reads_back_from_its_code() {
        for kind in RejectionKind::ALL {
            assert_eq!(RejectionKind::from_code(kind.code()), Some(kind));
        }
        assert_eq!(RejectionKind::from_code("NO_SUCH_CODE"), None);
    }

    /// The status and code each named refusal reaches a client with: what
    /// `node::rejected` answered for it when it matched the message.
    #[test]
    fn each_named_refusal_keeps_its_status_and_code() {
        let named: [(&Rejection, u16, &str); 28] = [
            (&system::NOT_A_DEPLOYER, 403, "DEPLOY_NOT_GRANTED"),
            (&system::PERSONAL_ORG_EXISTS, 409, "PERSONAL_ORG_EXISTS"),
            (&system::GUESTS_NOT_ALLOWED, 409, "GUESTS_NOT_ALLOWED"),
            (&system::NO_PERSONAL_ORGS, 409, "PERSONAL_ORGS_ENDED"),
            (&system::CANNOT_GRANT_UNHELD, 403, "CANNOT_GRANT_UNHELD"),
            (&system::UNKNOWN_CAPABILITIES, 400, "UNKNOWN_CAPABILITIES"),
            (&system::UNDECLARED_SECRET, 404, "UNDECLARED_SECRET"),
            (&system::OUTSIDE_DEPLOYMENT, 409, "OUTSIDE_DEPLOYMENT"),
            (&system::NO_DEPLOYMENT_SECRET, 404, "NOT_FOUND"),
            (&system::MODULE_IN_USE, 409, "MODULE_IN_USE"),
            (&system::UNIT_MEMBER, 409, "UNIT_MEMBER"),
            (&system::NO_RECOVERY_KEY, 409, "NO_RECOVERY_KEY"),
            (&system::RECOVERY_PENDING, 409, "RECOVERY_PENDING"),
            (&system::ACCOUNT_RECOVERY_PENDING, 409, "RECOVERY_PENDING"),
            (&system::RECOVERY_NOT_READY, 409, "RECOVERY_NOT_READY"),
            (&system::BAD_UPGRADE_POLICY, 400, "BAD_UPGRADE_POLICY"),
            (
                &system::SECOND_APPROVAL_REQUIRED,
                409,
                "SECOND_APPROVAL_REQUIRED",
            ),
            (
                &system::UPGRADE_CLASS_MISMATCH,
                409,
                "UPGRADE_CLASS_MISMATCH",
            ),
            (&system::ONE_APPROVAL, 409, "ONE_APPROVAL"),
            (&system::APPROVAL_PENDING, 409, "APPROVAL_PENDING"),
            (&system::AUTO_UPDATE_REFUSED, 409, "AUTO_UPDATE_REFUSED"),
            (&system::HOLD_NOT_OVER, 409, "HOLD_NOT_OVER"),
            (
                &system::RESTORE_POINT_REQUIRED,
                409,
                "RESTORE_POINT_REQUIRED",
            ),
            (&system::NO_RESTORE_POINT, 409, "NO_RESTORE_POINT"),
            (&system::UNDO_WINDOW_PASSED, 409, "UNDO_WINDOW_PASSED"),
            (&system::UNDO_STALE, 409, "UNDO_STALE"),
            (&system::UNDO_NOT_AS_NAMED, 409, "UNDO_NOT_AS_NAMED"),
            (&system::UNDO_RELEASE_GONE, 409, "UNDO_RELEASE_GONE"),
        ];
        for (refused, status, code) in named {
            let api = crate::node::rejected(refused);
            assert_eq!((api.status, api.code.as_str()), (status, code), "{refused}");
            assert_eq!(api.message, refused.message());
        }
        let cases = [
            (Rejection::quota("apps"), 429, "QUOTA_EXCEEDED"),
            (Rejection::sign_up("invite only"), 403, "SIGN_UP_REFUSED"),
            (Rejection::from("no such organization"), 400, "REJECTED"),
        ];
        for (refused, status, code) in cases {
            let api = crate::node::rejected(&refused);
            assert_eq!((api.status, api.code.as_str()), (status, code), "{refused}");
        }
        let stall = system::Stall {
            release: "2.1.0".into(),
            after: 0,
        };
        let api = crate::node::rejected(stall.rejection());
        assert_eq!((api.status, api.code.as_str()), (409, "RELEASE_DEADLINE"));
    }

    /// `apply`'s answer keeps the message under `err`, beside the code, and
    /// reads back as the same refusal; an answer without a code is `REJECTED`.
    #[test]
    fn an_answer_carries_its_message_and_code() {
        let refused = Rejection::quota("apps");
        let out = refused.to_output();
        assert_eq!(
            serde_json::from_slice::<Value>(&out).unwrap(),
            json!({ "err": "quota exceeded: apps", "code": "QUOTA_EXCEEDED" })
        );
        assert_eq!(system::output(&out), Err(refused));
        assert_eq!(
            system::output(br#"{"err":"quota exceeded: apps"}"#),
            Err(Rejection::from("quota exceeded: apps"))
        );
    }
}
