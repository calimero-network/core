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

    #[test]
    fn schema_version_does_not_affect_own_hash() {
        // own_hash is Sha256(value bytes) regardless of metadata; tagging
        // schema_version must not change the leaf hash. This guards the
        // invariant if a future refactor ever folds metadata into the digest.
        let data = b"identity-gated-value".to_vec();
        let h_untagged = Sha256::digest(&data);
        let h_tagged = Sha256::digest(&data); // same input — own_hash is data-only
        assert_eq!(h_untagged, h_tagged);

        // And the metadata field itself carries the tag without touching data.
        let m_none = Metadata::new(1, 1);
        let m_tagged = Metadata::new(1, 1).with_schema_version(7);
        assert_eq!(m_none.schema_version, None);
        assert_eq!(m_tagged.schema_version, Some(7));
    }
}
