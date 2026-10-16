use core::iter;

/// The exclusive upper bound for a byte prefix: the smallest key that does NOT
/// start with `prefix` — the `hi` of a half-open `[prefix, hi)` range that
/// covers exactly "all keys under this prefix", used for prefix scans and
/// range-clears against an ordered byte keyspace (e.g. RocksDB / `BTreeMap`).
///
/// It increments the last non-`0xFF` byte and drops the all-`0xFF` tail. For
/// the degenerate all-`0xFF` (or empty) prefix — where no finite successor
/// exists — it falls back to a longer all-`0xFF` key, which still upper-bounds
/// the scan.
///
/// ## Examples
///
/// ```
/// use calimero_primitives::utils::prefix_upper_bound;
///
/// assert_eq!(prefix_upper_bound(b"abc"), b"abd".to_vec());
/// // A trailing 0xFF is dropped, then the preceding byte is bumped.
/// assert_eq!(prefix_upper_bound(&[0x01, 0xFF]), vec![0x02]);
/// // All-0xFF has no finite successor: fall back to a longer all-0xFF key.
/// assert_eq!(prefix_upper_bound(&[0xFF, 0xFF]), vec![0xFF, 0xFF, 0xFF]);
/// ```
#[must_use]
pub fn prefix_upper_bound(prefix: &[u8]) -> Vec<u8> {
    let mut end = prefix.to_vec();
    while let Some(last) = end.last_mut() {
        if *last == 0xFF {
            let _ = end.pop();
        } else {
            *last += 1;
            return end;
        }
    }
    vec![0xFF; prefix.len() + 1]
}

/// A URL as it may be logged: no userinfo, query or fragment.
///
/// A node's `info` and above are shipped to whoever runs its log pipeline. A
/// URL is the usual place a credential rides along unnoticed: `user:pass@` in
/// the authority, a signed or tokened query string (`?token=`, `?X-Amz-...`).
/// The scheme, host, port and path stay, which is what diagnosis needs; a
/// dropped query is marked so the line still says there was one.
///
/// ## Examples
///
/// ```
/// use calimero_primitives::utils::loggable_url;
///
/// let url = url::Url::parse("https://me:secret@apps.test/a.wasm?token=abc#x").unwrap();
/// assert_eq!(loggable_url(&url), "https://apps.test/a.wasm?[redacted]");
/// ```
#[must_use]
pub fn loggable_url(url: &url::Url) -> String {
    let mut shown = url.clone();
    // Both fail only for URLs that cannot carry userinfo, which have none.
    let _ = shown.set_username("");
    let _ = shown.set_password(None);
    let had_query = shown.query().is_some();
    shown.set_query(None);
    shown.set_fragment(None);
    let mut out = shown.to_string();
    if had_query {
        out.push_str("?[redacted]");
    }
    out
}

/// [`loggable_url`] for a URL held as text. Text that does not parse as a URL
/// is not logged at all: it cannot be told apart from a credential.
#[must_use]
pub fn loggable_url_str(raw: &str) -> String {
    url::Url::parse(raw).map_or_else(|_| "[unparsed url]".to_owned(), |url| loggable_url(&url))
}

