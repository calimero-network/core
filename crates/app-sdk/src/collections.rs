//! Collections over the ordered key-value store. One leader applies every write in log order,
//! so they are plain data structures, not CRDTs; `Text` and `RichText` keep Fugue's stable
//! character ids for edits typed against a stale view.

mod cell;
mod counter;
mod frozen;
mod map;
mod per_user;
mod rich_text;
mod set;
mod text;
mod vector;

pub use cell::Cell;
pub use counter::Counter;
pub use frozen::{CborSha256, ContentHash, Frozen};
pub use map::UnorderedMap;
pub use per_user::PerUser;
pub use rich_text::{Mark, RichText, Span};
pub use set::Set;
pub use text::Text;
pub use vector::Vector;
