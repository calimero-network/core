"""The sealed transport under the generated client, and the node's step-ups.

The routes are `calimero_client_py.openapi`, generated from core's OpenAPI
document. A node takes every request but `GET /health` and `GET /attestation`
only through the sealed channel, so the generated code runs on an httpx
transport that hands each request to the native `SealedClient` (attestation,
sealing, the client's credential, the move between nodes and the refresh of a
session all happen there) and returns the node's answer as it came.

The transport also answers the two step-ups a node asks for, and sends the
request again with the proof added to its JSON body:

- `412 CONFIRM_REQUIRED`: a passkey assertion over `details.challenge`, as
  `confirm: {challengeId, assertion}`, from the passkey the client was made
  with (`Client.api(passkey)`);
- `428 OPERATOR_SIGNATURE_REQUIRED`: the operator key's Ed25519 signature over
  `details.message`, as `operatorProof: {nonce, signature}`, with the key
  `Client.use_operator_key` set (`Client.api(operator_signed=True)`), or the
  proof a signer `Client.use_operator_signer` set makes from `details`.

and, when asked (`follow_leader=True`), a `421 NOT_LEADER`: the request goes
to the `details.leaderUrl` the node names, attested under the client's policy,
with the client's API key.

A passkey is anything with `create(options) -> registration` and
`get(challenge: bytes) -> assertion` (as dicts): `SoftPasskey` is one.
"""

import asyncio
import base64
import json
from typing import Any, Dict, List, Optional, Tuple

import httpx

from ._native import CloudError, FeatureNotActive, LoginRequired, WrongNode
from .openapi import AuthenticatedClient

__all__ = [
    "AsyncSealedTransport",
    "SealedTransport",
    "error_from",
    "sealed_client",
]

# Nothing travels outside the channel, so the generated client's base URL is
# only a name: the native client sends each path to the node in use.
BASE_URL = "http://sealed.invalid"

# How many `421 NOT_LEADER` redirects a request follows: a node that holds no
# replica names one that may itself redirect once more.
LEADER_HOPS = 3

Answer = Tuple[int, List[Tuple[str, str]], bytes]


def _json(content: bytes) -> Any:
    try:
        return json.loads(content)
    except ValueError:
        return None


def _refusal(content: bytes) -> Tuple[str, str, Any]:
    """`(code, message, details)` of a node's error answer, read as core's
    client reads it: `ERROR` and no message when it is not the error shape."""
    v = _json(content)
    if not isinstance(v, dict):
        return "ERROR", "", None
    code = v.get("error")
    message = v.get("message")
    return (
        code if isinstance(code, str) else "ERROR",
        message if isinstance(message, str) else "",
        v.get("details"),
    )


def error_from(status: int, content: bytes) -> CloudError:
    """The exception a node's error answer stands for, with the arguments the
    client always raised: `(message, status, code, details)`, `details` as
    JSON text. `LoginRequired` for `401 UNAUTHENTICATED`, `WrongNode` for
    `421 WRONG_NODE` and `FeatureNotActive` for `409 FEATURE_NOT_ACTIVE`, all
    `CloudError`s."""
    code, message, details = _refusal(content)
    args = (
        f"{status} {code}: {message}",
        status,
        code,
        json.dumps(details, separators=(",", ":"), ensure_ascii=False),
    )
    if (status, code) == (401, "UNAUTHENTICATED"):
        return LoginRequired(*args)
    if (status, code) == (421, "WRONG_NODE"):
        return WrongNode(*args)
    if (status, code) == (409, "FEATURE_NOT_ACTIVE"):
        return FeatureNotActive(*args)
    return CloudError(*args)


def _asks(answer: Answer, status: int, code: str) -> Optional[Dict[str, Any]]:
    """The answer's `details` when it is `status` `code`, else `None`."""
    if answer[0] != status:
        return None
    found, _message, details = _refusal(answer[2])
    if found != code:
        return None
    return details if isinstance(details, dict) else {}


def _b64url(text: str) -> bytes:
    return base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))


def _with_proof(
    headers: List[Tuple[str, str]], body: bytes, field: str, proof: Any
) -> Optional[Tuple[List[Tuple[str, str]], bytes]]:
    """The request's JSON body with `field` set to `proof`; `None` when the
    body is not a JSON object (no proof can go in it)."""
    v = _json(body) if body else {}
    if not isinstance(v, dict):
        return None
    v[field] = proof
    kept = [(k, x) for k, x in headers if k.lower() != "content-type"]
    kept.append(("content-type", "application/json"))
    return kept, json.dumps(v, separators=(",", ":")).encode()


