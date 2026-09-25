//! A dice roll no player can rig: the smallest end-to-end use of TEE authorship.
//!
//! A member asks for a roll with [`TeeDice::roll`]. That writes nothing the game
//! trusts; it only emits `RollRequested` with the TEE handler `tee:resolve_roll`.
//! The namespace's elected TEE authority picks the event up, runs
//! [`TeeDice::resolve_roll`] inside the enclave, draws the face from
//! `env::tee_random_bytes`, and records it in `TeeOnly` state. Every peer accepts
//! that write because its signer resolves to the TEE authority, and drops any
//! member's attempt to write the same cell.

use calimero_sdk::serde::Serialize;
use calimero_sdk::{app, env};
use calimero_storage::collections::{LwwRegister, TeeOnly, UnorderedMap};
use thiserror::Error;

#[app::state(emits = for<'a> Event<'a>)]
pub struct TeeDice {
    /// Roll id → face. Written only by the TEE authority.
    results: TeeOnly<UnorderedMap<String, LwwRegister<u32>>>,
}

#[app::event]
pub enum Event<'a> {
    /// A member asked for a roll; the TEE handler resolves it.
    RollRequested { roll_id: &'a str, sides: u32 },
    /// The TEE recorded a face.
    RollResolved { roll_id: &'a str, face: u32 },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("a die needs between 2 and 1000 sides, got {0}")]
    BadSides(u32),
    #[error("roll `{0}` already exists")]
    DuplicateRoll(&'a str),
}

#[app::logic]
impl TeeDice {
    #[app::init]
    pub fn init() -> TeeDice {
        TeeDice {
            results: TeeOnly::new_tee_only(),
        }
    }

    /// Ask the TEE for a roll of a `sides`-sided die, named `roll_id`.
    pub fn roll(&mut self, roll_id: String, sides: u32) -> app::Result<()> {
        if !(2..=1000).contains(&sides) {
            app::bail!(Error::BadSides(sides));
        }
        if self.results.get()?.contains(&roll_id)? {
            app::bail!(Error::DuplicateRoll(&roll_id));
        }
        app::emit!((
            Event::RollRequested {
                roll_id: &roll_id,
                sides,
            },
            "tee:resolve_roll"
        ));
        Ok(())
    }

    /// Resolve a requested roll. Runs only on the elected TEE authority.
    ///
    /// Idempotent: a roll already resolved keeps its first face, so a replayed
    /// or duplicated request cannot re-roll it.
    #[app::tee]
    pub fn resolve_roll(&mut self, roll_id: String, sides: u32) -> app::Result<()> {
        if self.results.get()?.contains(&roll_id)? || !(2..=1000).contains(&sides) {
            return Ok(());
        }
        let face = unbiased_face(sides);
        let _previous = self
            .results
            .get_mut()?
            .insert(roll_id.clone(), LwwRegister::new(face))?;
        app::emit!(Event::RollResolved {
            roll_id: &roll_id,
            face,
        });
        Ok(())
    }

    /// The face a roll landed on, once the TEE has resolved it.
    pub fn get_roll(&self, roll_id: String) -> app::Result<Option<u32>> {
        Ok(self.results.get()?.get(&roll_id)?.map(|face| *face.get()))
    }

    /// Whether the TEE has resolved `roll_id` yet.
    pub fn is_resolved(&self, roll_id: String) -> app::Result<bool> {
        Ok(self.results.get()?.contains(&roll_id)?)
    }
}

/// A uniform face in `1..=sides`, by rejection sampling so that no face is
/// favoured when `sides` does not divide `2^32`.
fn unbiased_face(sides: u32) -> u32 {
    let zone = u32::MAX - (u32::MAX % sides);
    loop {
        let mut bytes = [0u8; 4];
        env::tee_random_bytes(&mut bytes);
        let draw = u32::from_le_bytes(bytes);
        if draw < zone {
            return draw % sides + 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use calimero_sdk::testing::TestHost;

    use super::*;

    #[test]
    fn the_tee_resolves_a_requested_roll_once() {
        let mut app = TestHost::new(TeeDice::init);

        app.call(|s| s.roll("r1".into(), 6)).unwrap();
        let requested = app.take_events();
        assert_eq!(requested.len(), 1);
        assert_eq!(requested[0].handler.as_deref(), Some("tee:resolve_roll"));
        assert_eq!(app.view(|s| s.get_roll("r1".into())).unwrap(), None);
        assert!(!app.view(|s| s.is_resolved("r1".into())).unwrap());

        app.call_as_tee(|s| s.resolve_roll("r1".into(), 6)).unwrap();
        assert!(app.view(|s| s.is_resolved("r1".into())).unwrap());
        let face = app
            .view(|s| s.get_roll("r1".into()))
            .unwrap()
            .expect("resolved");
        assert!((1..=6).contains(&face));

        // A second firing for the same roll cannot re-roll it.
        app.call_as_tee(|s| s.resolve_roll("r1".into(), 6)).unwrap();
        assert_eq!(app.view(|s| s.get_roll("r1".into())).unwrap(), Some(face));

        // Nor can a member ask for it again.
        assert!(app.call(|s| s.roll("r1".into(), 6)).is_err());
    }

    #[test]
    fn a_member_cannot_run_the_tee_method() {
        let mut app = TestHost::new(TeeDice::init);
        app.call(|s| s.roll("r1".into(), 6)).unwrap();

        let outcome = catch_unwind(AssertUnwindSafe(|| {
            app.call(|s| s.resolve_roll("r1".into(), 6))
        }));
        assert!(outcome.is_err(), "#[app::tee] must refuse a non-TEE caller");
        assert_eq!(app.view(|s| s.get_roll("r1".into())).unwrap(), None);
    }

    #[test]
    fn a_member_cannot_write_tee_only_state_directly() {
        let mut app = TestHost::new(TeeDice::init);

        let write = app.call(|s| {
            s.results
                .get_mut()
                .and_then(|map| map.insert("r1".into(), LwwRegister::new(6)))
        });
        assert!(write.is_err(), "the TeeOnly guard refuses a member's write");
        assert_eq!(app.view(|s| s.get_roll("r1".into())).unwrap(), None);
    }

    #[test]
    fn faces_stay_in_range() {
        let mut app = TestHost::new(TeeDice::init);
        for i in 0..200 {
            let id = format!("r{i}");
            app.call(|s| s.roll(id.clone(), 3)).unwrap();
            app.call_as_tee(|s| s.resolve_roll(id.clone(), 3)).unwrap();
            let face = app.view(|s| s.get_roll(id.clone())).unwrap().unwrap();
            assert!((1..=3).contains(&face));
        }
    }
}
