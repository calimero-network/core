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

#[cfg(test)]
mod tests {
    use super::loggable_url;

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
}
