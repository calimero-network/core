#!/usr/bin/env python3
"""Prove a device-key session can SUBSCRIBE to a context, on one transport.

Python rather than `curl`, unlike every other script here, because neither
transport is reachable from curl alone. WebSocket needs an HTTP/1.1 upgrade and
RFC 6455 framing; SSE needs a connection held open while a *second* request is
made on it. Both need to read a stream and keep reading. So this is stdlib only
-- no `websockets`, no `requests` -- because the merod image ships no CLI and
this runs as a `target: local` step on whatever the runner happens to be.

The sequence is the point, and it is the same on both transports:

  1. subscribe
  2. WAIT for the acknowledgement
  3. only then write
  4. assert a StateMutation naming that context arrives

Step 2 is not politeness. Writing first races the subscription, and the result
then depends on how fast the machine is rather than on how the node behaves --
a green run would prove nothing and a red one would name the wrong thing.

Step 4 is the assertion that matters. An ACCEPTED subscription is not a
DELIVERED event: a node can acknowledge `subscribe` and then send nothing --
wrong permission, wrong context, events routed elsewhere -- and a check that
stops at the acknowledgement passes anyway.

And the acknowledgement itself is checked for CONTENT, not for the absence of an
error, because of how the server refuses. `ws/subscribe.rs` drops a context the
caller may not observe and echoes only the ones it actually subscribed:

    Unauthorized ids are dropped rather than subscribed, and the response
    reflects only the contexts that were actually subscribed.

So a refusal arrives as `{"result":{"contextIds":[]}}` -- a success-shaped reply
with an empty list. Asserting the echo NAMES our context is what tells the two
apart; `not "error" in reply` cannot.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import socket
import struct
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import NoReturn

# Long enough to cover a cold node under a loaded CI runner, short enough that a
# genuinely undelivered event fails the step rather than hanging the run.
DEFAULT_TIMEOUT = 30.0


def log(msg: str) -> None:
    print(msg, flush=True)


def fail(msg: str) -> NoReturn:
    print(f"FAIL: {msg}", file=sys.stderr, flush=True)
    sys.exit(1)


# --------------------------------------------------------------------------
# The write whose delivery is being observed
# --------------------------------------------------------------------------


def post_json(url: str, body: dict, token: str | None = None, timeout: float = 30.0) -> dict:
    """POST JSON and parse JSON back, surfacing the body on an HTTP error.

    `urllib` raises on 4xx/5xx and the exception's `read()` holds the node's own
    explanation of the refusal, which is the only useful part -- a bare
    "HTTP Error 403" names nothing.
    """
    data = json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, method="POST")
    req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return json.loads(resp.read().decode())
    except urllib.error.HTTPError as err:
        detail = err.read().decode(errors="replace")[:400]
        raise RuntimeError(f"{url} -> HTTP {err.code}: {detail}") from err


def admin_token(url: str, username: str, password: str) -> str:
    """Mint an admin session from the node's `user_password` provider.

    The exact payload merobox's own `login` step posts (`commands/auth.py`):
    `public_key` is the username for this provider, and `timestamp` is REQUIRED
    -- `BaseTokenRequest` is `deny_unknown_fields`, so a body missing it is
    refused during deserialization, and the refusal names the parse rather than
    the login.

    This token is used ONLY to make the write that the subscription then
    observes. The subscription itself is authenticated by the device-key session
    and never by this -- that separation is the whole point of the scenario, so
    the two tokens are deliberately never interchangeable here.
    """
    res = post_json(
        f"{url}/auth/token",
        {
            "auth_method": "user_password",
            "public_key": username,
            "client_name": url,
            "timestamp": int(time.time()),
            "provider_data": {"username": username, "password": password},
        },
    )
    # The node wraps this route's body as `{"data": {...}, "error": null}` while
    # `/sse/subscription` answers flat. Unwrap rather than assume either shape:
    # the sibling shell scripts `sed` for `"access_token"` anywhere in the body
    # and so never had to notice the difference.
    body = res.get("data") if isinstance(res.get("data"), dict) else res
    token = body.get("access_token")
    if not token:
        raise RuntimeError(f"no admin token minted: {res}")
    return token


def write_to_context(url: str, token: str, context: str, key: str, value: str) -> None:
    """Mutate the context over JSON-RPC, so the subscriber has something to see.

    `execute` and not the delegated `intents` path on purpose. A warrant-backed
    write would make this scenario fail whenever the warrant machinery broke,
    which `delegated-authorship.yml` already covers exhaustively; what is under
    test here is event delivery, so the write should be the most ordinary one
    available.
    """
    res = post_json(
        f"{url}/jsonrpc",
        {
            "jsonrpc": "2.0",
            "id": "subscribe-probe",
            "method": "execute",
            "params": {"contextId": context, "method": "set", "argsJson": {"key": key, "value": value}},
        },
        token=token,
    )
    # Truthiness, not `"error" in res`: some routes answer
    # `{"data": ..., "error": null}`, where the key is always present and a
    # membership test would read every success as a failure.
    if res.get("error"):
        raise RuntimeError(f"the write the subscription should observe failed: {res}")
    log(f"  wrote {key}={value}")


# --------------------------------------------------------------------------
# Event recognition, shared by both transports
# --------------------------------------------------------------------------


def is_state_mutation(frame: str, context: str) -> bool:
    """Whether this frame is a StateMutation naming OUR context.

    Both halves are checked. `type` alone would accept a mutation in some other
    context the session also subscribes to, and `contextId` alone would accept
    any of the other payload variants that ride the same envelope
    (`SyncStatus`, `AppVersionChanged`, `XCall`), several of which arrive
    unprompted on a syncing node.
    """
    try:
        msg = json.loads(frame)
    except json.JSONDecodeError:
        return False
    result = msg.get("result")
    if not isinstance(result, dict):
        return False
    return result.get("type") == "StateMutation" and result.get("contextId") == context


def ack_names_context(frame: str, context: str, want_id: object) -> bool | None:
    """Classify a frame as our subscribe ack, and say whether it GRANTED.

    Returns True (granted), False (an ack that did not name our context -- i.e.
    a refusal), or None (not the ack at all, keep reading).

    Correlating on the request id matters: `ws/subscribe.rs` documents that
    events for a context may be pushed BEFORE the ack naming it, so the first
    frame to arrive is not necessarily the reply.
    """
    try:
        msg = json.loads(frame)
    except json.JSONDecodeError:
        return None
    if msg.get("id") != want_id:
        return None
    if "error" in msg:
        fail(f"the node refused the subscription: {frame[:400]}")
    granted = (msg.get("result") or {}).get("contextIds")
    if granted is None:
        return None
    return context in granted


# --------------------------------------------------------------------------
# WebSocket: RFC 6455, client side, the parts a subscribe needs
# --------------------------------------------------------------------------


class WebSocket:
    """A minimal RFC 6455 client: handshake, masked text out, frames in.

    Only what this probe needs. Fragmentation is handled because the server is
    free to use it; extensions are not negotiated, so no frame can arrive
    compressed.
    """

    def __init__(self, url: str, timeout: float):
        parts = urllib.parse.urlsplit(url)
        port = parts.port or (443 if parts.scheme == "wss" else 80)
        if parts.scheme == "wss":
            raise RuntimeError("wss is not supported by this probe; e2e nodes serve plain http")
        self.sock = socket.create_connection((parts.hostname, port), timeout=timeout)
        self.sock.settimeout(timeout)
        self.buf = b""

        path = parts.path or "/"
        if parts.query:
            path = f"{path}?{parts.query}"
        key = base64.b64encode(os.urandom(16)).decode()
        handshake = (
            f"GET {path} HTTP/1.1\r\n"
            f"Host: {parts.hostname}:{port}\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\n"
            "Sec-WebSocket-Version: 13\r\n"
            "\r\n"
        )
        self.sock.sendall(handshake.encode())

        while b"\r\n\r\n" not in self.buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise RuntimeError("the node closed the connection during the WebSocket handshake")
            self.buf += chunk
        head, _, rest = self.buf.partition(b"\r\n\r\n")
        self.buf = rest
        status = head.split(b"\r\n", 1)[0].decode(errors="replace")
        if "101" not in status:
            # An auth refusal lands here, not as a frame: the guard rejects the
            # request before the upgrade, so the status line IS the evidence.
            raise RuntimeError(f"the node refused the upgrade: {status} :: {head.decode(errors='replace')[:300]}")

    def send(self, text: str) -> None:
        payload = text.encode()
        header = bytearray([0x81])  # FIN | text
        mask = os.urandom(4)
        n = len(payload)
        # A client frame MUST be masked (RFC 6455 §5.1); an unmasked one is a
        # protocol error the server closes on.
        if n < 126:
            header.append(0x80 | n)
        elif n < (1 << 16):
            header.append(0x80 | 126)
            header += struct.pack("!H", n)
        else:
            header.append(0x80 | 127)
            header += struct.pack("!Q", n)
        header += mask
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
        self.sock.sendall(bytes(header) + masked)

    def _read(self, n: int) -> bytes:
        while len(self.buf) < n:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise RuntimeError("the node closed the WebSocket")
            self.buf += chunk
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def recv(self) -> str | None:
        """Next text message, or None on a control frame worth ignoring.

        Returns None rather than looping internally so the caller's deadline
        stays in charge of how long to wait overall.
        """
        message = b""
        while True:
            b0, b1 = self._read(2)
            fin, opcode = b0 & 0x80, b0 & 0x0F
            length = b1 & 0x7F
            if length == 126:
                (length,) = struct.unpack("!H", self._read(2))
            elif length == 127:
                (length,) = struct.unpack("!Q", self._read(8))
            # Server frames are never masked, so there is no mask key to read.
            payload = self._read(length) if length else b""

            if opcode == 0x8:
                raise RuntimeError("the node closed the WebSocket")
            if opcode == 0x9:  # ping -> pong, or the node drops us mid-wait
                # Masked like any other client frame. A control frame's payload
                # is always short enough for the 7-bit length, so no extended
                # length case arises here.
                mask = os.urandom(4)
                masked = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
                self.sock.sendall(bytes([0x8A, 0x80 | len(payload)]) + mask + masked)
                return None
            if opcode == 0xA:  # pong
                return None
            message += payload
            if fin:
                return message.decode(errors="replace")

    def close(self) -> None:
        try:
            self.sock.close()
        except OSError:
            pass


def run_ws(args: argparse.Namespace) -> None:
    # The token rides the QUERY STRING, which is the whole reason this transport
    # has to be proven separately: a browser cannot set a header on a WebSocket,
    # so `?token=` is the only way a real client authenticates here. The auth
    # guard accepts it only when there is no Authorization header
    # (`server/src/auth.rs`).
    ws_url = args.url.replace("http://", "ws://", 1) + "/ws?token=" + urllib.parse.quote(args.token)
    log("WS: opening")
    ws = WebSocket(ws_url, timeout=args.timeout)
    try:
        # No `jsonrpc` member. `RequestPayload` is an internally-tagged enum with
        # `deny_unknown_fields`, so including one is refused with
        # `invalid value: string "jsonrpc", expected "method" or "params"` --
        # which looks like a malformed subscribe and is really a malformed
        # envelope.
        request_id = 1
        ws.send(json.dumps({"id": request_id, "method": "subscribe", "params": {"contextIds": [args.context]}}))
        log("WS: subscribe sent")

        deadline = time.monotonic() + args.timeout
        pending: list[str] = []
        granted = None
        while time.monotonic() < deadline:
            frame = ws.recv()
            if frame is None:
                continue
            verdict = ack_names_context(frame, args.context, request_id)
            if verdict is None:
                # An event may precede the ack; keep it rather than discard it,
                # or a mutation delivered early would have to be written twice
                # to be seen.
                pending.append(frame)
                continue
            granted = verdict
            log(f"WS: ack {frame[:200]}")
            break
        if granted is None:
            fail("no subscribe acknowledgement arrived on the WebSocket")
        if not granted:
            fail(
                "the subscribe was acknowledged but the context was NOT granted -- the node "
                "dropped it as unauthorized and echoed an empty list"
            )

        write_to_context(args.write_url, args.write_token, args.context, args.key, args.value)

        for frame in pending:
            if is_state_mutation(frame, args.context):
                log(f"WS: StateMutation (pre-ack) {frame[:200]}")
                return
        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline:
            frame = ws.recv()
            if frame is None:
                continue
            if is_state_mutation(frame, args.context):
                log(f"WS: StateMutation {frame[:200]}")
                return
        fail("the subscription was accepted but no StateMutation was delivered over the WebSocket")
    finally:
        ws.close()


# --------------------------------------------------------------------------
# SSE: a held-open stream plus a second request that subscribes on it
# --------------------------------------------------------------------------


def run_sse(args: argparse.Namespace) -> None:
    """Subscribe over SSE, which takes two requests rather than one frame.

    Unlike WebSocket, the stream carries no client->server direction, so the
    subscribe cannot ride it. The node's shape (`server/src/sse.rs`) is:

        GET  /sse               -> the stream; its FIRST event announces
                                   `{"type":"connect","session_id":...}`
        POST /sse/subscription  -> `{"id":"<session_id>", "method":"subscribe",
                                     "params":{"contextIds":[...]}}`

    So the session id has to be read off the stream before anything can be
    subscribed to it -- opening the stream is NOT subscribing, and a check that
    stops at `200 text/event-stream` has tested only that a socket opened.

    Here the token rides an `Authorization` header, which is the other half of
    why both transports are proven: they authenticate by different means, so one
    working says nothing about the other.
    """
    parts = urllib.parse.urlsplit(args.url)
    import http.client

    conn = http.client.HTTPConnection(parts.hostname, parts.port or 80, timeout=args.timeout)
    log("SSE: opening")
    conn.request("GET", "/sse", headers={"Authorization": f"Bearer {args.token}", "Accept": "text/event-stream"})
    resp = conn.getresponse()
    if resp.status != 200:
        fail(f"the node refused the SSE stream: HTTP {resp.status} {resp.read()[:300]!r}")
    ctype = resp.getheader("content-type") or ""
    if "text/event-stream" not in ctype:
        fail(f"the SSE stream is not an event stream: content-type {ctype!r}")
    log(f"SSE: 200 {ctype}")

    def next_data(deadline: float) -> str | None:
        """Next `data:` payload, or None at the deadline.

        SSE frames are `data: <json>` lines terminated by a blank line; comments
        (`:` keep-alives) and the `id:`/`retry:` fields are skipped.
        """
        while time.monotonic() < deadline:
            try:
                line = resp.readline()
            except (socket.timeout, TimeoutError):
                return None
            if not line:
                return None
            text = line.decode(errors="replace").strip()
            if text.startswith("data:"):
                return text[5:].strip()
        return None

    try:
        deadline = time.monotonic() + args.timeout
        session_id = None
        while time.monotonic() < deadline:
            data = next_data(deadline)
            if data is None:
                break
            try:
                msg = json.loads(data)
            except json.JSONDecodeError:
                continue
            if msg.get("type") == "connect":
                session_id = msg.get("session_id")
                break
        if not session_id:
            fail("the SSE stream never announced a session id, so nothing could be subscribed to it")
        log(f"SSE: session {session_id}")

        # `id` is the SESSION id here and a string, where the WebSocket's is a
        # numeric request id (`sse::Request.id: String` vs `ws::Request.id:
        # Option<RequestId>`). Same `method`/`params` envelope otherwise.
        ack = post_json(
            f"{args.url}/sse/subscription",
            {"id": str(session_id), "method": "subscribe", "params": {"contextIds": [args.context]}},
            token=args.token,
        )
        log(f"SSE: ack {json.dumps(ack)[:200]}")
        if ack.get("error"):
            fail(f"the node refused the SSE subscription: {ack}")
        ack_body = ack.get("data") if isinstance(ack.get("data"), dict) else ack
        result = (ack_body.get("result") or {})
        # The two transports ack the SAME operation with DIFFERENT field names.
        # WebSocket returns the typed `SubscribeResponse` (`contextIds`,
        # `groupIds`); SSE builds an ad-hoc body in `sse/handlers.rs`
        # (`{"status":"subscribed","contexts":[...],"groups":[...]}`). Both are
        # read here, so this assertion keeps holding whichever way that
        # divergence is later resolved — and `contexts` first, because that is
        # what the node actually sends today.
        granted = result.get("contexts")
        if granted is None:
            granted = result.get("contextIds")
        if granted is None:
            fail(f"the SSE subscribe was not acknowledged: {ack}")
        if args.context not in granted:
            fail(
                "the SSE subscribe was acknowledged but the context was NOT granted -- the node "
                f"dropped it as unauthorized and echoed {granted}"
            )

        write_to_context(args.write_url, args.write_token, args.context, args.key, args.value)

        deadline = time.monotonic() + args.timeout
        while time.monotonic() < deadline:
            data = next_data(deadline)
            if data is None:
                break
            if is_state_mutation(data, args.context):
                log(f"SSE: StateMutation {data[:200]}")
                return
        fail("the subscription was accepted but no StateMutation was delivered over SSE")
    finally:
        conn.close()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--transport", choices=("ws", "sse"), required=True)
    ap.add_argument("--url", required=True, help="node to subscribe on")
    ap.add_argument("--token", required=True, help="the device-key session token")
    ap.add_argument("--context", required=True)
    ap.add_argument("--write-url", required=True, help="node to make the observed write on")
    ap.add_argument("--write-user", default="dev")
    ap.add_argument("--write-password", default="dev-password")
    ap.add_argument("--key", required=True)
    ap.add_argument("--value", required=True)
    ap.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT)
    args = ap.parse_args()

    try:
        args.write_token = admin_token(args.write_url, args.write_user, args.write_password)
    except RuntimeError as err:
        fail(str(err))

    try:
        (run_ws if args.transport == "ws" else run_sse)(args)
    except RuntimeError as err:
        fail(str(err))
    log(f"PASS: a device-key session subscribed over {args.transport.upper()} and received the write")


if __name__ == "__main__":
    main()
