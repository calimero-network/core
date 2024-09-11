#![allow(non_snake_case)]

use borsh::to_vec;
use claims::assert_err;

use super::*;

const TEST_UUID: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

#[cfg(test)]
mod id__public_methods {
    use super::*;

    #[test]
    fn as_bytes() {
        assert_eq!(Id(Uuid::from_bytes(TEST_UUID)).as_bytes(), &TEST_UUID);
    }
}

#[cfg(test)]
mod id__traits {
    use super::*;

    #[test]
    fn borsh_deserialization__valid() {
        assert_eq!(
            Id::try_from_slice(&TEST_UUID).unwrap(),
            Id(Uuid::from_bytes(TEST_UUID))
        );
    }

    #[test]
    fn borsh_deserialization__too_short() {
        assert_err!(Id::try_from_slice(&[1, 2, 3]));
    }

    #[test]
    fn borsh_serialization__valid() {
        let serialized = to_vec(&Id(Uuid::from_bytes(TEST_UUID))).unwrap();
        assert_eq!(serialized.len(), 16);
        assert_eq!(serialized, TEST_UUID);
    }

    #[test]
    fn borsh_serialization__roundtrip() {
        let id1 = Id::new();
        let id2 = Id::try_from_slice(&to_vec(&id1).unwrap()).unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn from__for_uuid() {
        assert_eq!(
            Uuid::from(Id(Uuid::from_bytes(TEST_UUID))).as_bytes(),
            &TEST_UUID
        );
    }
}

#[cfg(test)]
mod path__constructor {
    use super::*;

    #[test]
    fn new__valid() {
        let path1 = Path::new("::root").unwrap();
        assert_eq!(path1.path, "root");
        assert_eq!(path1.offsets, Vec::<u8>::new());

        let path2 = Path::new("::root::node").unwrap();
        assert_eq!(path2.path, "rootnode");
        assert_eq!(path2.offsets, vec![4]);

        let path3 = Path::new("::root::node::leaf").unwrap();
        assert_eq!(path3.path, "rootnodeleaf");
        assert_eq!(path3.offsets, vec![4, 8]);

        let uni2 = Path::new("::root::ø::leaf").unwrap();
        assert_eq!(uni2.path, "rootøleaf");
        assert_eq!(uni2.offsets, vec![4, 6]);

        let uni3 = Path::new("::root::अ::leaf").unwrap();
        assert_eq!(uni3.path, "rootअleaf");
        assert_eq!(uni3.offsets, vec![4, 7]);

        let uni4 = Path::new("::root::🙂::leaf").unwrap();
        assert_eq!(uni4.path, "root🙂leaf");
        assert_eq!(uni4.offsets, vec![4, 8]);
    }

    #[test]
    fn new__valid_and_deep() {
        let path = Path::new("::a".repeat(255)).unwrap();
        assert_eq!(path.path, "a".repeat(255).as_str());
        assert_eq!(path.offsets, (1..=254).collect::<Vec<_>>());
    }

    #[test]
    fn new__valid_and_long() {
        let path = Path::new(format!("::{}", "a".repeat(255))).unwrap();
        assert_eq!(path.path, "a".repeat(255).as_str());
        assert_eq!(path.offsets, Vec::<u8>::new());
    }

    #[test]
    fn new__empty() {
        let err = Path::new("");
        assert_err!(err);
        assert_eq!(err.unwrap_err(), PathError::Empty);
    }

    #[test]
    fn new__not_absolute() {
        let err = Path::new("root::node::leaf");
        assert_err!(err);
        assert_eq!(err.unwrap_err(), PathError::NotAbsolute);
    }

    #[test]
    fn new__only_separators() {
        let err1 = Path::new("::");
        assert_err!(err1);
        assert_eq!(err1.unwrap_err(), PathError::EmptySegment);

        let err2 = Path::new("::::");
        assert_err!(err2);
        assert_eq!(err2.unwrap_err(), PathError::EmptySegment);
    }

    #[test]
    fn new__too_long() {
        let err1 = Path::new(format!("::{}", "a".repeat(256)));
        assert_err!(err1);
        assert_eq!(err1.unwrap_err(), PathError::Overflow);

        let err2 = Path::new("::a".repeat(256));
        assert_err!(err2);
        assert_eq!(err2.unwrap_err(), PathError::Overflow);
    }

    #[test]
    fn new__with_empty_segments() {
        let err1 = Path::new("::::");
        assert_err!(err1);
        assert_eq!(err1.unwrap_err(), PathError::EmptySegment);

        let err2 = Path::new("::root::::leaf");
        assert_err!(err2);
        assert_eq!(err2.unwrap_err(), PathError::EmptySegment);

        let err3 = Path::new("::root::node::leaf::");
        assert_err!(err3);
        assert_eq!(err3.unwrap_err(), PathError::EmptySegment);
    }
}

#[cfg(test)]
mod path__public_methods {
    use super::*;

    #[test]
    fn depth() {
        assert_eq!(Path::new("::root").unwrap().depth(), 0);
        assert_eq!(Path::new("::root::node::leaf").unwrap().depth(), 2);
    }

    #[test]
    fn first() {
        assert_eq!(Path::new("::root").unwrap().first(), "root");
        assert_eq!(Path::new("::root::node::leaf").unwrap().first(), "root");
    }