class SealedTransport(httpx.BaseTransport):
    """An httpx transport that sends each request through the native client's
    sealed channel, answering the step-ups it is given the means for.

    `repeatable` marks every request safe to send twice (a GET always is):
    a node answering `503` is then left for the next. `retry_unavailable`
    sends it again to the same node for a while first (a call with its
    idempotency key). `once` sends it once (a sign-in or sign-up)."""

    def __init__(
        self,
        native: Any,
        passkey: Any = None,
        *,
        operator_signed: bool = False,
        operator_key: Optional[str] = None,
        operator_signer: Any = None,
        follow_leader: bool = False,
        repeatable: bool = False,
        retry_unavailable: bool = False,
        once: bool = False,
    ):
        self._native = native
        self._passkey = passkey
        self._operator = (
            operator_signed or operator_key is not None or operator_signer is not None
        )
        self._operator_key = operator_key
        self._operator_signer = operator_signer
        self._follow_leader = follow_leader
        self._repeatable = repeatable
        self._retry = retry_unavailable
        self._once = once

    def _mode(self, method: str) -> str:
        if self._once:
            return "once"
        if self._repeatable or method in ("GET", "HEAD"):
            return "repeatable"
        return "run"

    def _send(
        self,
        method: str,
        path: str,
        headers: List[Tuple[str, str]],
        body: bytes,
        leader: Optional[str],
    ) -> Tuple[Answer, Optional[str]]:
        """Send once (to `leader` when one is known), following `NOT_LEADER`
        when asked; answers the node's answer and the leader it came from."""

        def once(at: Optional[str]) -> Answer:
            if at is not None:
                return self._native.request_at(at, method, path, headers, body)
            return self._native.request(
                method, path, headers, body, self._mode(method), self._retry
            )

        answer = once(leader)
        if not self._follow_leader:
            return answer, leader
        for _ in range(LEADER_HOPS):
            details = _asks(answer, 421, "NOT_LEADER")
            url = (details or {}).get("leaderUrl")
            if not isinstance(url, str) or not self._native.has_api_key:
                break
            leader = url
            answer = once(leader)
        return answer, leader

    def exchange(
        self, method: str, path: str, headers: List[Tuple[str, str]], body: bytes
    ) -> Answer:
        """The node's answer to one request, a step-up answered on the way."""
        answer, leader = self._send(method, path, headers, body, None)
        details = _asks(answer, 412, "CONFIRM_REQUIRED")
        if details is not None and self._passkey is not None:
            challenge = details.get("challenge")
            assertion = self._passkey.get(
                _b64url(challenge if isinstance(challenge, str) else "")
            )
            confirm = {
                "challengeId": details.get("challengeId"),
                "assertion": assertion,
            }
            again = _with_proof(headers, body, "confirm", confirm)
            if again is not None:
                answer, _ = self._send(method, path, again[0], again[1], leader)
            return answer
        # A decision is signed for the node that asks; when the signed request
        # was sent on to a leader, the leader asks again with its own nonce.
        signed_at = set()
        while self._operator and leader not in signed_at:
            details = _asks(answer, 428, "OPERATOR_SIGNATURE_REQUIRED")
            if details is None:
                break
            signed_at.add(leader)
            if self._operator_signer is not None:
                # The caller's own signer (it may check the statement first).
                proof = self._operator_signer(dict(details))
            else:
                message = details.get("message")
                signature = self._native.operator_sign(
                    base64.b64decode(message if isinstance(message, str) else ""),
                    self._operator_key,
                )
                if signature is None:
                    break
                proof = {"nonce": details.get("nonce"), "signature": signature}
            again = _with_proof(headers, body, "operatorProof", proof)
            if again is None:
                break
            answer, leader = self._send(method, path, again[0], again[1], leader)
        return answer

    @staticmethod
    def _parts(request: httpx.Request) -> Tuple[str, List[Tuple[str, str]]]:
        path = request.url.raw_path.decode("ascii")
        route = path.split("?", 1)[0]
        if any(s in (".", "..") for s in route.split("/")):
            # The generated calls put a path argument in the URL unencoded
            # (the facade's methods encode it): an id must not climb to
            # another route.
            raise ValueError(f"refusing a path with a dot segment: {route}")
        return path, list(request.headers.multi_items())

    def handle_request(self, request: httpx.Request) -> httpx.Response:
        path, headers = self._parts(request)
        status, answered, content = self.exchange(
            request.method, path, headers, request.read()
        )
        return httpx.Response(
            status, headers=answered, content=content, request=request
        )


class AsyncSealedTransport(httpx.AsyncBaseTransport):
    """`SealedTransport` for the generated `asyncio` calls: the native client
    blocks (with the GIL released), so each request runs on the loop's
    default executor."""

    def __init__(self, sync: SealedTransport):
        self._sync = sync

    async def handle_async_request(self, request: httpx.Request) -> httpx.Response:
        path, headers = SealedTransport._parts(request)
        body = await request.aread()
        loop = asyncio.get_running_loop()
        status, answered, content = await loop.run_in_executor(
            None, self._sync.exchange, request.method, path, headers, body
        )
        return httpx.Response(
            status, headers=answered, content=content, request=request
        )


def sealed_client(
    native: Any, passkey: Any = None, **options: Any
) -> AuthenticatedClient:
    """A generated-client `AuthenticatedClient` whose requests (sync and
    async) go through `native`'s sealed channel, with `native`'s credential.
    `options` are `SealedTransport`'s."""
    transport = SealedTransport(native, passkey, **options)
    client = AuthenticatedClient(base_url=BASE_URL, token="")
    client.set_httpx_client(
        httpx.Client(
            base_url=BASE_URL, transport=transport, trust_env=False, timeout=None
        )
    )
    client.set_async_httpx_client(
        httpx.AsyncClient(
            base_url=BASE_URL,
            transport=AsyncSealedTransport(transport),
            trust_env=False,
            timeout=None,
        )
    )
    return client
