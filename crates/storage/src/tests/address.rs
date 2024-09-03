#![allow(non_snake_case)]

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
        assert_eq!(path1.offsets, vec![]);

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
        assert_eq!(path.offsets, vec![]);
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
mod path__implementations {
    use super::*;

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
