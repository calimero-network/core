//! A system command judged on the state as it stands, before it is proposed.
//!
//! Spec: [Judged before it is proposed](https://calimero-network.github.io/core/protocol/system-group/#judged-before-it-is-proposed)

#[allow(
    clippy::wildcard_imports,
    reason = "judges with the same items `SystemMachine::run` reads"
)]
use super::*;

/// What [`SystemMachine::run`] would refuse of `cmd` if it applied next on
/// `m`, read-only: its proof ([`authorize`]), the new-work refusal, and the
/// authority and subject checks of the commands an account with no role can
/// send (`orgs::prejudge`, `apps::prejudge`). It refuses only what `run`
/// refuses on the same state; what it lets through is judged again where it
/// applies. A refusal of a subject the actor cannot see names it missing
/// (`404`), as reads do.
///
/// # Errors
/// The refusal `run` would answer, or the missing subject's.
pub fn prejudge(m: &SystemMachine, cmd: &SystemCmd) -> Result<(), Rejection> {
    let s = &m.state;
    // The entry applies at an index past this one: the marks `run` reads are
    // judged there, and a mark that lands meanwhile only makes it skip less.
    let next = m.index.saturating_add(1);
    let mut cmd = cmd.clone();
    if s.holds_current_rules(next) {
        cmd.hold_to_current_rules();
    }
    if !cmd.is_session_command() || s.proves_sessions(next) {
        let _used = authorize(s, &cmd)?;
    }
    if let Some(stall) = m.new_work_refusal(&cmd) {
        return Err(stall.rejection());
    }
    match cmd.area() {
        Area::Orgs => orgs::prejudge(s, &cmd),
        Area::Apps => apps::prejudge(s, &cmd),
        _ => Ok(()),
    }
}
