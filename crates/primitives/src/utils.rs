use std::iter;

pub fn compact_path(path: &str) -> impl Iterator<Item = &str> {
    let mut path = path;

    iter::from_fn(move || {
        if path.is_empty() {
            return None;
        }

        loop {
            let idx = path.find("::").map_or(path.len(), |idx| idx + 2);
            let (segment, rest) = path.split_at(idx);

            path = rest;

            if rest.is_empty() {
                break Some(segment);
            }

            if segment.ends_with(">::") {
                break Some(segment);
            }

            let Some(idx) = segment.rfind(&['<', '>']) else {
                continue;
            };

            let (pre, _post) = segment.split_at(idx + 1);

            break Some(pre);
        }
    })
}

#[cfg(test)]
mod tests {
    use core::any::{type_name, type_name_of_val};

    use super::compact_path;

    #[test]
    fn test_compact_path() {
        let path = "path::to::Struct<path::to::OtherStruct<Option<Vec<module::Thing<T>>>, U>>";

        let captures = compact_path(path).collect::<Vec<_>>();

        assert_eq!(
            captures,
            ["Struct<", "OtherStruct<Option<Vec<", "Thing<T>>>, U>>"]
        );
    }

    #[test]
    fn already_compact() {
        let path = "Struct<OtherStruct<Option<Vec<Thing<T>>>, U>>";

        let captures = compact_path(path).collect::<Vec<_>>();

        assert_eq!(captures, ["Struct<OtherStruct<Option<Vec<Thing<T>>>, U>>"]);
    }

    #[test]
    fn test_type_name() {
        let name = type_name::<Vec<[fn() -> u8; 32]>>();

        assert_eq!(name, "alloc::vec::Vec<[fn() -> u8; 32]>");

        let captures = compact_path(name).collect::<Vec<_>>();

        assert_eq!(captures, ["Vec<[fn() -> u8; 32]>"]);
    }

    struct Thing<T> {
        _a: T,
    }

    #[expect(non_local_definitions, reason = "this is intentional")]
    const __PRIVATE: () = {
        fn scoped() {}

        impl<T> Thing<T> {
            fn t1() -> &'static str {
                type_name_of_val(&scoped)
            }

            fn t2() -> &'static str {
                let x = 10;
                let anon = |a: u8| x + a;
                type_name_of_val(&anon)
            }

            fn t3() -> &'static str {
                struct Contained<'a, T> {
                    _a: &'a T,
                }

                type_name::<Contained<'static, *const ()>>()
            }

            fn t4() -> &'static str {
                struct Contained<'a, T> {
                    _a: &'a T,
                }

                impl<T> Contained<'static, T> {
                    fn t5() -> &'static str {
                        let val = async { 10u8 };

                        type_name_of_val(&val)
                    }
                }

                Contained::<'static, u8>::t5()
            }
        }
    };

    #[test]
    fn test_type_scoped_t0() {
        let name = type_name::<Thing<Vec<u8>>>();

        assert_eq!(
            name,
            "calimero_primitives::utils::tests::Thing<alloc::vec::Vec<u8>>"
        );

        let captures = compact_path(name).collect::<Vec<_>>();

        assert_eq!(captures, ["Thing<", "Vec<u8>>"]);
    }

    #[test]
    fn test_type_scoped_t1() {
        let name = Thing::<Vec<u8>>::t1();

        assert_eq!(name, "calimero_primitives::utils::tests::__PRIVATE::scoped");

        let captures = compact_path(name).collect::<Vec<_>>();

        assert_eq!(captures, ["scoped"]);
    }

    #[test]
    fn test_type_scoped_t2() {
        let name = Thing::<Vec<u8>>::t2();

        assert_eq!(name, "calimero_primitives::utils::tests::__PRIVATE::<impl calimero_primitives::utils::tests::Thing<alloc::vec::Vec<u8>>>::t2::{{closure}}");

        let captures = compact_path(name).collect::<Vec<_>>();

        assert_eq!(captures, ["<", "Thing<", "Vec<u8>>>::", "{{closure}}"]);
    }

    #[test]
    fn test_type_scoped_t3() {
        let name = Thing::<Vec<u8>>::t3();

        assert_eq!(name, "calimero_primitives::utils::tests::__PRIVATE::<impl calimero_primitives::utils::tests::Thing<_>>::t3::Contained<*const ()>");

        let captures = compact_path(name).collect::<Vec<_>>();

        assert_eq!(captures, ["<", "Thing<_>>::", "Contained<*const ()>"]);
    }

    #[test]
    fn test_type_scoped_t4() {
        let name = Thing::<Vec<u8>>::t4();

        assert_eq!(name, "calimero_primitives::utils::tests::__PRIVATE::<impl calimero_primitives::utils::tests::Thing<_>>::t4::Contained<u8>::t5::{{closure}}");

        let captures = compact_path(name).collect::<Vec<_>>();

        assert_eq!(
            captures,
            ["<", "Thing<_>>::", "Contained<u8>::", "{{closure}}"]
        );
    }
}
