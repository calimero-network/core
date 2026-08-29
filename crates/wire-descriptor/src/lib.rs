//! A structural fingerprint for borsh types that travel between nodes.
//!
//! # The failure this exists to catch
//!
//! Borsh encodes an enum variant as a bare `u8` *ordinal* derived from
//! declaration order. Inserting a variant anywhere but the end silently
//! renumbers every later variant, and a struct field added in the middle
//! silently reinterprets every byte after it. Neither shows up in an
//! encode → decode round-trip inside one binary: the encoder and the decoder
//! shift together and agree on the wrong answer. It only shows up when an old
//! node meets a new one — in production, as "joined, replicating 0 contexts".
//!
//! Frozen-byte goldens (decode-only) catch this for the *specific* variants
//! somebody remembered to freeze. This module catches it for the *shape of the
//! whole surface*, including the case nobody freezes: a field retyped from
//! `u32` to `u64`, or renamed in a way that swaps two same-width fields.
//!
//! # Why a hand-maintained descriptor
//!
//! There is no derive here on purpose. A macro that reflected the real type
//! would make the snapshot a function of the type, so a change to the type
//! would change both sides at once — exactly the same blindness as a
//! round-trip test, one level up. The descriptor is a **second, independent
//! statement** of the wire layout, written by a human, and the gate is that
//! the two statements agree. The crates that own a wire type pair each
//! descriptor with an exhaustive `match` (for enums) or an exhaustive
//! destructuring (for structs), so adding a variant or a field does not
//! compile until the descriptor is updated too, and updating the descriptor
//! moves the snapshot, which is the review signal.
//!
//! # Usage
//!
//! Build a [`Surface`] in a `#[cfg(test)]` module of the crate that owns the
//! types, then call [`assert_snapshot`] against a committed `.txt` file.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

/// Environment variable that turns the snapshot assertion into a regeneration.
///
/// Mirrors `UPDATE_FIXTURES` on the HTTP-DTO wire-contract gate.
pub const UPDATE_ENV: &str = "UPDATE_WIRE_FINGERPRINT";

/// One named field of a struct, or of a struct-like enum variant.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    /// Field name exactly as declared (a rename is a review-worthy event even
    /// though borsh does not encode it — it usually accompanies a retype).
    pub name: &'static str,
    /// Rendered type, as declared. Foreign leaf types are named, not expanded;
    /// pin those with [`Leaf`] instead of growing this into a whole-graph
    /// reflection.
    pub ty: &'static str,
}

/// One enum variant and the borsh ordinal it must keep forever.
#[derive(Clone, Copy, Debug)]
pub struct Variant {
    /// The `u8` borsh discriminant. Declaration order, permanently.
    pub ordinal: u8,
    /// Variant name as declared.
    pub name: &'static str,
    /// Fields in declaration (= encoding) order. Empty for a unit variant.
    pub fields: &'static [Field],
}

/// Whether a described type encodes as a tagged union or as a flat record.
#[derive(Clone, Copy, Debug)]
pub enum Shape {
    /// Tagged union: one leading ordinal byte, then the variant's fields.
    Enum(&'static [Variant]),
    /// Flat record: fields concatenated in declaration order, no tag.
    Struct(&'static [Field]),
}

/// A described wire type.
#[derive(Clone, Copy, Debug)]
pub struct TypeDesc {
    /// Type name as declared.
    pub name: &'static str,
    /// Its encoding shape.
    pub shape: Shape,
}

/// A type whose layout is owned by *another* crate and is not described
/// field-by-field here, pinned instead by the bytes a canonical instance
/// encodes to.
///
/// This is the deliberate boundary of the gate: the descriptor stops at the
/// crate that owns the wire surface, and everything below it is held still by
/// a hash rather than by a second graph of hand-written descriptors. A field
/// added to `DeviceCert` reddens the leaf line without this crate knowing
/// anything about `DeviceCert`.
pub struct Leaf {
    /// Type name as referenced from the descriptor above.
    pub name: &'static str,
    /// `borsh::to_vec` of a canonical (conventionally all-zero) instance.
    pub bytes: Vec<u8>,
}

/// Everything one crate contributes to the replicated wire surface.
pub struct Surface {
    /// Snapshot label — conventionally the crate name.
    pub label: &'static str,
    /// Described types, in a stable order chosen by the author.
    pub types: Vec<TypeDesc>,
    /// Foreign leaf types pinned by encoded bytes.
    pub leaves: Vec<Leaf>,
}

impl Surface {
    /// The snapshot body: everything except the header comment.
    ///
    /// Deliberately a line-per-variant text rather than a single hash, so the
    /// `git diff` on an intentional change *is* the review artifact — you see
    /// "variant 7 became variant 8", not "a hash changed".
    #[must_use]
    pub fn body(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "surface {}", self.label);
        out.push('\n');
        for ty in &self.types {
            match ty.shape {
                Shape::Enum(variants) => {
                    let _ = writeln!(out, "enum {}", ty.name);
                    for v in variants {
                        let _ = writeln!(out, "  {} {}{}", v.ordinal, v.name, render(v.fields));
                    }
                }
                Shape::Struct(fields) => {
                    let _ = writeln!(out, "struct {}", ty.name);
                    for (i, f) in fields.iter().enumerate() {
                        let _ = writeln!(out, "  {} {}: {}", i, f.name, f.ty);
                    }
                }
            }
            out.push('\n');
        }
        for leaf in &self.leaves {
            let _ = writeln!(
                out,
                "leaf {} len={} blake3={}",
                leaf.name,
                leaf.bytes.len(),
                short_hash(&leaf.bytes)
            );
        }
        out
    }

    /// A single value naming this whole surface — handy to quote in a release
    /// note or a compatibility catalog entry.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        short_hash(self.body().as_bytes())
    }

