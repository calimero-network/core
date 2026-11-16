"""The recovery disk's files, as their holder opens them.

A Calimero node (``merod --recovery-dir``) writes each app's backups sealed to
the X25519 public key of whoever upgrades the app: the operator, or the app's
organization or owner account. Only that key's private half opens a file, and
it never reaches a node. This is the holder's side, for operators and scripts:

1. :func:`new_backup_key_pair`, then register the public half
   (``POST /ops/backup-key`` signed by the operator, or ``/orgs/{org}/backup-key``
   / ``/auth/backup-key`` with a passkey);
2. after a loss, check each listed header (``GET /ops/recovery-files``) with
   :func:`verify_header` and open its data key with :func:`open_data_key`;
3. send the keys to the node over the sealed channel: ``POST /ops/recover`` and
   ``POST /ops/apps/{app}/restore {recovery}`` (the operator), or
   ``POST /recover/holder`` (:func:`holder_recovery_body`).

The format is core's ``calimero_client::recovery``: HPKE (RFC 9180) base mode,
DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, AES-256-GCM; the header is JSON signed
by the node's Ed25519 receipt key over :data:`SIGNING_LABEL` and its bytes. The
tests open what core sealed. :func:`verify_attestation` checks that a header's
signing key is an attested node's receipt key, through the caller's quote
verifier (core's ``verify_attestation``).

Needs the ``cryptography`` package (``pip install calimero-client-py[recovery]``).
"""

from __future__ import annotations

import hashlib
import hmac
import json
import os
import struct
from typing import Any, Callable, Dict, Iterable, List, Optional

RECOVERY_FORMAT = 1
SIGNING_LABEL = b"calimero.recovery-backup.v1\x00"
HPKE_INFO = b"calimero.recovery-backup.v1"
HPKE_AAD = b"calimero.recovery-backup.key.v1"

_KEM_SUITE = b"KEM" + (0x0020).to_bytes(2, "big")
_HPKE_SUITE = (
    b"HPKE"
    + (0x0020).to_bytes(2, "big")
    + (0x0001).to_bytes(2, "big")
    + (0x0002).to_bytes(2, "big")
)


def _crypto():
    try:
        from cryptography.hazmat.primitives.asymmetric import ed25519, x25519
        from cryptography.hazmat.primitives.ciphers.aead import AESGCM
    except ImportError as e:  # pragma: no cover - the extra is missing
        raise ImportError(
            "recovery needs the cryptography package: pip install calimero-client-py[recovery]"
        ) from e
    return ed25519, x25519, AESGCM


def holder_key(holder: Dict[str, str]) -> str:
    """``operator``, ``org:<id>`` or ``account:<id>``: how routes name a holder."""
    return (
        "operator"
        if holder["kind"] == "operator"
        else f"{holder['kind']}:{holder['id']}"
    )


def new_backup_key_pair() -> Dict[str, Any]:
    """``{"secret": bytes(32), "public_key": hex}``: keep the secret off every node."""
    secret = os.urandom(32)
    return {"secret": secret, "public_key": backup_public_key(secret)}


def backup_public_key(secret: bytes) -> str:
    """The public key (64 hex) of a backup key's secret."""
    _, x25519, _ = _crypto()
    from cryptography.hazmat.primitives import serialization

    pk = x25519.X25519PrivateKey.from_private_bytes(secret).public_key()
    return pk.public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw
    ).hex()


def verify_header(header: str, signature: str) -> Dict[str, Any]:
    """A listed file's header (the JSON text as signed), checked against the
    node's Ed25519 signature (hex). Refuses a newer format or a bad signature."""
    ed25519, _, _ = _crypto()
    from cryptography.exceptions import InvalidSignature

    parsed = json.loads(header)
    fmt = parsed.get("format")
    if not isinstance(fmt, int) or fmt > RECOVERY_FORMAT:
        raise ValueError(
            f"recovery file format {fmt} is not one this release reads ({RECOVERY_FORMAT})"
        )
    key = ed25519.Ed25519PublicKey.from_public_bytes(
        bytes.fromhex(parsed["signingKey"])
    )
    try:
        key.verify(bytes.fromhex(signature), SIGNING_LABEL + header.encode())
    except InvalidSignature:
        raise ValueError("the node's signature over the header does not hold") from None
    return parsed


def _extract(salt: bytes, ikm: bytes) -> bytes:
    return hmac.new(salt or bytes(32), ikm, hashlib.sha256).digest()


def _expand(prk: bytes, info: bytes, length: int) -> bytes:
    out, previous, i = b"", b"", 1
    while len(out) < length:
        previous = hmac.new(prk, previous + info + bytes([i]), hashlib.sha256).digest()
        out += previous
        i += 1
    return out[:length]


def _labeled_extract(suite: bytes, salt: bytes, label: str, ikm: bytes) -> bytes:
    return _extract(salt, b"HPKE-v1" + suite + label.encode() + ikm)


def _labeled_expand(
    suite: bytes, prk: bytes, label: str, info: bytes, length: int
) -> bytes:
    return _expand(
        prk,
        length.to_bytes(2, "big") + b"HPKE-v1" + suite + label.encode() + info,
        length,
    )


