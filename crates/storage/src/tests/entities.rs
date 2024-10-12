use std::time::{SystemTime, UNIX_EPOCH};

use borsh::to_vec;
use calimero_test_utils::storage::create_test_store;
use claims::{assert_ge, assert_le};
use sha2::{Digest, Sha256};
use velcro::btree_map;

use super::*;
use crate::interface::Interface;
use crate::tests::common::{Page, Paragraph, Paragraphs, Person};

#[cfg(test)]
mod collection__public_methods {
    use super::*;

    #[test]
    fn child_info() {
        let child_info = vec![
            ChildInfo::new(Id::new(), Sha256::digest(b"1").into()),
            ChildInfo::new(Id::new(), Sha256::digest(b"2").into()),
            ChildInfo::new(Id::new(), Sha256::digest(b"3").into()),
        ];
        let mut paras = Paragraphs::new();
        paras.child_info = child_info.clone();
        assert_eq!(paras.child_info(), &paras.child_info);
        assert_eq!(paras.child_info(), &child_info);
    }

    #[test]
    fn has_children() {
        let mut paras = Paragraphs::new();
        assert!(!paras.has_children());

        let child_info = vec![
            ChildInfo::new(Id::new(), Sha256::digest(b"1").into()),
            ChildInfo::new(Id::new(), Sha256::digest(b"2").into()),
            ChildInfo::new(Id::new(), Sha256::digest(b"3").into()),
        ];
        paras.child_info = child_info;
        assert!(paras.has_children());
    }
}

#[cfg(test)]
mod data__public_methods {
    use super::*;

    #[test]
    fn calculate_merkle_hash() {
        let element = Element::new(&Path::new("::root::node::leaf").unwrap());
        let person = Person {
            name: "Alice".to_owned(),
            age: 30,
            storage: element.clone(),
        };

        let mut hasher = Sha256::new();
        hasher.update(person.id().as_bytes());
        hasher.update(&to_vec(&person.name).unwrap());
        hasher.update(&to_vec(&person.age).unwrap());
        hasher.update(&to_vec(&person.element().metadata).unwrap());
        let expected_hash: [u8; 32] = hasher.finalize().into();

        assert_eq!(person.calculate_merkle_hash().unwrap(), expected_hash);
    }

    #[test]
    fn calculate_merkle_hash_for_child__valid() {
        let parent = Element::new(&Path::new("::root::node").unwrap());
        let mut page = Page::new_from_element("Node", parent);
        let child1 = Element::new(&Path::new("::root::node::leaf").unwrap());
        let para1 = Paragraph::new_from_element("Leaf1", child1);

        page.paragraphs
            .child_info
            .push(ChildInfo::new(para1.element().id(), [0; 32]));
        let para1_slice = to_vec(&para1).unwrap();
        let para1_hash = page
            .calculate_merkle_hash_for_child("paragraphs", &para1_slice)
            .unwrap();
        let expected_hash1 = para1.calculate_merkle_hash().unwrap();
        assert_eq!(para1_hash, expected_hash1);

        let child2 = Element::new(&Path::new("::root::node::leaf").unwrap());
        let para2 = Paragraph::new_from_element("Leaf2", child2);
        let para2_slice = to_vec(&para2).unwrap();
        let para2_hash = page
            .calculate_merkle_hash_for_child("paragraphs", &para2_slice)
            .unwrap();
        assert_ne!(para2_hash, para1_hash);
    }

    #[test]
    fn calculate_merkle_hash_for_child__invalid() {
        let parent = Element::new(&Path::new("::root::node").unwrap());
        let mut page = Page::new_from_element("Node", parent);
        let child1 = Element::new(&Path::new("::root::node::leaf").unwrap());
        let para1 = Paragraph::new_from_element("Leaf1", child1);

        page.paragraphs
            .child_info
            .push(ChildInfo::new(para1.element().id(), [0; 32]));
        let invalid_slice = &[0, 1, 2, 3];
        let result = page.calculate_merkle_hash_for_child("paragraphs", invalid_slice);
        assert!(matches!(result, Err(StorageError::DeserializationError(_))));
    }

    #[test]
    fn calculate_merkle_hash_for_child__unknown_collection() {
        let parent = Element::new(&Path::new("::root::node").unwrap());
        let mut page = Page::new_from_element("Node", parent);
        let child = Element::new(&Path::new("::root::node::leaf").unwrap());
        let para = Paragraph::new_from_element("Leaf", child);

        page.paragraphs
            .child_info
            .push(ChildInfo::new(para.element().id(), [0; 32]));
        let para_slice = to_vec(&para).unwrap();
        let result = page.calculate_merkle_hash_for_child("unknown_collection", &para_slice);
        assert!(matches!(
            result,
            Err(StorageError::UnknownCollectionType(_))
        ));
    }