    /// The full committed file: header, fingerprint, body.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(HEADER);
        let _ = writeln!(out, "fingerprint blake3={}", self.fingerprint());
        out.push('\n');
        out.push_str(&self.body());
        out
    }
}

fn render(fields: &[Field]) -> String {
    if fields.is_empty() {
        return String::new();
    }
    let inner: Vec<String> = fields
        .iter()
        .map(|f| format!("{}: {}", f.name, f.ty))
        .collect();
    format!("({})", inner.join(", "))
}

fn short_hash(bytes: &[u8]) -> String {
    // 16 hex chars is plenty to make an accidental collision impossible while
    // keeping the snapshot readable; this is a change detector, not a MAC.
    blake3::hash(bytes).to_hex()[..16].to_owned()
}

const HEADER: &str = "\
# Calimero replicated wire surface — GENERATED, DO NOT EDIT BY HAND.
#
# Regenerate with UPDATE_WIRE_FINGERPRINT=1 and the crate's test command; the
# failing assertion prints the exact one.
#
# APPENDING a variant at the END of an enum is wire-COMPATIBLE: an old node
# fails to decode the new tag and rejects the op, but every op it already
# understands still decodes identically.
#
# INSERTING, REMOVING, REORDERING or RETYPING anything below is a HARD BREAK:
# borsh discriminants are bare positional u8s, so every later variant silently
# renumbers and an old peer decodes the WRONG variant rather than erroring.
# That is the rc.7/rc.8 NamespaceGovOpValue incident — a fleet that reported
# 'joined' while replicating nothing. If you must do it, bump the schema
# version constant in the same commit and plan a coordinated rollout.
";

/// Compare a computed surface against a committed snapshot, returning a
/// human-readable report on mismatch.
///
/// # Errors
///
/// Returns the rendered diff plus remediation guidance when the two differ.
pub fn check(surface: &Surface, committed: &str) -> Result<(), String> {
    let computed = surface.render();
    if computed == committed {
        return Ok(());
    }
    Err(format!(
        "{}\n{}",
        diff(committed, &computed),
        GUIDANCE.trim_end()
    ))
}

/// A line-oriented diff of committed (`-`) versus computed (`+`).
///
/// Not a real LCS diff: wire snapshots change in small, localized ways, and a
/// positional comparison points at the moved variant just as well while
/// staying dependency-free.
#[must_use]
pub fn diff(committed: &str, computed: &str) -> String {
    let old: Vec<&str> = committed.lines().collect();
    let new: Vec<&str> = computed.lines().collect();
    let mut out = String::from("wire surface changed:\n");
    let mut shown = 0_usize;
    for i in 0..old.len().max(new.len()) {
        let o = old.get(i).copied();
        let n = new.get(i).copied();
        if o == n {
            continue;
        }
        if shown == MAX_DIFF_LINES {
            let _ = writeln!(out, "  ... (further differences suppressed)");
            break;
        }
        shown += 1;
        let _ = writeln!(out, "  line {}:", i + 1);
        if let Some(o) = o {
            let _ = writeln!(out, "    - committed: {o}");
        }
        if let Some(n) = n {
            let _ = writeln!(out, "    + computed:  {n}");
        }
    }
    out
}

