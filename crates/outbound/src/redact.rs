//! Redacting a request's secrets from what comes back: the response body, and a
//! failure's reason, which goes to the guest and into the node's log.
//!
//! An upstream that echoes its input may echo a secret encoded rather than as
//! it was sent, so each secret is matched in the encodings an HTTP API commonly
//! applies, worked out once per request ([`encodings`]):
//!
//! - as sent;
//! - hex, lower and upper case;
//! - base64, standard and URL-safe, with and without padding, and the part of it
//!   the secret alone decides when it sits inside a longer base64 value at any
//!   offset (HTTP Basic credentials: `user:secret`);
//! - percent-encoded (RFC 3986 with upper or lower case hex, `encodeURIComponent`,
//!   and a form body's `application/x-www-form-urlencoded`);
//! - inside a JSON string (as `serde_json` escapes, ASCII-only `\u` escapes as
//!   Python writes them, and that with `\/` as PHP writes it).
//!
//! One Aho-Corasick automaton holds every pattern of every secret, so the scan
//! is linear in the bytes scanned, leftmost-longest so a padded form wins over
//! its unpadded prefix. A response is read whole before it is redacted (no
//! streaming), so no match is split across chunks.
//!
//! **Not covered**, so this is defence in depth, not a guarantee: any other
//! transformation (another encoding, nested or double encodings, mixed-case hex,
//! a cipher, compression), a hash or MAC of a secret, and a partial echo (a
//! prefix, a suffix, a slice). An encoded form is matched only for a secret of
//! [`MIN_ENCODED_SECRET_BYTES`] to [`MAX_ENCODED_SECRET_BYTES`] bytes; any other
//! is matched as sent only.

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use eyre::{bail, Result as EyreResult};
use zeroize::Zeroizing;

/// What replaces a secret, or an encoding of one, echoed back.
pub(crate) const REDACTED: &[u8] = b"[redacted]";

/// The most distinct secret values one request may carry; a request with more
/// is refused. Bounds the patterns a request's redaction holds
/// ([`MAX_REDACTION_PATTERNS`]).
pub const MAX_SECRETS_PER_REQUEST: usize = 32;

/// The longest secret whose encodings are redacted. A longer one is redacted
/// only as sent. Bounds the bytes of each pattern (at most 12 per byte of the
/// secret: a 4-byte character as two `\u` escapes).
pub const MAX_ENCODED_SECRET_BYTES: usize = 4096;

/// The shortest secret whose encodings are redacted: a shorter one's encodings
/// are short enough to match ordinary text. It is redacted as sent.
pub const MIN_ENCODED_SECRET_BYTES: usize = 8;

/// The most patterns one secret contributes: itself, 2 hex, 10 base64, 4
/// percent-encoded, 3 JSON.
pub const PATTERNS_PER_SECRET: usize = 20;

/// The most patterns one request's redaction holds.
pub const MAX_REDACTION_PATTERNS: usize = MAX_SECRETS_PER_REQUEST * PATTERNS_PER_SECRET;

/// Replaces every secret of one request, and its encodings, in what comes back.
pub(crate) struct Redactor {
    /// `None`: the request carried no secret.
    matcher: Option<AhoCorasick>,
    patterns: usize,
}

/// The pattern count only: the patterns are secrets.
impl core::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Redactor")
            .field("patterns", &self.patterns)
            .finish()
    }
}

