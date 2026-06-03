use std::time::{SystemTime, UNIX_EPOCH};

use claims::{assert_ge, assert_le};
use sha2::{Digest, Sha256};
use velcro::btree_map;

use super::*;
use crate::interface::MainInterface;
use crate::tests::common::{Page, Paragraph, Paragraphs, Person};

#[cfg(test)]
mod collection__public_methods {
    use super::*;

    #[test]
    fn name() {
        let _paras = Paragraphs::new();
    }
}

#[cfg(test)]
mod data__public_methods {
    use super::*;

    #[test]
    fn collections() {
        let parent = Element::new(None);
        let page = Page::new_from_element("Node", parent);
        assert_eq!(
            page.collections(),
            btree_map! {
                "Paragraphs".to_owned(): MainInterface::child_info_for(page.id()).unwrap_or_default(),
            }
        );

        let child = Element::new(None);
        let para = Paragraph::new_from_element("Leaf", child);
        assert_eq!(para.collections(), BTreeMap::new());
    }

    #[test]
    fn element() {
        let element = Element::new(None);
        let person = Person {
            name: "Alice".to_owned(),
            age: 30,
            storage: element.clone(),
        };
        assert_eq!(person.element(), &element);
    }

    #[test]
    fn element_mut() {
        let element = Element::new(None);
        let mut person = Person {
            name: "Bob".to_owned(),
            age: 40,
            storage: element.clone(),
        };
        assert!(element.is_dirty);
        assert!(person.element().is_dirty);
        person.element_mut().is_dirty = false;
        assert!(element.is_dirty);
        assert!(!person.element().is_dirty);
    }

    #[test]
    fn id() {
        let element = Element::new(None);
        let id = element.id;
        let person = Person {
            name: "Eve".to_owned(),
            age: 20,
            storage: element,
        };
        assert_eq!(person.id(), id);
    }
}

#[cfg(test)]
mod child_info__constructor {
    use super::*;

    #[test]
    fn new() {
        let id = Id::random();
        let hash = Sha256::digest(b"1").into();
        let info = ChildInfo::new(id, hash, Metadata::default());
        assert_eq!(info.id, id);
        assert_eq!(info.merkle_hash, hash);
    }
}

#[cfg(test)]
mod child_info__public_methods {
    use super::*;

    #[test]
    fn id() {
        let info = ChildInfo::new(
            Id::random(),
            Sha256::digest(b"1").into(),
            Metadata::default(),
        );
        assert_eq!(info.id(), info.id);
    }

    #[test]
    fn merkle_hash() {
        let info = ChildInfo::new(
            Id::random(),
            Sha256::digest(b"1").into(),
            Metadata::default(),
        );
        assert_eq!(info.merkle_hash(), info.merkle_hash);
    }
}

#[cfg(test)]
mod child_info__traits {
    use super::*;

    #[test]
    fn display() {
        let info = ChildInfo::new(
            Id::random(),
            Sha256::digest(b"1").into(),
            Metadata::default(),
        );
        assert_eq!(
            format!("{info}"),
            format!(
                "ChildInfo {}: {}",
                info.id(),
                hex::encode(info.merkle_hash())
            )
        );
        assert_eq!(
            info.to_string(),
            format!(
                "ChildInfo {}: {}",
                info.id(),
                hex::encode(info.merkle_hash())
            )
        );
    }
}

#[cfg(test)]
mod element__constructor {
    use super::*;

    #[test]
    fn new() {
        let timestamp1 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let element = Element::new(None);
        let timestamp2 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        assert_ge!(element.metadata.created_at, timestamp1);
        assert_le!(element.metadata.created_at, timestamp2);
        assert_ge!(*element.metadata.updated_at, timestamp1);
        assert_le!(*element.metadata.updated_at, timestamp2);
        assert!(element.is_dirty);
    }
}

#[cfg(test)]
mod element__public_methods {
    use super::*;

    #[test]
    fn created_at() {
        let timestamp1 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let element = Element::new(None);
        let timestamp2 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        assert_ge!(element.created_at(), timestamp1);
        assert_le!(element.created_at(), timestamp2);
    }

    #[test]
    fn id() {
        let element = Element::new(None);
        assert_eq!(element.id(), element.id);
    }

    #[test]
    fn is_dirty() {
        let element = Element::root();
        assert!(element.is_dirty());

        let mut person = Person {
            name: "Alice".to_owned(),
            age: 30,
            storage: element,
        };
        assert!(MainInterface::save(&mut person).unwrap());
        assert!(!person.element().is_dirty());

        person.element_mut().update();
        assert!(person.element().is_dirty());
    }

    #[test]
    #[ignore]
    fn metadata() {
        todo!()
    }

    #[test]
    fn update() {
        let element = Element::root();
        let updated_at = element.metadata.updated_at;
        let mut person = Person {
            name: "Bob".to_owned(),
            age: 40,
            storage: element,
        };
        assert!(MainInterface::save(&mut person).unwrap());
        assert!(!person.element().is_dirty);

        person.element_mut().update();
        assert!(person.element().is_dirty);
        assert_ge!(person.element().metadata.updated_at, updated_at);
    }

    #[test]
    fn updated_at() {
        let timestamp1 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let element = Element::new(None);
        let mut person = Person {
            name: "Eve".to_owned(),
            age: 20,
            storage: element,
        };
        let timestamp2 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        assert_ge!(person.element().updated_at(), timestamp1);
        assert_le!(person.element().updated_at(), timestamp2);

        person.element_mut().update();
        let timestamp3 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        assert_ge!(person.element().updated_at(), timestamp2);
        assert_le!(person.element().updated_at(), timestamp3);
    }
}

