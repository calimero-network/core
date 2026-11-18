//! A digest of a group's applied state, kept as entries apply, so the replicas of
//! a group can compare their states at the same log index and a replica that
//! applied differently from the others is noticed instead of serving on silently.
//!
//! The digest is a sum, modulo 2^256, of `SHA-256(label ‖ len(key) ‖ key ‖ value)`
//! over every state row of the group (its stored key, hidden keys included, and
//! its value as stored). A sum depends on the rows alone, not on the order they
//! were written in, how entries were batched, or whether the state arrived by
//! applying entries or by a snapshot install, so two replicas at the same index
//! hold the same digest exactly when they hold the same rows. A write moves it by
//! the row it adds less the row it replaces ([`StateDigest::replace`]), which
//! costs one read of the row it replaces, never a scan.
//!
//! Only disk machines have one: an in-memory machine (the system group) writes no
//! rows as it applies.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// What the digest of a row is taken over. Raise it when a row's stored shape, or
/// this sum, changes: replicas on releases with different versions do not compare.
pub const DIGEST_VERSION: u32 = 1;

/// A replica records its digest at every index that is a multiple of this, so
/// replicas that batched their applies differently still meet at the same index.
pub const DIGEST_EVERY: u64 = 16;

/// How many of those recorded digests a replica keeps: a replica that lags another
/// by more than `DIGEST_EVERY * DIGEST_KEPT` entries is compared once it catches up.
pub const DIGEST_KEPT: usize = 16;

const ROW_LABEL: &[u8] = b"calimero.log.state-digest.v1";

/// A digest of a group's state rows ([`self`]).
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct StateDigest([u64; 4]);

impl StateDigest {
    /// The digest of one row.
    #[must_use]
    pub fn of_row(key: &[u8], value: &[u8]) -> Self {
        let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
        ctx.update(ROW_LABEL);
        ctx.update(&u32::try_from(key.len()).unwrap_or(u32::MAX).to_be_bytes());
        ctx.update(key);
        ctx.update(value);
        let h = ctx.finish();
        Self::from_bytes(h.as_ref())
    }

    /// From 32 bytes, little-endian (a shorter slice reads as zeros past its end).
    fn from_bytes(b: &[u8]) -> Self {
        let mut limbs = [0_u64; 4];
        for (i, limb) in limbs.iter_mut().enumerate() {
            let mut w = [0_u8; 8];
            for (j, byte) in w.iter_mut().enumerate() {
                *byte = b.get(i * 8 + j).copied().unwrap_or(0);
            }
            *limb = u64::from_le_bytes(w);
        }
        Self(limbs)
    }

    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        let mut out = [0_u8; 32];
        for (i, limb) in self.0.iter().enumerate() {
            for (j, byte) in limb.to_le_bytes().into_iter().enumerate() {
                out[i * 8 + j] = byte;
            }
        }
        out
    }

    /// `self + other`, modulo 2^256.
    #[must_use]
    pub fn plus(self, other: Self) -> Self {
        let mut out = [0_u64; 4];
        let mut carry = false;
        for (i, o) in out.iter_mut().enumerate() {
            let (s, c1) = self.0[i].overflowing_add(other.0[i]);
            let (s, c2) = s.overflowing_add(u64::from(carry));
            *o = s;
            carry = c1 || c2;
        }
        Self(out)
    }

    /// `self - other`, modulo 2^256.
    #[must_use]
    pub fn minus(self, other: Self) -> Self {
        let mut out = [0_u64; 4];
        let mut borrow = false;
        for (i, o) in out.iter_mut().enumerate() {
            let (s, b1) = self.0[i].overflowing_sub(other.0[i]);
            let (s, b2) = s.overflowing_sub(u64::from(borrow));
            *o = s;
            borrow = b1 || b2;
        }
        Self(out)
    }

    /// The digest after row `key` goes from `old` to `new` (`None`: absent).
    #[must_use]
    pub fn replace(self, key: &[u8], old: Option<&[u8]>, new: Option<&[u8]>) -> Self {
        let mut d = self;
        if let Some(old) = old {
            d = d.minus(Self::of_row(key, old));
        }
        if let Some(new) = new {
            d = d.plus(Self::of_row(key, new));
        }
        d
    }
}

impl fmt::Display for StateDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.to_bytes()))
    }
}

impl fmt::Debug for StateDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StateDigest({self})")
    }
}

impl Serialize for StateDigest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for StateDigest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        let b = hex::decode(s.as_bytes()).map_err(serde::de::Error::custom)?;
        if b.len() != 32 {
            return Err(serde::de::Error::custom("a state digest is 32 bytes"));
        }
        Ok(Self::from_bytes(&b))
    }
}

/// The digest a group's state had at one index, as it is kept with the applied
/// index (meta row `digest`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DigestAt {
    pub index: u64,
    pub digest: StateDigest,
}

/// What one replica tells another of its state ([`crate::Group::digests`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Digests {
    /// [`DIGEST_VERSION`] of the release that took them.
    pub version: u32,
    /// The digest at the index this replica has applied.
    pub applied: DigestAt,
    /// Its digests at the latest indexes that are multiples of [`DIGEST_EVERY`],
    /// oldest first, at most [`DIGEST_KEPT`].
    pub recorded: Vec<DigestAt>,
}

