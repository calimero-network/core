use calimero_test_utils::storage::create_test_store;
use claims::{assert_ge, assert_le};

use super::*;
use crate::interface::Interface;
use crate::tests::common::{Paragraphs, Person};

#[cfg(test)]
mod collection__public_methods {
    use super::*;

    #[test]
    fn child_ids() {
        let child_ids = vec![Id::new(), Id::new(), Id::new()];
        let mut paras = Paragraphs::new();
        paras.child_ids = child_ids.clone();
        assert_eq!(paras.child_ids(), &paras.child_ids);
        assert_eq!(paras.child_ids(), &child_ids);
    }

    #[test]
    fn has_children() {
        let mut paras = Paragraphs::new();
        assert!(!paras.has_children());

        let child_ids = vec![Id::new(), Id::new(), Id::new()];
        paras.child_ids = child_ids;
        assert!(paras.has_children());
    }
}

#[cfg(test)]
mod data__public_methods {
    use super::*;

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
