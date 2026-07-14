//! Embed/read the app state schema as a `calimero_abi_v1` wasm custom section,
//! so the schema travels inside the bytecode (and is covered by `blob_id`).

use wasmparser::{Parser, Payload};

use crate::schema::Manifest;
use crate::validate::ValidationError;

const SECTION_NAME: &str = "calimero_abi_v1";

/// Outcome of reading the embedded state-schema section. The three cases the
/// identity-downgrade gate must treat differently: a usable schema, a schema
/// present but from a newer toolchain than this build understands, and nothing
/// usable at all.
#[derive(Debug)]
pub enum EmbeddedSchema {
    /// A present, structurally-valid manifest of a schema major this build
    /// supports.
    Supported(Manifest),
    /// A section is present and parses as a `Manifest`, but its `schema_version`
    /// names a major this build does not understand (e.g. `wasm-abi/2`). The
    /// node cannot enumerate its identity-gated fields, so a security gate must
    /// treat it as "present but opaque" — NOT as "absent".
    UnsupportedVersion(String),
    /// No `calimero_abi_v1` section, or one that is malformed / structurally
    /// invalid (bad JSON, dangling refs, …) — indistinguishable from no schema.
    Absent,
}

impl EmbeddedSchema {
    /// The manifest when present and supported, else `None`. Collapses both
    /// `Absent` and `UnsupportedVersion` to "no usable schema" — for readers that
    /// only consume a schema they can understand and are NOT making a security
    /// (identity-downgrade) decision.
    #[must_use]
    pub fn into_manifest(self) -> Option<Manifest> {
        match self {
            EmbeddedSchema::Supported(manifest) => Some(manifest),
            EmbeddedSchema::UnsupportedVersion(_) | EmbeddedSchema::Absent => None,
        }
    }
}

/// Error from [`write_embedded_state_schema`]. The read path stays infallible
/// (`Option`) for fail-open; the write path is build-time tooling and fails
/// closed on malformed input rather than emitting a corrupt module.
#[derive(Debug)]
pub enum EmbedError {
    Serialize(serde_json::Error),
    MalformedWasm(&'static str),
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbedError::Serialize(e) => write!(f, "failed to serialize manifest: {e}"),
            EmbedError::MalformedWasm(m) => write!(f, "malformed wasm: {m}"),
        }
    }
}

impl std::error::Error for EmbedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EmbedError::Serialize(e) => Some(e),
            EmbedError::MalformedWasm(_) => None,
        }
    }
}

/// Read the embedded state-schema `Manifest`, or `None` if the section is absent,
/// malformed, or tagged with a schema major this build does not support.
///
/// This collapses "unsupported future version" into `None` — fine for readers
/// that only consume a schema they can understand. The identity-downgrade gate
/// must NOT use this: a future-major schema read as `None` would fail open like
/// an absent section. That gate uses [`read_embedded_state_schema_versioned`],
/// which surfaces [`EmbeddedSchema::UnsupportedVersion`] so it can fail closed.
pub fn read_embedded_state_schema(wasm: &[u8]) -> Option<Manifest> {
    read_embedded_state_schema_versioned(wasm).into_manifest()
}

