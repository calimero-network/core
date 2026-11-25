//! The shared storage encoding (D10): order-preserving keys, deterministic CBOR values.
//!
//! Ids in these comments cite `docs/specs/app-runtime-v0.14.md` (D decisions in section 2): D10
//! (one storage encoding for both SDKs).

pub mod key;
pub mod value;
