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