impl Redactor {
    /// The redaction of `secrets` (each as often as it was substituted).
    ///
    /// # Errors
    /// More than [`MAX_SECRETS_PER_REQUEST`] distinct values, or the automaton
    /// cannot be built.
    pub(crate) fn new(secrets: &[Zeroizing<String>]) -> EyreResult<Self> {
        let mut distinct: Vec<&str> = secrets
            .iter()
            .map(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        distinct.sort_unstable();
        distinct.dedup();
        if distinct.len() > MAX_SECRETS_PER_REQUEST {
            bail!("a request may carry at most {MAX_SECRETS_PER_REQUEST} distinct secrets");
        }
        let mut patterns: Vec<Zeroizing<Vec<u8>>> = distinct
            .iter()
            .flat_map(|secret| encodings(secret))
            .collect();
        patterns.sort_unstable_by(|a, b| a.as_slice().cmp(b.as_slice()));
        patterns.dedup_by(|a, b| a.as_slice() == b.as_slice());
        if patterns.is_empty() {
            return Ok(Self {
                matcher: None,
                patterns: 0,
            });
        }
        let matcher = AhoCorasickBuilder::new()
            .match_kind(MatchKind::LeftmostLongest)
            .build(patterns.iter().map(|p| p.as_slice()))?;
        Ok(Self {
            matcher: Some(matcher),
            patterns: patterns.len(),
        })
    }

    /// `bytes` with every match replaced by [`REDACTED`].
    pub(crate) fn redact(&self, bytes: Vec<u8>) -> Vec<u8> {
        match &self.matcher {
            None => bytes,
            Some(matcher) => {
                let mut out = Vec::with_capacity(bytes.len());
                matcher.replace_all_with_bytes(&bytes, &mut out, |_, _, out| {
                    out.extend_from_slice(REDACTED);
                    true
                });
                out
            }
        }
    }

    /// `text` with every match replaced by [`REDACTED`]. Every pattern is UTF-8,
    /// so a match never splits a character.
    pub(crate) fn redact_str(&self, text: String) -> String {
        match &self.matcher {
            None => text,
            Some(matcher) => {
                let mut out = String::with_capacity(text.len());
                matcher.replace_all_with(&text, &mut out, |_, _, out| {
                    out.push_str("[redacted]");
                    true
                });
                out
            }
        }
    }
}

/// `secret` and the encodings of it an upstream might echo, at most
/// [`PATTERNS_PER_SECRET`], possibly repeated.
pub(crate) fn encodings(secret: &str) -> Vec<Zeroizing<Vec<u8>>> {
    let raw = secret.as_bytes();
    let mut out = vec![Zeroizing::new(raw.to_vec())];
    if !(MIN_ENCODED_SECRET_BYTES..=MAX_ENCODED_SECRET_BYTES).contains(&raw.len()) {
        return out;
    }
    out.push(hex(raw, b"0123456789abcdef"));
    out.push(hex(raw, b"0123456789ABCDEF"));
    for (padded, unpadded) in [(STANDARD, STANDARD_NO_PAD), (URL_SAFE, URL_SAFE_NO_PAD)] {
        out.push(Zeroizing::new(padded.encode(raw).into_bytes()));
        out.push(Zeroizing::new(unpadded.encode(raw).into_bytes()));
        // Inside a longer value, `shift` bytes into a 3-byte group: the
        // characters only the secret's bits decide.
        for shift in 0..3 {
            let mut shifted = Zeroizing::new(vec![0; shift]);
            shifted.extend_from_slice(raw);
            let encoded = Zeroizing::new(unpadded.encode(shifted.as_slice()).into_bytes());
            let first = (8 * shift).div_ceil(6);
            let last = 8 * shifted.len() / 6;
            out.push(Zeroizing::new(encoded[first..last].to_vec()));
        }
    }
    out.push(percent(raw, b"", true));
    out.push(percent(raw, b"", false));
    out.push(percent(raw, b"!'()*", true));
    out.push(Zeroizing::new(
        url::form_urlencoded::byte_serialize(raw)
            .collect::<String>()
            .into_bytes(),
    ));
    out.push(json(secret, false, false));
    out.push(json(secret, true, false));
    out.push(json(secret, true, true));
    debug_assert!(out.len() <= PATTERNS_PER_SECRET);
    out
}

fn hex(raw: &[u8], digits: &[u8; 16]) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(Vec::with_capacity(raw.len() * 2));
    for b in raw {
        out.push(digits[usize::from(b >> 4)]);
        out.push(digits[usize::from(b & 0xf)]);
    }
    out
}

/// Percent-encodes all but RFC 3986's unreserved characters and `keep`.
fn percent(raw: &[u8], keep: &[u8], upper: bool) -> Zeroizing<Vec<u8>> {
    let digits: &[u8; 16] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    let mut out = Zeroizing::new(Vec::with_capacity(raw.len() * 3));
    for &b in raw {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) || keep.contains(&b) {
            out.push(b);
        } else {
            out.extend_from_slice(&[
                b'%',
                digits[usize::from(b >> 4)],
                digits[usize::from(b & 0xf)],
            ]);
        }
    }
    out
}

