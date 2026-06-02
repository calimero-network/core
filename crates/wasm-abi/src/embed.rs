//! Embed/read the app state schema as a `calimero_abi_v1` wasm custom section,
//! so the schema travels inside the bytecode (and is covered by `blob_id`).

use wasmparser::{Parser, Payload};

use crate::schema::Manifest;

const SECTION_NAME: &str = "calimero_abi_v1";

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

impl std::error::Error for EmbedError {}

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
/// `manifest` (replacing any pre-existing one — idempotent). Fails closed on a
/// malformed/truncated module rather than silently emitting a corrupt one.
pub fn write_embedded_state_schema(wasm: &[u8], manifest: &Manifest) -> Result<Vec<u8>, EmbedError> {
    let json = serde_json::to_vec(manifest).map_err(EmbedError::Serialize)?;
    if wasm.len() < 8 {
        return Err(EmbedError::MalformedWasm("input shorter than the 8-byte wasm header"));
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
        let Some(section_end) =
            i.checked_add(header_len).and_then(|x| x.checked_add(size as usize))
        else {
            return Err(EmbedError::MalformedWasm("section length overflow"));
        };
        if section_end > wasm.len() {
            return Err(EmbedError::MalformedWasm("section extends past end of input (truncated)"));
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
        let wasm1 = write_embedded_state_schema(&empty_module(), &sample_manifest()).expect("embed");
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
    fn writer_errors_on_too_short_input() {
        assert!(write_embedded_state_schema(&[0x00, 0x61], &sample_manifest()).is_err());
    }
}