    #[test]
    fn collections() {
        let parent = Element::new(&Path::new("::root::node").unwrap());
        let page = Page::new_from_element("Node", parent);
        assert_eq!(
            page.collections(),
            btree_map! {
                "paragraphs".to_owned(): page.paragraphs.child_info().clone()
            }
        );

        let child = Element::new(&Path::new("::root::node::leaf").unwrap());
        let para = Paragraph::new_from_element("Leaf", child);
        assert_eq!(para.collections(), BTreeMap::new());
    }

    #[test]
    fn element() {
        let path = Path::new("::root::node::leaf").unwrap();
        let element = Element::new(&path);
        let person = Person {
            name: "Alice".to_owned(),
            age: 30,
            storage: element.clone(),
        };
        assert_eq!(person.element(), &element);
    }

    #[test]
    fn element_mut() {
        let path = Path::new("::root::node::leaf").unwrap();
        let element = Element::new(&path);
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
        let path = Path::new("::root::node::leaf").unwrap();
        let element = Element::new(&path);
        let id = element.id;
        let person = Person {
            name: "Eve".to_owned(),
            age: 20,
            storage: element,
        };
        assert_eq!(person.id(), id);
    }

    #[test]
    fn path() {
        let path = Path::new("::root::node::leaf").unwrap();
        let element = Element::new(&path);
        let person = Person {
            name: "Steve".to_owned(),
            age: 50,
            storage: element,
        };
        assert_eq!(person.path(), path);
    }
}

#[cfg(test)]
mod child_info__constructor {
    use super::*;

    #[test]
    fn new() {
        let id = Id::new();
        let hash = Sha256::digest(b"1").into();
        let info = ChildInfo::new(id, hash);
        assert_eq!(info.id, id);
        assert_eq!(info.merkle_hash, hash);
    }
}

#[cfg(test)]
mod child_info__public_methods {
    use super::*;

    #[test]
    fn id() {
        let info = ChildInfo::new(Id::new(), Sha256::digest(b"1").into());
        assert_eq!(info.id(), info.id);
    }

    #[test]
    fn merkle_hash() {
        let info = ChildInfo::new(Id::new(), Sha256::digest(b"1").into());
        assert_eq!(info.merkle_hash(), info.merkle_hash);
    }
}

#[cfg(test)]
mod child_info__traits {
    use super::*;

    #[test]
    fn display() {
        let info = ChildInfo::new(Id::new(), Sha256::digest(b"1").into());
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
        let path = Path::new("::root::node::leaf").unwrap();
        let element = Element::new(&path);
        let timestamp2 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        assert_eq!(element.path, path);
        assert_ge!(element.metadata.created_at, timestamp1);
        assert_le!(element.metadata.created_at, timestamp2);
        assert_ge!(element.metadata.updated_at, timestamp1);
        assert_le!(element.metadata.updated_at, timestamp2);
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
        let element = Element::new(&Path::new("::root::node::leaf").unwrap());
        let timestamp2 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        assert_ge!(element.created_at(), timestamp1);
        assert_le!(element.created_at(), timestamp2);
    }

    #[test]
    fn id() {
        let path = Path::new("::root::node::leaf").unwrap();
        let element = Element::new(&path);
        assert_eq!(element.id(), element.id);
    }

    #[test]
    fn is_dirty() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node::leaf").unwrap());
        assert!(element.is_dirty());

        let mut person = Person {
            name: "Alice".to_owned(),
            age: 30,
            storage: element,
        };
        assert!(interface.save(person.element().id(), &mut person).unwrap());
        assert!(!person.element().is_dirty());

        person.element_mut().update();
        assert!(person.element().is_dirty());
    }

    #[test]
    fn merkle_hash() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node::leaf").unwrap());
        let mut person = Person {
            name: "Steve".to_owned(),
            age: 50,
            storage: element.clone(),
        };
        let expected_hash = interface.calculate_merkle_hash_for(&person, false).unwrap();
        assert_ne!(person.element().merkle_hash(), expected_hash);

        assert!(interface.save(person.element().id(), &mut person).unwrap());
        assert_eq!(person.element().merkle_hash(), expected_hash);
    }

    #[test]
    #[ignore]
    fn metadata() {
        todo!()
    }

    #[test]
    fn path() {
        let path = Path::new("::root::node::leaf").unwrap();
        let element = Element::new(&path);
        assert_eq!(element.path(), element.path);
    }

    #[test]
    fn update() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node::leaf").unwrap());
        let updated_at = element.metadata.updated_at;
        let mut person = Person {
            name: "Bob".to_owned(),
            age: 40,
            storage: element,
        };
        assert!(interface.save(person.element().id(), &mut person).unwrap());
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
        let element = Element::new(&Path::new("::root::node::leaf").unwrap());
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
        let path = Path::new("::root::node::leaf").unwrap();
        let element = Element::new(&path);
        assert_eq!(
            format!("{element}"),
            format!("Element {}: ::root::node::leaf", element.id())
        );
        assert_eq!(
            element.to_string(),
            format!("Element {}: ::root::node::leaf", element.id())
        );
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
