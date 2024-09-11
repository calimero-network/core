#![allow(non_snake_case)]

use super::*;

#[cfg(test)]
mod data__constructor {
    use super::*;

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
        let path = Path::new("::root::node::leaf").unwrap();
        let element = Element::new(&path);
        assert_eq!(element.path, path);
    }
}

#[cfg(test)]
mod element__public_methods {
    use super::*;

    #[test]
    #[ignore]
    fn children() {
        todo!()
    }

    #[test]
    #[ignore]
    fn data() {
        todo!()
    }

    #[test]
    #[ignore]
    fn has_children() {
        todo!()
    }

    #[test]
    fn id() {
        let path = Path::new("::root::node::leaf").unwrap();
        let element = Element::new(&path);
        assert_eq!(element.id(), element.id);
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
    use super::*;

    #[test]
    #[ignore]
    fn new() {
        todo!()
    }
}
