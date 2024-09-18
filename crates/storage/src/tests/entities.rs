#![allow(non_snake_case)]

use claims::{assert_ge, assert_le};

use super::*;
use crate::interface::Interface;
use crate::tests::common::create_test_store;

#[cfg(test)]
mod data__constructor {
    #[test]
    #[ignore]
    fn new() {
        todo!()
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
    fn child_ids() {
        let child_ids = vec![Id::new(), Id::new(), Id::new()];
        let mut element = Element::new(&Path::new("::root::node::leaf").unwrap());
        element.child_ids = child_ids.clone();
        assert_eq!(element.child_ids(), element.child_ids);
        assert_eq!(element.child_ids(), child_ids);
    }

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
    #[ignore]
    fn data() {
        todo!()
    }

    #[test]
    fn has_children() {
        let mut element = Element::new(&Path::new("::root::node::leaf").unwrap());
        assert!(!element.has_children());

        let child_ids = vec![Id::new(), Id::new(), Id::new()];
        element.child_ids = child_ids.clone();
        assert!(element.has_children());
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
        let mut element = Element::new(&Path::new("::root::node::leaf").unwrap());
        assert!(element.is_dirty());

        assert!(interface.save(element.id(), &mut element).unwrap());
        assert!(!element.is_dirty());

        element.update_data(Data {});
        assert!(element.is_dirty());
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
    fn update_data() {
        let (db, _dir) = create_test_store();
        let interface = Interface::new(db);
        let mut element = Element::new(&Path::new("::root::node::leaf").unwrap());
        let updated_at = element.metadata.updated_at;
        assert!(interface.save(element.id(), &mut element).unwrap());
        assert!(!element.is_dirty);

        element.update_data(Data {});
        assert!(element.is_dirty);
        assert_ge!(element.metadata.updated_at, updated_at);
    }

    #[test]
    fn updated_at() {
        let timestamp1 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let mut element = Element::new(&Path::new("::root::node::leaf").unwrap());
        let timestamp2 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        assert_ge!(element.updated_at(), timestamp1);
        assert_le!(element.updated_at(), timestamp2);

        element.update_data(Data {});
        let timestamp3 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        assert_ge!(element.updated_at(), timestamp2);
        assert_le!(element.updated_at(), timestamp3);
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