#[cfg(test)]
mod element__traits {
    use super::*;

    #[test]
    fn display() {
        let element = Element::new(None);
        assert_eq!(format!("{element}"), format!("Element {}", element.id()));
        assert_eq!(element.to_string(), format!("Element {}", element.id()));
    }
}

#[cfg(test)]
mod metadata__constructor {

    #[test]
    #[ignore]
    fn new() {
        todo!()
    }
}

mod metadata__schema_version {
    use super::*;
    use crate::index::Index;
    use crate::store::{MockedStorage, StorageAdaptor};

    #[test]
    fn schema_version_defaults_none() {
        let m = Metadata::new(1, 1);
        assert_eq!(
            m.schema_version, None,
            "legacy/unmarked entries carry no schema tag"
        );
    }

    #[test]
    fn with_schema_version_sets_tag() {
        let m = Metadata::new(1, 1).with_schema_version(2);
        assert_eq!(m.schema_version, Some(2));
        assert_eq!(m.schema_version(), Some(2));
    }

    #[test]
    fn constructors_default_schema_version_none() {
        use calimero_primitives::crdt::CrdtType;

        assert_eq!(Metadata::new(1, 1).schema_version, None);
        assert_eq!(
            Metadata::with_crdt_type(1, 1, CrdtType::GCounter).schema_version,
            None
        );
        assert_eq!(
            Metadata::with_field_name(1, 1, "f".to_owned()).schema_version,
            None
        );
        assert_eq!(
            Metadata::with_crdt_type_and_field_name(1, 1, CrdtType::GCounter, "f".to_owned())
                .schema_version,
            None
        );
    }

    /// Drives `data` through the REAL persistence path (`save_raw` →
    /// `save_internal`, where `own_hash = Sha256(final_data)` and the parent
    /// `full_hash` is recomputed) under `metadata`, returning the leaf's
    /// `(full_hash, own_hash)` AND the parent's `(full_hash, _)` as actually
    /// recorded in the Merkle index. Each call runs in an isolated store
    /// (distinct `MockedStorage` const generic), so the two invocations cannot
    /// cross-contaminate. This is the same computation the sync layer compares
    /// peer-to-peer — not a hand-rolled `Sha256(data)` re-statement.
    fn persisted_hashes<S: StorageAdaptor>(
        parent: Id,
        leaf: Id,
        data: &[u8],
        metadata: Metadata,
    ) -> (([u8; 32], [u8; 32]), [u8; 32]) {
        use crate::action::Action;
        use crate::interface::{ApplyContext, Interface};

        // Register the parent (root) and the leaf under it in the index so
        // `save_raw` doesn't reject the leaf as an orphan.
        Interface::<S>::apply_action(
            Action::Add {
                id: leaf,
                data: data.to_vec(),
                ancestors: vec![ChildInfo::new(parent, [0_u8; 32], Metadata::new(1, 1))],
                metadata: Metadata::new(1, 1),
            },
            &ApplyContext::empty(),
        )
        .expect("seed leaf under parent");

        // Now persist the value under the metadata-under-test through the real
        // digest path. `save_raw` stamps `own_hash = Sha256(final_data)` and
        // propagates the new `full_hash` up to the parent.
        Interface::<S>::save_raw(leaf, data.to_vec(), metadata).expect("save_raw leaf");

        let leaf_hashes = Index::<S>::get_hashes_for(leaf)
            .expect("leaf hashes")
            .expect("leaf index present");
        let (parent_full, _) = Index::<S>::get_hashes_for(parent)
            .expect("parent hashes")
            .expect("parent index present");
        (leaf_hashes, parent_full)
    }

    /// Real-behavior guard for the CORE 6c.1 invariant: `Metadata.schema_version`
    /// is Merkle-invisible. We persist BYTE-IDENTICAL data under two metadata
    /// that differ ONLY in `schema_version` (`None` vs `Some(7)`) and assert the
    /// resulting `own_hash`, leaf `full_hash`, AND parent `full_hash` — every
    /// hash the sync layer compares — are equal. If a future refactor ever
    /// folded metadata into the digest, the tagged store would diverge and this
    /// test would fail. (The previous version hashed `Sha256(data)` twice over
    /// the same input — a tautology that never fed `schema_version` through the
    /// real hasher and would pass even after such a regression.)
    #[test]
    fn schema_version_does_not_affect_own_hash() {
        let parent = Id::new([0x40; 32]);
        let leaf = Id::new([0x41; 32]);
        let data = b"identity-gated-value".to_vec();

        // Identical except `schema_version`: one unmarked, one tagged v7.
        let untagged = Metadata::new(1, 1);
        let tagged = Metadata::new(1, 1).with_schema_version(7);

        let ((leaf_full_untagged, own_untagged), parent_full_untagged) =
            persisted_hashes::<MockedStorage<8101>>(parent, leaf, &data, untagged);
        let ((leaf_full_tagged, own_tagged), parent_full_tagged) =
            persisted_hashes::<MockedStorage<8102>>(parent, leaf, &data, tagged);

        assert_eq!(
            own_untagged, own_tagged,
            "schema_version must not change the leaf own_hash (Sha256 over value bytes only)"
        );
        assert_eq!(
            leaf_full_untagged, leaf_full_tagged,
            "schema_version must not change the leaf full_hash"
        );
        assert_eq!(
            parent_full_untagged, parent_full_tagged,
            "schema_version must not change the parent full_hash that propagates to the root"
        );
    }
}
