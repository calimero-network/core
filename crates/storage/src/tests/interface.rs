#![allow(non_snake_case)]

use super::*;

#[cfg(test)]
mod interface__constructor {
    use super::*;

    #[test]
    fn new() {
        assert_eq!(Interface::new(), Interface {});
    }
}

#[cfg(test)]
mod interface__public_methods {
    use super::*;

    #[test]
    #[ignore]
    fn find_by_id() {
        todo!()
    }

    #[test]
    #[ignore]
    fn find_by_path() {
        todo!()
    }

    #[test]
    #[ignore]
    fn find_children_by_id() {
        todo!()
    }

    #[test]
    #[ignore]
    fn save() {
        todo!()
    }

    #[test]
    #[ignore]
    fn validate() {
        todo!()
    }
}
