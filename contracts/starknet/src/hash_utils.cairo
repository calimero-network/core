use core::hash::LegacyHash;
use super::types::ContextIdentity;

// Implement LegacyHash for ContextIdentity
pub impl LegacyHashContextIdentity of LegacyHash<ContextIdentity> {
    fn hash(state: felt252, value: ContextIdentity) -> felt252 {
        LegacyHash::<felt252>::hash(state, value.value)
    }
}

// Implement LegacyHash for (felt252, ContextIdentity)
pub impl LegacyHashContextIdAndIdentity of LegacyHash<(felt252, ContextIdentity)> {
    fn hash(state: felt252, value: (felt252, ContextIdentity)) -> felt252 {
        let (context_id, identity) = value;
        let state = LegacyHash::<felt252>::hash(state, context_id);
        LegacyHash::<ContextIdentity>::hash(state, identity)
    }
}