/**
 * Byte-level primitives shared by the sealed transport, the attestation check
 * and the agent gate: hex handling, fixed-width integers and concatenation.
 * Internal — not exported from the package root.
 */

/** Little-endian u32, for borsh fixed-width fields and epoch counters. */
export function u32le(value: number): Uint8Array {
  const out = new Uint8Array(4);
  new DataView(out.buffer).setUint32(0, value, true);
  return out;
}

export function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

export function fromHex(value: string, label: string, expectedBytes: number): Uint8Array {
  const clean = value.trim();
  if (!/^[0-9a-fA-F]*$/.test(clean) || clean.length !== expectedBytes * 2) {
    throw new Error(
      `${label} must be ${expectedBytes * 2} hex characters, got ${clean.length}`,
    );
  }
  // Indexed rather than `match(/../g)!`: the length is already validated above,
  // so the assertion would only be telling the compiler what the guard proved,
  // and this needs no assertion at all.
  const bytes = new Uint8Array(expectedBytes);
  for (let i = 0; i < expectedBytes; i += 1) {
    bytes[i] = Number.parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

/**
 * Decode hex of an unknown length.
 *
 * The sized {@link fromHex} is the right default — every key and digest here
 * has a fixed width, and checking it catches a truncated paste at the edge
 * rather than deep inside a signature check. This one exists for the already-
 * encoded structures whose length is genuinely variable: a device certificate,
 * a login statement, a request signature. It still rejects odd-length and
 * non-hex input, so the only check it gives up is the one that cannot apply.
 */
export function fromHexUnsized(value: string, label: string): Uint8Array {
  const clean = value.trim();
  if (!/^[0-9a-fA-F]*$/.test(clean) || clean.length % 2 !== 0) {
    throw new Error(`${label} must be an even number of hex characters`);
  }
  const bytes = new Uint8Array(clean.length / 2);
  for (let i = 0; i < bytes.length; i += 1) {
    bytes[i] = Number.parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

export function u64le(value: number | bigint): Uint8Array {
  const out = new Uint8Array(8);
  new DataView(out.buffer).setBigUint64(0, BigInt(value), true);
  return out;
}

export function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((total, p) => total + p.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}