const MAX_DIFF_LINES: usize = 40;

const GUIDANCE: &str = "\
What to do about it:

  * You APPENDED a variant at the end of an enum (ordinals of existing
    variants unchanged, only new lines at the bottom of a block):
    wire-compatible. Regenerate the snapshot and mention it in the PR.

  * Anything else is a HARD BREAK — a variant inserted or removed, two
    variants swapped, a field added/removed/reordered/retyped, a leaf hash
    moved. Old peers will decode the wrong variant or the wrong field bytes,
    silently. Bump the schema-version constant for that surface in the same
    commit, and treat the rollout as coordinated (owner nodes and TEE fleet
    nodes must not straddle the change).

Regenerate (only after deciding which of the two cases you are in):

  UPDATE_WIRE_FINGERPRINT=1 cargo test -p <crate> wire_fingerprint
";

/// Assert a surface against a snapshot file, or rewrite it when
/// [`UPDATE_ENV`] is set.
///
/// # Panics
///
/// Panics with the diff and guidance when the surface has drifted, and when
/// the snapshot cannot be read or written.
pub fn assert_snapshot(surface: &Surface, path: &Path) {
    let computed = surface.render();

    if std::env::var_os(UPDATE_ENV).is_some() {
        fs::write(path, &computed)
            .unwrap_or_else(|e| panic!("write wire snapshot {}: {e}", path.display()));
        return;
    }

    let committed = fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "read wire snapshot {}: {e}\n\
             If this is a new surface, create it with \
             {UPDATE_ENV}=1 cargo test -p <crate> wire_fingerprint",
            path.display()
        )
    });

    if let Err(report) = check(surface, &committed) {
        panic!("{}\n\nsnapshot: {}", report, path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: TypeDesc = TypeDesc {
        name: "E",
        shape: Shape::Enum(&[
            Variant {
                ordinal: 0,
                name: "One",
                fields: &[],
            },
            Variant {
                ordinal: 1,
                name: "Two",
                fields: &[Field {
                    name: "x",
                    ty: "u32",
                }],
            },
        ]),
    };

    fn surface(types: Vec<TypeDesc>) -> Surface {
        Surface {
            label: "test",
            types,
            leaves: vec![],
        }
    }

    #[test]
    fn identical_surface_passes() {
        let s = surface(vec![A]);
        let rendered = s.render();
        assert!(check(&s, &rendered).is_ok());
    }

    #[test]
    fn a_reordered_variant_is_reported_with_its_line() {
        let committed = surface(vec![A]).render();

        const SWAPPED: TypeDesc = TypeDesc {
            name: "E",
            shape: Shape::Enum(&[
                Variant {
                    ordinal: 0,
                    name: "Two",
                    fields: &[Field {
                        name: "x",
                        ty: "u32",
                    }],
                },
                Variant {
                    ordinal: 1,
                    name: "One",
                    fields: &[],
                },
            ]),
        };

        let report = check(&surface(vec![SWAPPED]), &committed).expect_err("must fail");
        assert!(report.contains("- committed:   0 One"), "{report}");
        assert!(report.contains("+ computed:    0 Two(x: u32)"), "{report}");
        assert!(report.contains("HARD BREAK"), "{report}");
    }

    #[test]
    fn a_retyped_field_moves_the_fingerprint() {
        const RETYPED: TypeDesc = TypeDesc {
            name: "E",
            shape: Shape::Enum(&[
                Variant {
                    ordinal: 0,
                    name: "One",
                    fields: &[],
                },
                Variant {
                    ordinal: 1,
                    name: "Two",
                    fields: &[Field {
                        name: "x",
                        ty: "u64",
                    }],
                },
            ]),
        };

        assert_ne!(
            surface(vec![A]).fingerprint(),
            surface(vec![RETYPED]).fingerprint(),
            "u32 -> u64 must not be invisible"
        );
    }

    #[test]
    fn a_leaf_whose_bytes_change_is_reported() {
        let base = Surface {
            label: "test",
            types: vec![],
            leaves: vec![Leaf {
                name: "L",
                bytes: vec![0; 4],
            }],
        };
        let grown = Surface {
            label: "test",
            types: vec![],
            leaves: vec![Leaf {
                name: "L",
                bytes: vec![0; 8],
            }],
        };
        let report = check(&grown, &base.render()).expect_err("must fail");
        assert!(report.contains("leaf L len=4"), "{report}");
        assert!(report.contains("leaf L len=8"), "{report}");
    }
}