    #[test]
    fn is_ancestor_of() {
        assert!(Path::new("::root::node")
            .unwrap()
            .is_ancestor_of(&Path::new("::root::node::leaf").unwrap()));
        assert!(Path::new("::root")
            .unwrap()
            .is_ancestor_of(&Path::new("::root::node::leaf").unwrap()));

        assert!(!Path::new("::root::node::leaf")
            .unwrap()
            .is_ancestor_of(&Path::new("::root::node").unwrap()));
        assert!(!Path::new("::root::node")
            .unwrap()
            .is_ancestor_of(&Path::new("::root::node").unwrap()));
        assert!(!Path::new("::root::node")
            .unwrap()
            .is_ancestor_of(&Path::new("::root::another").unwrap()));
    }

    #[test]
    fn is_descendant_of() {
        assert!(Path::new("::root::node::leaf")
            .unwrap()
            .is_descendant_of(&Path::new("::root::node").unwrap()));
        assert!(Path::new("::root::node::leaf")
            .unwrap()
            .is_descendant_of(&Path::new("::root").unwrap()));

        assert!(!Path::new("::root::node")
            .unwrap()
            .is_descendant_of(&Path::new("::root::node::leaf").unwrap()));
        assert!(!Path::new("::root::node")
            .unwrap()
            .is_descendant_of(&Path::new("::root::node").unwrap()));
        assert!(!Path::new("::root::node")
            .unwrap()
            .is_descendant_of(&Path::new("::root::another").unwrap()));
    }

    #[test]
    fn is_root() {
        assert!(Path::new("::root").unwrap().is_root());
        assert!(!Path::new("::root::node").unwrap().is_root());
    }

    #[test]
    fn join() {
        let path1 = Path::new("::root::node").unwrap();
        let path2 = Path::new("::leaf").unwrap();
        let joined1 = path1.join(&path2).unwrap();
        assert_eq!(joined1.to_string(), "::root::node::leaf");

        let path3 = Path::new("::root").unwrap();
        let path4 = Path::new("::node::leaf").unwrap();
        let joined2 = path3.join(&path4).unwrap();
        assert_eq!(joined2.to_string(), "::root::node::leaf");
    }

    #[test]
    fn last() {
        let path1 = Path::new("::root").unwrap();
        assert_eq!(path1.last(), "root");

        let path2 = Path::new("::root::node::leaf").unwrap();
        assert_eq!(path2.last(), "leaf");
    }

    #[test]
    fn parent() {
        assert_eq!(
            Path::new("::root::node::leaf")
                .unwrap()
                .parent()
                .unwrap()
                .to_string(),
            "::root::node"
        );
        assert!(Path::new("::root").unwrap().parent().is_none());
    }

    #[test]
    fn segment() {
        let path = Path::new("::root::node::leaf").unwrap();
        assert_eq!(path.segment(0).unwrap(), "root");
        assert_eq!(path.segment(1).unwrap(), "node");
        assert_eq!(path.segment(2).unwrap(), "leaf");
        assert!(path.segment(3).is_none());
    }

    #[test]
    fn segments() {
        let path = Path::new("::root::node::leaf").unwrap();
        assert_eq!(
            path.segments().collect::<Vec<_>>(),
            vec!["root", "node", "leaf"]
        );
    }
}

#[cfg(test)]
mod path__traits {
    use super::*;

    #[test]
    fn borsh_deserialization__valid() {
        assert_eq!(
            Path::try_from_slice(&to_vec("::root::node::leaf").unwrap()).unwrap(),
            Path::new("::root::node::leaf").unwrap()
        );
    }

    #[test]
    fn borsh_deserialization__invalid() {
        assert_err!(Path::try_from_slice(&[1, 2, 3]));
    }

    #[test]
    fn borsh_serialization__valid() {
        let path = Path::new("::root::node::leaf").unwrap();
        let serialized = to_vec(&path).unwrap();
        assert_eq!(serialized, to_vec("::root::node::leaf").unwrap());
    }

    #[test]
    fn borsh_serialization__roundtrip() {
        let path1 = Path::new("::root::node::leaf").unwrap();
        let path2 = Path::try_from_slice(&to_vec(&path1).unwrap()).unwrap();
        assert_eq!(path1, path2);
    }

    #[test]
    fn display() {
        let path = Path::new("::root::node::leaf").unwrap();
        assert_eq!(format!("{path}"), "::root::node::leaf");
        assert_eq!(path.to_string(), "::root::node::leaf");
    }

    #[test]
    fn from__for_string() {
        let path = Path::new("::root::node::leaf").unwrap();
        assert_eq!(String::from(path), "::root::node::leaf".to_owned());
    }

    #[test]
    fn try_from__str() {
        let path = Path::try_from("::root::node::leaf").unwrap();
        assert_eq!(path.path, "rootnodeleaf");
        assert_eq!(path.offsets, vec![4, 8]);
    }

    #[test]
    fn try_from__string() {
        let path = Path::try_from("::root::node::leaf".to_owned()).unwrap();
        assert_eq!(path.path, "rootnodeleaf");
        assert_eq!(path.offsets, vec![4, 8]);
    }
}