/// Creates an iterator that finds and compacts segments of a Rust type name.
///
/// This function scans a path, discarding segments that do not contain generic
/// parameters (`<` or `>`). For segments that do, it truncates them after the
/// last `<` or `>` character.
/// Creates an iterator that splits a Rust-style path by `"::"`.
///
/// ## Examples
///
/// ```
/// use calimero_primitives::utils::compact_path;
///
/// let path = "path::to::Struct<path::to::OtherStruct<Option<Vec<module::Thing<T>>>, U>>";
/// let captures = compact_path(path).collect::<Vec<_>>();
/// assert_eq!(
///     captures,
///     ["Struct<", "OtherStruct<Option<Vec<", "Thing<T>>>, U>>"]
/// );
/// ```
pub fn compact_path(path: &str) -> impl Iterator<Item = &str> {
    const PATH_SEPARATOR: &str = "::";
    const PATH_SEPARATOR_END: &str = ">::";

    let mut path = path;

    iter::from_fn(move || {
        if path.is_empty() {
            return None;
        }

        // TODO: refactor the function to improve the readability.
        loop {
            let idx = path
                .find(PATH_SEPARATOR)
                .map_or(path.len(), |idx| idx + PATH_SEPARATOR.len());
            let (segment, rest) = path.split_at(idx);

            path = rest;

            if rest.is_empty() {
                break Some(segment);
            }

            if segment.ends_with(PATH_SEPARATOR_END) {
                break Some(segment);
            }

            let Some(idx) = segment.rfind(['<', '>']) else {
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

    use super::{compact_path, loggable_url, loggable_url_str, prefix_upper_bound};

    #[test]
    fn a_logged_url_carries_no_credentials() {
        let url = url::Url::parse("https://me:secret@apps.test:8443/a/b.wasm?token=abc&x=1#frag")
            .expect("valid url");
        let shown = loggable_url(&url);
        assert_eq!(shown, "https://apps.test:8443/a/b.wasm?[redacted]");
        for secret in ["me", "secret", "token", "abc", "frag"] {
            assert!(!shown.contains(secret), "{secret} survived in {shown}");
        }
    }

    #[test]
    fn a_plain_url_is_logged_as_is() {
        let url =
            url::Url::parse("http://127.0.0.1:2428/admin-api/kms/challenge").expect("valid url");
        assert_eq!(
            loggable_url(&url),
            "http://127.0.0.1:2428/admin-api/kms/challenge"
        );
    }

    #[test]
    fn unparsed_text_is_not_logged() {
        assert_eq!(loggable_url_str("not a url me:secret"), "[unparsed url]");
        assert_eq!(
            loggable_url_str("https://u:p@registry.test/x?sig=1"),
            "https://registry.test/x?[redacted]"
        );
    }

    #[test]
    fn prefix_upper_bound_normal() {
        // Bumps the last byte; the result is the first key not under `prefix`.
        assert_eq!(prefix_upper_bound(b"abc"), b"abd".to_vec());
        assert_eq!(prefix_upper_bound(&[0x00]), vec![0x01]);
        assert_eq!(prefix_upper_bound(&[0x01, 0x02]), vec![0x01, 0x03]);
    }

    #[test]
    fn prefix_upper_bound_trailing_ff() {
        // Trailing 0xFF bytes are dropped, then the preceding byte is bumped.
        assert_eq!(prefix_upper_bound(&[0x01, 0xFF]), vec![0x02]);
        assert_eq!(prefix_upper_bound(&[0x01, 0xFF, 0xFF]), vec![0x02]);
    }

    #[test]
    fn prefix_upper_bound_all_ff() {
        // No finite successor exists: fall back to a longer all-0xFF key that
        // still upper-bounds the scan.
        assert_eq!(prefix_upper_bound(&[0xFF]), vec![0xFF, 0xFF]);
        assert_eq!(prefix_upper_bound(&[0xFF, 0xFF]), vec![0xFF, 0xFF, 0xFF]);
        // Empty prefix has no last byte either.
        assert_eq!(prefix_upper_bound(&[]), vec![0xFF]);
    }

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

        // The exact rendering here is rustc's, and `type_name` makes no stability
        // promise about it — 1.98 began printing the elided lifetime that 1.88
        // omitted. If a toolchain bump breaks this line, re-read the new output and
        // update it; the `compact_path` assertion below is the actual subject.
        assert_eq!(name, "calimero_primitives::utils::tests::__PRIVATE::<impl calimero_primitives::utils::tests::Thing<_>>::t3::Contained<'_, *const ()>");

        let captures = compact_path(name).collect::<Vec<_>>();

        assert_eq!(captures, ["<", "Thing<_>>::", "Contained<'_, *const ()>"]);
    }

    #[test]
    fn test_type_scoped_t4() {
        let name = Thing::<Vec<u8>>::t4();

        // See the note in `test_type_scoped_t3` about rustc-version dependence.
        assert_eq!(name, "calimero_primitives::utils::tests::__PRIVATE::<impl calimero_primitives::utils::tests::Thing<_>>::t4::Contained<'_, u8>::t5::{{closure}}");

        let captures = compact_path(name).collect::<Vec<_>>();

        assert_eq!(
            captures,
            ["<", "Thing<_>>::", "Contained<'_, u8>::", "{{closure}}"]
        );
    }
}