def open_data_key(header: Dict[str, Any], secret: bytes) -> bytes:
    """Open a file's data key (32 bytes) with the holder's secret."""
    _, x25519, AESGCM = _crypto()
    from cryptography.exceptions import InvalidTag

    if backup_public_key(secret) != header["recipient"]:
        raise ValueError("this key is not the one the file was sealed to")
    sk = x25519.X25519PrivateKey.from_private_bytes(secret)
    enc = bytes.fromhex(header["enc"])
    dh = sk.exchange(x25519.X25519PublicKey.from_public_bytes(enc))
    prk = _labeled_extract(_KEM_SUITE, b"", "eae_prk", dh)
    shared = _labeled_expand(
        _KEM_SUITE, prk, "shared_secret", enc + bytes.fromhex(header["recipient"]), 32
    )
    context = (
        b"\x00"
        + _labeled_extract(_HPKE_SUITE, b"", "psk_id_hash", b"")
        + _labeled_extract(_HPKE_SUITE, b"", "info_hash", HPKE_INFO)
    )
    schedule = _labeled_extract(_HPKE_SUITE, shared, "secret", b"")
    key = _labeled_expand(_HPKE_SUITE, schedule, "key", context, 32)
    nonce = _labeled_expand(_HPKE_SUITE, schedule, "base_nonce", context, 12)
    try:
        plain = AESGCM(key).decrypt(
            nonce, bytes.fromhex(header["wrappedKey"]), HPKE_AAD
        )
    except InvalidTag:
        raise ValueError("this key does not open the file") from None
    if len(plain) != 32:
        raise ValueError("the data key is not 32 bytes")
    return plain


def recovery_attestation_nonce(cluster: str, node: int) -> str:
    """The nonce a node's recovery attestation binds (core's ``attestation_nonce``), hex."""
    name = cluster.encode()
    data = (
        b"calimero.recovery-backup.attest.v1\x00"
        + len(name).to_bytes(4, "big")
        + name
        + node.to_bytes(8, "big")
    )
    return hashlib.sha256(data).hexdigest()


def receipt_key_binding(
    receipt_key: bytes, governance: Any, cluster: str, node: int
) -> str:
    """What a receipt-key quote binds after its nonce (core's ``receipt_key_binding``,
    ``receipt-key.v3``), hex: the key, the governance as the node serialized it,
    and the cluster (u32-BE length-prefixed) and node id (u64 BE), so one node's
    quote never stands for another node's or another cluster's."""
    g = hashlib.sha256(json.dumps(governance, separators=(",", ":")).encode()).digest()
    name = cluster.encode()
    return hashlib.sha256(
        b"calimero.tee-attest.receipt-key.v3"
        + receipt_key
        + g
        + struct.pack(">I", len(name))
        + name
        + struct.pack(">Q", node)
    ).hexdigest()


def verify_attestation(
    header: Dict[str, Any], verify_quote: Callable[[str, str, str], bool]
) -> None:
    """Check that the key that signed ``header`` is a node's attested receipt key.

    ``verify_quote(quote_b64, nonce_hex, report_data_suffix_hex)`` must verify
    the quote against Intel's root and trusted measurements and check that its
    report data is the nonce followed by the suffix. Raises ``ValueError``.
    """
    doc = header.get("attestation")
    if not isinstance(doc, dict) or not isinstance(doc.get("quote"), str):
        raise ValueError("the file carries no attestation")
    if doc.get("cluster") != header["cluster"] or doc.get("node") != header["node"]:
        raise ValueError("the attestation is another node's")
    if doc.get("receiptKey") != header["signingKey"]:
        raise ValueError("the attested key is not the one that signed the file")
    nonce = recovery_attestation_nonce(header["cluster"], header["node"])
    if doc.get("nonce") != nonce:
        raise ValueError("the attestation is not over the recovery nonce")
    suffix = receipt_key_binding(
        bytes.fromhex(header["signingKey"]),
        doc.get("governance"),
        header["cluster"],
        header["node"],
    )
    if not verify_quote(doc["quote"], nonce, suffix):
        raise ValueError("the quote was not accepted")


def holder_recovery_body(
    holder: Dict[str, str],
    files: Iterable[Dict[str, str]],
    secret: bytes,
    apps: Optional[List[str]] = None,
    verify_quote: Optional[Callable[[str, str, str], bool]] = None,
) -> Dict[str, Any]:
    """The body of ``POST /recover/holder`` from a holder's listed files
    (``GET /ops/recovery-files`` entries): the newest holder file, the newest
    file of each app in ``apps`` (all when ``None``) and every code file, each
    checked and opened with ``secret``."""
    checked = []
    for f in sorted(files, key=lambda f: f["name"]):
        h = verify_header(f["header"], f["signature"])
        if verify_quote is not None:
            verify_attestation(h, verify_quote)
        if holder_key(h["holder"]) != holder_key(holder):
            raise ValueError(
                f"{f['name']} is {holder_key(h['holder'])}'s, not {holder_key(holder)}'s"
            )
        checked.append((f["name"], h))

    def opened(name: str, h: Dict[str, Any]) -> Dict[str, str]:
        return {"file": name, "key": open_data_key(h, secret).hex()}

    slices = [c for c in checked if c[1]["kind"] == "holder"]
    if not slices:
        raise ValueError(f"no holder file of {holder_key(holder)}")
    newest: Dict[str, Any] = {}
    for name, h in checked:
        if h["kind"] == "app" and (apps is None or h.get("app") in apps):
            newest[h["app"]] = (name, h)
    return {
        "holder": holder_key(holder),
        "slice": opened(*slices[-1]),
        "apps": [opened(n, h) for n, h in newest.values()],
        "code": [opened(n, h) for n, h in checked if h["kind"] == "code"],
    }