/// Read the embedded state-schema section as a three-way [`EmbeddedSchema`],
/// distinguishing a usable manifest, a present-but-unsupported-version section,
/// and an absent/malformed one. Security gates (identity downgrade) use this so
/// an unsupported future major fails *closed* instead of being mistaken for "no
/// schema".
///
/// The writer emits exactly one section, but this is robust to several. The
/// resolution rules, in priority order:
///   - a `Supported` section is **never** overwritten by an `UnsupportedVersion`
///     one, so a usable schema always wins over an opaque one *regardless of
///     order* — the security property the gate relies on;
///   - among multiple `Supported` sections the last wins (matches the writer's
///     append/replace semantics);
///   - an `UnsupportedVersion` is reported only when no `Supported` section
///     exists (last unsupported wins among themselves);
///   - a malformed / structurally-invalid section is ignored entirely.
#[must_use]
pub fn read_embedded_state_schema_versioned(wasm: &[u8]) -> EmbeddedSchema {
    let mut found = EmbeddedSchema::Absent;
    for payload in Parser::new(0).parse_all(wasm).flatten() {
        if let Payload::CustomSection(reader) = payload {
            if reader.name() == SECTION_NAME {
                // Deserialization alone does not vouch for validity, only that
                // the bytes parse — `validate_manifest` decides usability and
                // separates an unsupported version from genuine malformation.
                if let Ok(manifest) = serde_json::from_slice::<Manifest>(reader.data()) {
                    match crate::validate::validate_manifest(&manifest) {
                        Ok(()) => found = EmbeddedSchema::Supported(manifest),
                        // A newer toolchain's schema: present but opaque. Record
                        // it unless we already hold a usable (supported) one.
                        Err(ValidationError::UnsupportedSchemaVersion(version)) => {
                            if !matches!(found, EmbeddedSchema::Supported(_)) {
                                found = EmbeddedSchema::UnsupportedVersion(version);
                            }
                        }
                        // Malformed / structurally invalid → treat as absent
                        // (leave any prior find untouched).
                        Err(_) => {}
                    }
                }
            }
        }
    }
    found
}

/// Return a copy of `wasm` carrying exactly one `calimero_abi_v1` section with
/// `manifest` (replacing any pre-existing one — idempotent). Fails closed on a
/// malformed/truncated module rather than silently emitting a corrupt one.
pub fn write_embedded_state_schema(
    wasm: &[u8],
    manifest: &Manifest,
) -> Result<Vec<u8>, EmbedError> {
    let json = serde_json::to_vec(manifest).map_err(EmbedError::Serialize)?;
    if wasm.len() < 8 {
        return Err(EmbedError::MalformedWasm(
            "input shorter than the 8-byte wasm header",
        ));
    }
    let mut out = Vec::with_capacity(wasm.len() + json.len() + 64);
    out.extend_from_slice(&wasm[..8]); // magic + version

    let mut i = 8usize;
    while i < wasm.len() {
        let id = wasm[i];
        let Some((size, size_len)) = read_leb_u32(&wasm[i + 1..]) else {
            return Err(EmbedError::MalformedWasm("unparseable section-size LEB"));
        };
        let header_len = 1 + size_len;
        let Some(section_end) = i
            .checked_add(header_len)
            .and_then(|x| x.checked_add(size as usize))
        else {
            return Err(EmbedError::MalformedWasm("section length overflow"));
        };
        if section_end > wasm.len() {
            return Err(EmbedError::MalformedWasm(
                "section extends past end of input (truncated)",
            ));
        }
        let mut skip = false;
        if id == 0x00 {
            let payload = &wasm[i + header_len..section_end];
            if let Some((name_len, name_len_len)) = read_leb_u32(payload) {
                let name_start = name_len_len;
                if let Some(name_end) = name_start.checked_add(name_len as usize) {
                    if name_end <= payload.len()
                        && &payload[name_start..name_end] == SECTION_NAME.as_bytes()
                    {
                        skip = true;
                    }
                }
            }
        }
        if !skip {
            out.extend_from_slice(&wasm[i..section_end]);
        }
        i = section_end;
    }

    out.extend_from_slice(&encode_custom_section(SECTION_NAME, &json));
    Ok(out)
}

/// `id(0x00) + leb128(payload_len) + leb128(name_len) + name + data`.
fn encode_custom_section(name: &str, data: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(5 + name.len() + data.len());
    write_leb_u32(name.len() as u32, &mut payload);
    payload.extend_from_slice(name.as_bytes());
    payload.extend_from_slice(data);

    let mut section = Vec::with_capacity(1 + 5 + payload.len());
    section.push(0x00);
    write_leb_u32(payload.len() as u32, &mut section);
    section.extend_from_slice(&payload);
    section
}

fn write_leb_u32(mut v: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if v == 0 {
            break;
        }
    }
}

