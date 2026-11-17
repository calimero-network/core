//! Collections over the ordered key-value store. One leader applies every write in log order,
//! so they are plain data structures, not CRDTs; `Text` and `RichText` keep Fugue's stable
//! character ids for edits typed against a stale view.

mod cell;
mod counter;
mod frozen;
mod map;
mod nested;
mod per_user;
mod rich_text;
mod set;
mod text;
mod vector;

pub use cell::Cell;
pub use counter::Counter;
pub use frozen::{CborSha256, ContentHash, Frozen};
pub use map::UnorderedMap;
pub use nested::Nested;
pub use per_user::PerUser;
pub use rich_text::{Mark, RichText, Span};
pub use set::Set;
pub use text::Text;
pub use vector::Vector;

pub use crate::commons::KeyPath;

/// A collection a [`Nested`] can keep under each of its keys: every collection, every
/// [`Table`](crate::Table), and `Nested` itself, so nesting goes as deep as the key length
/// allows.
pub trait Nestable<S: crate::store::Store>: crate::app::StateField + Sized {
    /// The collection whose keys start at `path`, on `store`. A `Nested` calls it with the
    /// path of one of its keys; a state field's collection is at [`KeyPath::root`].
    fn at(path: KeyPath, store: S) -> Self;
}
