//! The per-scope convergence root.

use crate::scope_root;

#[test]
fn scope_root_combines_all_three_components() {
    let base = scope_root([0u8; 32], [0u8; 32], [0u8; 32]);
    // Changing ANY component (entities, acl, or groups) moves the root —
    // the property that makes a hash-neutral ACL rotation impossible.
    assert_ne!(base, scope_root([1u8; 32], [0u8; 32], [0u8; 32]));
    assert_ne!(base, scope_root([0u8; 32], [1u8; 32], [0u8; 32]));
    assert_ne!(base, scope_root([0u8; 32], [0u8; 32], [1u8; 32]));
}
