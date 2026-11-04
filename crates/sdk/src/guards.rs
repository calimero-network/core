//! The guards every app wrote: signed-in and admin.

use crate::env::{self, Caller, OrgRole};
use crate::error::{AppError, Result};

/// The account the caller acts for; `LoginRequired` for system, public and app callers.
fn account() -> Result<String> {
    match env::caller() {
        Caller::Account(account) | Caller::ApiKey { account, .. } => Ok(account),
        Caller::Public | Caller::System | Caller::App(_) => Err(AppError::login_required()),
    }
}

/// An account whose role is at least `role` in the app's org; with no org, the app owner
/// holds every role and any other account only `Member`.
fn require_role(role: OrgRole) -> Result<String> {
    let account = account()?;
    let held = match env::org() {
        Some(_) => env::org_role(&account),
        None if account == env::app_owner() => Some(OrgRole::Owner),
        None => Some(OrgRole::Member),
    };
    match held {
        Some(held) if held >= role => Ok(account),
        _ => Err(AppError::forbidden()),
    }
}

/// The caller's account, if it is a member (or more) of the app's org.
pub fn require_member() -> Result<String> {
    require_role(OrgRole::Member)
}

/// The caller's account, if it is an admin (or the owner) of the app's org.
pub fn require_admin() -> Result<String> {
    require_role(OrgRole::Admin)
}