/// Read an unsigned LEB128 u32; returns `Some((value, bytes_consumed))` on
/// success, or `None` for malformed input (never panics).
///
/// Hardened for hostile/malformed input:
/// - A truncated sequence (continuation bit set but input exhausted) returns
///   `None`.
/// - An overlong (>5-byte) encoding returns `None`.
///
/// Callers treat `None` as an unparseable section and stop copying, which
/// ensures the output is always a structurally valid wasm module.
fn read_leb_u32(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut result = 0u32;
    let mut shift = 0u32;
    let mut i = 0usize;
    loop {
        let Some(&byte) = bytes.get(i) else {
            return None; // truncated: no terminating byte
        };
        i += 1;
        if shift >= 32 {
            return None; // a 6th continuation byte — overlong for a u32
        }
        let payload = (byte & 0x7f) as u32;
        // Reject a byte whose set bits would be truncated by the shift (e.g. a
        // 5th byte > 0x0f, which would overflow u32 bit 31). `(x << s) >> s == x`
        // holds iff no set bit was shifted out.
        if (payload << shift) >> shift != payload {
            return None;
        }
        result |= payload << shift;
        if byte & 0x80 == 0 {
            return Some((result, i));
        }
        shift += 7;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECTION: &str = "calimero_abi_v1";

    fn sample_manifest() -> Manifest {
        serde_json::from_str(
            r#"{"schema_version":"wasm-abi/1","types":{"Root":{"kind":"record","fields":[]}},"methods":[],"events":[],"state_root":"Root"}"#,
        ).unwrap()
    }

    fn empty_module() -> Vec<u8> {
        vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]
    }

    /// A manifest tagged with a schema major this build does not support.
    /// `write_embedded_state_schema` does not validate, so it embeds fine; the
    /// reader is what rejects it.
    fn future_major_manifest() -> Manifest {
        serde_json::from_str(
            r#"{"schema_version":"wasm-abi/2","types":{"Root":{"kind":"record","fields":[]}},"methods":[],"events":[],"state_root":"Root"}"#,
        ).unwrap()
    }

    #[test]
    fn versioned_reads_supported() {
        let wasm = write_embedded_state_schema(&empty_module(), &sample_manifest()).expect("embed");
        assert!(matches!(
            read_embedded_state_schema_versioned(&wasm),
            EmbeddedSchema::Supported(_)
        ));
    }

    #[test]
    fn versioned_surfaces_unsupported_future_major() {
        let wasm =
            write_embedded_state_schema(&empty_module(), &future_major_manifest()).expect("embed");
        // The gate-facing reader keeps the section visible as opaque-but-present,
        // so the gate can fail closed rather than mistake it for "no schema".
        match read_embedded_state_schema_versioned(&wasm) {
            EmbeddedSchema::UnsupportedVersion(v) => assert_eq!(v, "wasm-abi/2"),
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }

    #[test]
    fn versioned_absent_for_module_without_section() {
        assert!(matches!(
            read_embedded_state_schema_versioned(&empty_module()),
            EmbeddedSchema::Absent
        ));
    }

    /// Build a module with two raw `calimero_abi_v1` sections in the given JSON
    /// order, bypassing the writer's dedup (which would keep only one).
    fn module_with_two_sections(first: &Manifest, second: &Manifest) -> Vec<u8> {
        let mut wasm = empty_module();
        wasm.extend_from_slice(&encode_custom_section(
            SECTION_NAME,
            &serde_json::to_vec(first).unwrap(),
        ));
        wasm.extend_from_slice(&encode_custom_section(
            SECTION_NAME,
            &serde_json::to_vec(second).unwrap(),
        ));
        wasm
    }

    #[test]
    fn versioned_supported_wins_over_a_later_unsupported_section() {
        // Supported first, UnsupportedVersion appended after: the supported one
        // must still win. This is the security property — an opaque section can
        // never demote a usable one — so it is pinned in both orderings.
        let wasm = module_with_two_sections(&sample_manifest(), &future_major_manifest());
        assert!(matches!(
            read_embedded_state_schema_versioned(&wasm),
            EmbeddedSchema::Supported(_)
        ));
    }

    #[test]
    fn versioned_later_supported_wins_over_earlier_unsupported_section() {
        // UnsupportedVersion first, Supported appended after: last-wins for a
        // usable schema, so the result is still Supported.
        let wasm = module_with_two_sections(&future_major_manifest(), &sample_manifest());
        assert!(matches!(
            read_embedded_state_schema_versioned(&wasm),
            EmbeddedSchema::Supported(_)
        ));
    }

    #[test]
    fn option_reader_collapses_unsupported_to_none() {
        // The convenience `Option` reader still hides the unsupported section
        // (fail-open) — which is exactly why the gate must use the versioned one.
        let wasm =
            write_embedded_state_schema(&empty_module(), &future_major_manifest()).expect("embed");
        assert!(read_embedded_state_schema(&wasm).is_none());
    }

    #[test]
    fn round_trip() {
        let wasm = write_embedded_state_schema(&empty_module(), &sample_manifest()).expect("embed");
        let got = read_embedded_state_schema(&wasm).expect("section present");
        assert_eq!(got.state_root.as_deref(), Some("Root"));
    }

    #[test]
    fn read_absent_is_none() {
        assert!(read_embedded_state_schema(&empty_module()).is_none());
    }

    #[test]
    fn re_embed_is_idempotent_replace() {
        let wasm1 =
            write_embedded_state_schema(&empty_module(), &sample_manifest()).expect("embed");
        let wasm2 = write_embedded_state_schema(&wasm1, &sample_manifest()).expect("embed");
        let count = wasmparser::Parser::new(0)
            .parse_all(&wasm2)
            .filter_map(Result::ok)
            .filter(|p| matches!(p, wasmparser::Payload::CustomSection(c) if c.name() == SECTION))
            .count();
        assert_eq!(count, 1);
        assert!(read_embedded_state_schema(&wasm2).is_some());
    }

    #[test]
    fn produces_a_valid_module() {
        // The output must still parse cleanly end-to-end.
        let wasm = write_embedded_state_schema(&empty_module(), &sample_manifest()).expect("embed");
        for p in wasmparser::Parser::new(0).parse_all(&wasm) {
            p.expect("output is a valid wasm module");
        }
    }

    #[test]
    fn writer_errors_on_overlong_leb_section_size() {
        let mut wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        wasm.push(0x00);
        wasm.extend_from_slice(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]);
        assert!(write_embedded_state_schema(&wasm, &sample_manifest()).is_err());
    }
    #[test]
    fn writer_errors_on_truncated_leb() {
        let mut wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        wasm.push(0x00);
        wasm.extend_from_slice(&[0x80, 0x80]);
        assert!(write_embedded_state_schema(&wasm, &sample_manifest()).is_err());
    }
    #[test]
    fn embed_preserves_other_sections() {
        // A module with a (empty) type section before our embed: the type section
        // must survive and the calimero_abi_v1 section must be added, valid module.
        let mut wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        wasm.extend_from_slice(&[0x01, 0x01, 0x00]); // type section id=1, size=1, count=0
        let out = write_embedded_state_schema(&wasm, &sample_manifest()).expect("embed");
        let (mut has_type, mut has_abi) = (false, false);
        for p in wasmparser::Parser::new(0).parse_all(&out) {
            match p.expect("valid module out") {
                wasmparser::Payload::TypeSection(_) => has_type = true,
                wasmparser::Payload::CustomSection(c) if c.name() == SECTION => has_abi = true,
                _ => {}
            }
        }
        assert!(has_type, "pre-existing type section preserved");
        assert!(has_abi, "calimero_abi_v1 section added");
    }

    #[test]
    fn writer_errors_on_too_short_input() {
        assert!(write_embedded_state_schema(&[0x00, 0x61], &sample_manifest()).is_err());
    }
}
