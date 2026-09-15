//! The one property the domain set has to hold: no two of them are equal, and
//! no two `domain_hash` calls with different splits can produce one another.

use std::collections::HashSet;

use calimero_primitives::identity::domain_hash;

use crate::domain::ALL_DOMAINS;

#[test]
fn signing_domains_are_pairwise_distinct() {
    let unique: HashSet<&[u8]> = ALL_DOMAINS.iter().copied().collect();
    assert_eq!(
        unique.len(),
        ALL_DOMAINS.len(),
        "a shared domain would let a signature be replayed across purposes"
    );
}

#[test]
fn domain_hash_is_not_confusable_by_shifting_bytes() {
    // Length-prefixing is what stops ("ab", "c") and ("a", "bc") colliding.
    assert_ne!(domain_hash(b"ab", &[b"c"]), domain_hash(b"a", &[b"bc"]),);
    assert_ne!(
        domain_hash(b"d", &[b"ab", b"c"]),
        domain_hash(b"d", &[b"a", b"bc"]),
    );
}

/// Known answers, for a signer written in another language.
///
/// The test above proves the two splits *differ*; it cannot tell an
/// implementer whether their own digest is the right one. These can: they are
/// the smallest inputs that exercise every part of the construction — the
/// empty case fixes the domain length prefix on its own, and the two splits
/// fix that a part's prefix is counted separately from the domain's.
///
/// A client that reproduces these three reproduces `domain_hash`. One that
/// forgets the length prefixes matches none of them, which is the failure this
/// catches at the client's own desk rather than as a 403 from a relay.
///
/// They are quoted in `docs/src/content/docs/build/delegated-execution-client.mdx`;
/// changing one means changing the doc and breaking every non-Rust signer.
#[test]
fn domain_hash_has_known_answers() {
    assert_eq!(
        hex::encode(domain_hash(b"", &[])),
        "af5570f5a1810b7af78caf4bc70a660f0df51e42baf91d4de5b2328de0e83dfc",
        "SHA256 of the 8 zero bytes that prefix an empty domain -- NOT SHA256 \
         of the empty string, which is what a client omitting the prefix gets",
    );
    assert_eq!(
        hex::encode(domain_hash(b"ab", &[b"c"])),
        "43ee655579de01ca739b3f95c1c2d3f46d353b2c0df818064ea594506cdb2617",
    );
    assert_eq!(
        hex::encode(domain_hash(b"a", &[b"bc"])),
        "9a8acca1b6c6c0befd3fbc756aed625da998c998f7252e738c4ef061906b9b21",
    );
}
