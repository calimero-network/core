//! Synchronisation utilities for external runtimes.

use core::cell::RefCell;
use std::io;

use borsh::{to_vec, BorshDeserialize, BorshSerialize};

use crate::env;
use crate::integration::Comparison;
use crate::interface::Action;

/// An artifact to aid synchronisation with an external runtime.
#[derive(Debug, BorshSerialize)]
pub enum SyncArtifact {
    /// A list of actions.
    Actions(Vec<Action>),
    /// A list of comparisons.
    Comparisons(Vec<Comparison>),
}

impl BorshDeserialize for SyncArtifact {
    fn deserialize_reader<R: io::Read>(reader: &mut R) -> io::Result<Self> {
        let Ok(tag) = u8::deserialize_reader(reader) else {
            return Ok(SyncArtifact::Comparisons(vec![]));
        };

        match tag {
            0 => Ok(SyncArtifact::Actions(Vec::deserialize_reader(reader)?)),
            1 => Ok(SyncArtifact::Comparisons(Vec::deserialize_reader(reader)?)),
            _ => Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid tag")),
        }
    }
}

thread_local! {
    static ACTIONS: RefCell<Vec<Action>> = const { RefCell::new(Vec::new())  };
    static COMPARISON: RefCell<Vec<Comparison>> = const { RefCell::new(Vec::new())  };
}

/// Records an action for eventual synchronisation.
///
/// # Parameters
///
/// * `action` - The action to record.
///
pub fn push_action(action: Action) {
    ACTIONS.with(|actions| actions.borrow_mut().push(action));
}

/// Records a comparison for eventual synchronisation.
///
/// # Parameters
///
/// * `comparison` - The comparison to record.
///
pub fn push_comparison(comparison: Comparison) {
    COMPARISON.with(|comparisons| comparisons.borrow_mut().push(comparison));
}

/// Commits the root hash to the runtime.
/// This will also commit any recorded actions or comparisons.
/// If both actions and comparisons are present, this function will panic.
/// This function must only be called once.
///
/// # Errors
///
/// This function will return an error if there are issues accessing local
/// data or if there are problems during the comparison process.
///
pub fn commit_root(root_hash: &[u8; 32]) -> eyre::Result<()> {
    let actions = ACTIONS.with(RefCell::take);
    let comparison = COMPARISON.with(RefCell::take);

    let artifact = match (&*actions, &*comparison) {
        (&[], &[]) => vec![],
        (&[], _) => to_vec(&SyncArtifact::Comparisons(comparison))?,
        (_, &[]) => to_vec(&SyncArtifact::Actions(actions))?,
        _ => eyre::bail!("both actions and comparison are present"),
    };

    env::commit(root_hash, &artifact);

    Ok(())
}
