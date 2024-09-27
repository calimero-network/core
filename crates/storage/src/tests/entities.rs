use borsh::to_vec;
use calimero_test_utils::storage::create_test_store;
use claims::{assert_ge, assert_le};
use sha2::{Digest, Sha256};

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
    fn calculate_full_merkle_hash__cached_values() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let element = Element::new(&Path::new("::root::node").unwrap());
        let mut page = Page::new_from_element("Node", element);
        assert!(interface.save(page.id(), &mut page).unwrap());
        assert_eq!(interface.children_of(&page.paragraphs).unwrap(), vec![]);

        let child1 = Element::new(&Path::new("::root::node::leaf1").unwrap());
        let child2 = Element::new(&Path::new("::root::node::leaf2").unwrap());
        let child3 = Element::new(&Path::new("::root::node::leaf3").unwrap());
        let mut para1 = Paragraph::new_from_element("Leaf1", child1);
        let mut para2 = Paragraph::new_from_element("Leaf2", child2);
        let mut para3 = Paragraph::new_from_element("Leaf3", child3);
        assert!(interface.save(para1.id(), &mut para1).unwrap());
        assert!(interface.save(para2.id(), &mut para2).unwrap());
        assert!(interface.save(para3.id(), &mut para3).unwrap());
        page.paragraphs.child_info = vec![
            ChildInfo::new(para1.id(), para1.element().merkle_hash()),
            ChildInfo::new(para2.id(), para2.element().merkle_hash()),
            ChildInfo::new(para3.id(), para3.element().merkle_hash()),
        ];
        assert!(interface.save(page.id(), &mut page).unwrap());

        let mut hasher0 = Sha256::new();
        hasher0.update(page.id().as_bytes());
        hasher0.update(&to_vec(&page.title).unwrap());
        hasher0.update(&to_vec(&page.element().metadata).unwrap());
        let expected_hash0: [u8; 32] = hasher0.finalize().into();

        let mut hasher1 = Sha256::new();
        hasher1.update(para1.id().as_bytes());
        hasher1.update(&to_vec(&para1.text).unwrap());
        hasher1.update(&to_vec(&para1.element().metadata).unwrap());
        let expected_hash1: [u8; 32] = hasher1.finalize().into();
        let mut hasher1b = Sha256::new();
        hasher1b.update(expected_hash1);
        let expected_hash1b: [u8; 32] = hasher1b.finalize().into();

        let mut hasher2 = Sha256::new();
        hasher2.update(para2.id().as_bytes());
        hasher2.update(&to_vec(&para2.text).unwrap());
        hasher2.update(&to_vec(&para2.element().metadata).unwrap());
        let expected_hash2: [u8; 32] = hasher2.finalize().into();
        let mut hasher2b = Sha256::new();
        hasher2b.update(expected_hash2);
        let expected_hash2b: [u8; 32] = hasher2b.finalize().into();

        let mut hasher3 = Sha256::new();
        hasher3.update(para3.id().as_bytes());
        hasher3.update(&to_vec(&para3.text).unwrap());
        hasher3.update(&to_vec(&para3.element().metadata).unwrap());
        let expected_hash3: [u8; 32] = hasher3.finalize().into();
        let mut hasher3b = Sha256::new();
        hasher3b.update(expected_hash3);
        let expected_hash3b: [u8; 32] = hasher3b.finalize().into();

        let mut hasher = Sha256::new();
        hasher.update(&expected_hash0);
        hasher.update(&expected_hash1b);
        hasher.update(&expected_hash2b);
        hasher.update(&expected_hash3b);
        let expected_hash: [u8; 32] = hasher.finalize().into();

        assert_eq!(page.calculate_merkle_hash().unwrap(), expected_hash0);
        assert_eq!(
            para1.calculate_full_merkle_hash(&interface, false).unwrap(),
            expected_hash1b
        );
        assert_eq!(
            para2.calculate_full_merkle_hash(&interface, false).unwrap(),
            expected_hash2b
        );
        assert_eq!(
            para3.calculate_full_merkle_hash(&interface, false).unwrap(),
            expected_hash3b
        );
        assert_eq!(
            page.calculate_full_merkle_hash(&interface, false).unwrap(),
            expected_hash
        );
    }

    #[test]
    #[ignore]
    fn calculate_full_merkle_hash__recalculated_values() {
        // TODO: Later, tests should be added for recalculating the hashes, and
        // TODO: especially checking when the data has been interfered with or
        // TODO: otherwise arrived at an invalid state.
        todo!()
    }

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
        let expected_hash = person
            .calculate_full_merkle_hash(&interface, false)
            .unwrap();
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