/// `secret` as the inside of a JSON string: `"`, `\` and control characters
/// escaped; with `ascii`, every non-ASCII character as `\u` escapes (lower
/// case, UTF-16); with `slash`, `/` as `\/`.
fn json(secret: &str, ascii: bool, slash: bool) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(String::with_capacity(secret.len() * 2));
    for c in secret.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '/' if slash => out.push_str("\\/"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' || (ascii && !c.is_ascii()) => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
            c => out.push(c),
        }
    }
    Zeroizing::new(core::mem::take(&mut *out).into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redactor(secrets: &[&str]) -> Redactor {
        let secrets: Vec<_> = secrets
            .iter()
            .map(|s| Zeroizing::new((*s).to_owned()))
            .collect();
        Redactor::new(&secrets).expect("a redactor")
    }

    #[test]
    fn json_escapes_match_serde_json() {
        let secret = "a\"b\\c\n\u{1}é😀/";
        let serde = serde_json::to_string(secret).expect("json");
        assert_eq!(
            json(secret, false, false).as_slice(),
            &serde.as_bytes()[1..serde.len() - 1]
        );
        assert_eq!(
            String::from_utf8(json(secret, true, true).to_vec()).expect("utf-8"),
            r#"a\"b\\c\n\u0001\u00e9\ud83d\ude00\/"#
        );
    }

    #[test]
    fn a_secret_contributes_at_most_its_share_of_patterns() {
        for secret in ["k3y/+=\"é\"?>~?>~", "abcdefgh", "a b c d e f g h"] {
            assert!(encodings(secret).len() <= PATTERNS_PER_SECRET);
        }
    }

    #[test]
    fn a_short_or_long_secret_is_matched_as_sent_only() {
        assert_eq!(encodings("short").len(), 1);
        assert_eq!(
            encodings(&"x".repeat(MAX_ENCODED_SECRET_BYTES + 1)).len(),
            1
        );
        let r = redactor(&["short"]);
        assert_eq!(
            r.redact(b"a short one, 73686f7274".to_vec()),
            b"a [redacted] one, 73686f7274"
        );
    }

    #[test]
    fn base64_inside_a_longer_value_is_redacted_at_every_offset() {
        let secret = "0123456789abcdef-secret";
        let r = redactor(&[secret]);
        for prefix in ["", "a", "ab", "abc", "user:"] {
            for suffix in ["", "!", "!!", "!!!"] {
                let whole = format!("{prefix}{secret}{suffix}");
                for engine in [STANDARD, URL_SAFE] {
                    let encoded = engine.encode(&whole);
                    let out =
                        String::from_utf8(r.redact(encoded.clone().into_bytes())).expect("utf-8");
                    assert!(out.contains("[redacted]"), "{whole:?} as {encoded}: {out}");
                    // What is left is at most the characters the neighbours share.
                    assert!(
                        out.len() - "[redacted]".len() <= encoded.len() - 4 * secret.len() / 3 + 4,
                        "{whole:?} as {encoded}: {out}"
                    );
                }
            }
        }
    }

    #[test]
    fn several_secrets_in_one_pass_and_text_left_alone() {
        let r = redactor(&["first-secret", "second-secret", "first-secret"]);
        assert!(r.patterns <= 2 * PATTERNS_PER_SECRET);
        assert_eq!(
            r.redact_str("a first-secret, a 7365636f6e642d736563726574, nothing else".to_owned()),
            "a [redacted], a [redacted], nothing else"
        );
        assert_eq!(redactor(&[]).redact(b"as is".to_vec()), b"as is");
    }

    #[test]
    fn too_many_distinct_secrets_are_refused() {
        let many: Vec<_> = (0..=MAX_SECRETS_PER_REQUEST)
            .map(|i| Zeroizing::new(format!("secret-number-{i}")))
            .collect();
        assert!(Redactor::new(&many[..MAX_SECRETS_PER_REQUEST]).is_ok());
        assert!(Redactor::new(&many).is_err());
        let repeated = vec![Zeroizing::new("same-secret".to_owned()); 1000];
        assert!(Redactor::new(&repeated).is_ok());
    }
}