/// How two replicas' [`Digests`] compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparison {
    /// Both had the same state at `index`, the latest index both know a digest at.
    Agree { index: u64 },
    /// They had different states at `index`.
    Differ {
        index: u64,
        ours: StateDigest,
        theirs: StateDigest,
    },
    /// No index both know a digest at (one lags too far, or they run releases
    /// whose digests are not comparable): nothing to say yet.
    Unknown,
}

impl Digests {
    fn at(&self) -> BTreeMap<u64, StateDigest> {
        self.recorded
            .iter()
            .chain(std::iter::once(&self.applied))
            .map(|d| (d.index, d.digest))
            .collect()
    }

    /// Compare at every index both know a digest at: a difference at any of them
    /// (the latest such) wins over agreement at others.
    #[must_use]
    pub fn compare(&self, theirs: &Self) -> Comparison {
        if self.version != theirs.version {
            return Comparison::Unknown;
        }
        let ours = self.at();
        let other = theirs.at();
        let mut agree = None;
        for (index, d) in ours.iter().rev() {
            let Some(t) = other.get(index) else { continue };
            if t != d {
                return Comparison::Differ {
                    index: *index,
                    ours: *d,
                    theirs: *t,
                };
            }
            agree = agree.or(Some(*index));
        }
        agree.map_or(Comparison::Unknown, |index| Comparison::Agree { index })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(r: &[(&str, &str)]) -> StateDigest {
        r.iter().fold(StateDigest::default(), |d, (k, v)| {
            d.plus(StateDigest::of_row(k.as_bytes(), v.as_bytes()))
        })
    }

    #[test]
    fn the_digest_is_of_the_rows_whatever_order_or_path_they_came_by() {
        let a = rows(&[("a", "1"), ("b", "2"), ("c", "3")]);
        let b = rows(&[("c", "3"), ("a", "1"), ("b", "2")]);
        assert_eq!(a, b, "the order rows were written in does not count");
        // Written, overwritten and deleted on the way there.
        let c = StateDigest::default()
            .replace(b"b", None, Some(b"9"))
            .replace(b"a", None, Some(b"1"))
            .replace(b"x", None, Some(b"gone"))
            .replace(b"b", Some(b"9"), Some(b"2"))
            .replace(b"c", None, Some(b"3"))
            .replace(b"x", Some(b"gone"), None);
        assert_eq!(a, c, "a replaced or deleted row leaves nothing behind");
        assert_ne!(a, rows(&[("a", "1"), ("b", "2"), ("c", "4")]));
        assert_ne!(
            rows(&[("ab", "c")]),
            rows(&[("a", "bc")]),
            "where the key ends is part of the row"
        );
    }

    #[test]
    fn the_sum_carries_and_borrows_across_its_limbs() {
        let max = StateDigest([u64::MAX; 4]);
        let one = StateDigest([1, 0, 0, 0]);
        assert_eq!(max.plus(one), StateDigest::default());
        assert_eq!(StateDigest::default().minus(one), max);
        let r = StateDigest::of_row(b"k", b"v");
        assert_eq!(max.plus(r).minus(r), max);
    }

    #[test]
    fn a_digest_is_written_as_hex_and_read_back() {
        let d = rows(&[("k", "v")]);
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json.len(), 66);
        assert_eq!(serde_json::from_str::<StateDigest>(&json).unwrap(), d);
        assert!(serde_json::from_str::<StateDigest>("\"00\"").is_err());
    }

    fn at(index: u64, d: StateDigest) -> DigestAt {
        DigestAt { index, digest: d }
    }

    #[test]
    fn replicas_compare_at_the_latest_index_both_know() {
        let x = rows(&[("k", "1")]);
        let y = rows(&[("k", "2")]);
        let z = rows(&[("k", "3")]);
        let ours = Digests {
            version: DIGEST_VERSION,
            applied: at(40, z),
            recorded: vec![at(16, x), at(32, y)],
        };
        // A follower a few entries behind: they meet at 32.
        let behind = Digests {
            version: DIGEST_VERSION,
            applied: at(35, y),
            recorded: vec![at(16, x), at(32, y)],
        };
        assert_eq!(ours.compare(&behind), Comparison::Agree { index: 32 });
        let diverged = Digests {
            version: DIGEST_VERSION,
            applied: at(35, y),
            recorded: vec![at(16, x), at(32, z)],
        };
        assert_eq!(
            ours.compare(&diverged),
            Comparison::Differ {
                index: 32,
                ours: y,
                theirs: z
            }
        );
        // An idle group: both at the same applied index between recordings.
        let same = Digests {
            version: DIGEST_VERSION,
            applied: at(40, z),
            recorded: vec![],
        };
        assert_eq!(ours.compare(&same), Comparison::Agree { index: 40 });
        let far = Digests {
            version: DIGEST_VERSION,
            applied: at(3, x),
            recorded: vec![],
        };
        assert_eq!(ours.compare(&far), Comparison::Unknown);
        let other_release = Digests {
            version: DIGEST_VERSION + 1,
            ..ours.clone()
        };
        assert_eq!(ours.compare(&other_release), Comparison::Unknown);
    }
}
