//! Embed/read the app state schema as a `calimero_abi_v1` wasm custom section,
//! so the schema travels inside the bytecode (and is covered by `blob_id`).

use wasmparser::{Parser, Payload};

use crate::schema::Manifest;

const SECTION_NAME: &str = "calimero_abi_v1";

/// Read the embedded state-schema `Manifest`, or `None` if the section is absent
/// or malformed (drives fail-open at the upgrade gate).
pub fn read_embedded_state_schema(wasm: &[u8]) -> Option<Manifest> {
    for payload in Parser::new(0).parse_all(wasm).flatten() {
        if let Payload::CustomSection(reader) = payload {
            if reader.name() == SECTION_NAME {
                return serde_json::from_slice::<Manifest>(reader.data()).ok();
            }
        }
    }
    None
}

/// Return a copy of `wasm` carrying exactly one `calimero_abi_v1` section with
/// `manifest` (replacing any pre-existing one — idempotent).
pub fn write_embedded_state_schema(wasm: &[u8], manifest: &Manifest) -> Vec<u8> {
    let json = serde_json::to_vec(manifest).expect("Manifest serializes");
    let mut out = Vec::with_capacity(wasm.len() + json.len() + 64);

    // Preserve magic + version (8 bytes); fall back to a fresh header if the input
    // is somehow shorter (defensive — real wasm always has it).
    if wasm.len() >= 8 {
        out.extend_from_slice(&wasm[..8]);
    } else {
        out.extend_from_slice(&[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]);
    }

    // Walk top-level sections by raw bytes, copying each verbatim EXCEPT a
    // pre-existing calimero_abi_v1 custom section (which we strip, then re-add once).
    let mut i = 8usize;
    while i < wasm.len() {
        let id = wasm[i];
        let Some((size, size_len)) = read_leb_u32(&wasm[i + 1..]) else {
            break; // malformed section-size LEB — stop copying defensively
        };
        let header_len = 1 + size_len;
        let section_end = i + header_len + size as usize;
        if section_end > wasm.len() {
            break; // truncated/garbage tail — stop copying defensively
        }
        let mut skip = false;
        if id == 0x00 {
            let payload = &wasm[i + header_len..section_end];
            if let Some((name_len, name_len_len)) = read_leb_u32(payload) {
                let name_start = name_len_len;
                let name_end = name_start + name_len as usize;
                if name_end <= payload.len() && &payload[name_start..name_end] == SECTION_NAME.as_bytes() {
                    skip = true;
                }
            }
        }
        if !skip {
            out.extend_from_slice(&wasm[i..section_end]);
        }
        i = section_end;
    }

    out.extend_from_slice(&encode_custom_section(SECTION_NAME, &json));
    out
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
        if shift < 32 {
            result |= ((byte & 0x7f) as u32) << shift;
        }
        i += 1;
        if byte & 0x80 == 0 {
            return Some((result, i));
        }
        shift += 7;
        if shift >= 35 {
            return None; // overlong for a u32 — bail safely
        }
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

    #[test]
    fn round_trip() {
        let wasm = write_embedded_state_schema(&empty_module(), &sample_manifest());
        let got = read_embedded_state_schema(&wasm).expect("section present");
        assert_eq!(got.state_root.as_deref(), Some("Root"));
    }

    #[test]
    fn read_absent_is_none() {
        assert!(read_embedded_state_schema(&empty_module()).is_none());
    }

    #[test]
    fn re_embed_is_idempotent_replace() {
        let wasm1 = write_embedded_state_schema(&empty_module(), &sample_manifest());
        let wasm2 = write_embedded_state_schema(&wasm1, &sample_manifest());
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
        let wasm = write_embedded_state_schema(&empty_module(), &sample_manifest());
        for p in wasmparser::Parser::new(0).parse_all(&wasm) {
            p.expect("output is a valid wasm module");
        }
    }

    #[test]
    fn writer_does_not_panic_on_overlong_leb_section_size() {
        // magic+version, then a custom-section id (0x00) whose size LEB is an
        // overlong 6-byte sequence. Must not panic; output must still be a valid
        // module carrying our section.
        let mut wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        wasm.push(0x00); // custom section id
        wasm.extend_from_slice(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]); // overlong LEB
        let out = write_embedded_state_schema(&wasm, &sample_manifest());
        for p in wasmparser::Parser::new(0).parse_all(&out) {
            p.expect("valid module out");
        }
        assert!(read_embedded_state_schema(&out).is_some());
    }

    #[test]
    fn writer_does_not_panic_on_truncated_leb() {
        // Trailing custom-section id with a truncated (never-terminating) size LEB.
        let mut wasm = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        wasm.push(0x00);
        wasm.extend_from_slice(&[0x80, 0x80]); // continuation bits, no terminator
        let out = write_embedded_state_schema(&wasm, &sample_manifest());
        for p in wasmparser::Parser::new(0).parse_all(&out) {
            p.expect("valid module out");
        }
        assert!(read_embedded_state_schema(&out).is_some());
    }
}
