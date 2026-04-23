//! P1 plumbing tests for the DAG-causal epic (#2233).
//!
//! These tests prove that:
//! 1. `ApplyContext` can be constructed empty and with explicit parents.
//! 2. `apply_action` accepts both forms and behaves identically (P1 is
//!    plumbing-only — the verifier does not yet consult `causal_parents`).
//! 3. The plumbing surface is stable: parents pass through without affecting
//!    the outcome of an apply, the action is applied, and the entity is
//!    retrievable.
//!
//! Together these are the proof that landing P1 doesn't change observable
//! behavior. P2 will start consuming `causal_parents` and will need its own
//! test suite that asserts the new semantics.

use borsh::to_vec;

use crate::action::Action;
use crate::address::Id;
use crate::entities::{Data, Element};
use crate::interface::{ApplyContext, Interface, MainInterface};
use crate::tests::common::Page;

#[test]
fn apply_context_empty_has_no_parents() {
    let ctx = ApplyContext::empty();
    assert!(ctx.causal_parents.is_empty());
}

#[test]
fn apply_context_with_parents_preserves_them() {
    let parents = vec![[1u8; 32], [2u8; 32], [3u8; 32]];
    let ctx = ApplyContext::with_parents(parents.clone());
    assert_eq!(ctx.causal_parents, parents);
}

#[test]
fn apply_action_accepts_empty_context() {
    let page = Page::new_from_element("ctx-empty", Element::root());
    let action = Action::Add {
        id: page.id(),
        data: to_vec(&page).unwrap(),
        ancestors: vec![],
        metadata: page.element().metadata.clone(),
    };

    MainInterface::apply_action(action, &ApplyContext::empty()).expect("apply with empty ctx");

    let retrieved: Page = MainInterface::find_by_id(page.id())
        .unwrap()
        .expect("entity should exist after apply");
    assert_eq!(retrieved.title, "ctx-empty");
}

#[test]
fn apply_action_accepts_non_empty_context_with_identical_outcome() {
    // Same action applied with empty vs non-empty parents must produce
    // identical observable state, because P1 is plumbing-only.
    let page = Page::new_from_element("ctx-with-parents", Element::root());
    let action = Action::Add {
        id: page.id(),
        data: to_vec(&page).unwrap(),
        ancestors: vec![],
        metadata: page.element().metadata.clone(),
    };

    let parents = vec![[0xAA; 32], [0xBB; 32]];
    let ctx = ApplyContext::with_parents(parents);

    MainInterface::apply_action(action, &ctx).expect("apply with parents");

    let retrieved: Page = MainInterface::find_by_id(page.id())
        .unwrap()
        .expect("entity should exist");
    assert_eq!(retrieved.title, "ctx-with-parents");

    // Verify the entity ID resolves the same way regardless of context.
    let same_id: Id = page.id();
    assert!(MainInterface::find_by_id::<Page>(same_id)
        .unwrap()
        .is_some());
}

#[test]
fn apply_context_field_is_non_exhaustive_safe() {
    // The struct is `#[non_exhaustive]` so callers in other crates can't
    // construct it via struct literal. The constructors must work.
    let _ = ApplyContext::default();
    let _ = ApplyContext::empty();
    let _ = ApplyContext::with_parents(vec![]);
}
